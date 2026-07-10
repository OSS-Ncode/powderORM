//! The async database client — connection management and query execution.
//!
//! The backend (SQLite via `rusqlite`) is synchronous, so blocking calls are
//! dispatched to Tokio's blocking pool. Every public method is `async`, which
//! is what lets the language bindings map a Rust `Future` cleanly onto a JS
//! `Promise` (napi) and a Python `asyncio` awaitable (PyO3).

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use rusqlite::types::Value as SqlValue;
use rusqlite::{ffi, Connection, OpenFlags};

use crate::array::{Column, ColumnData};
use crate::batch::RecordBatch;
use crate::codec;
use crate::error::{Error, Result};
use crate::query::Value;
use crate::schema::{DataType, Field};

/// An async handle to a database connection.
///
/// Cloning is cheap (`Arc` bump) and every clone shares the same underlying
/// connection, serialized behind a mutex.
#[derive(Clone)]
pub struct Client {
    inner: Backend,
}

/// The engine behind a [`Client`]. SQLite is bundled; PostgreSQL is opt-in
/// via the `postgres` cargo feature and selected by connection URL.
#[derive(Clone)]
enum Backend {
    Sqlite(SqliteClient),
    #[cfg(feature = "postgres")]
    Pg(Arc<crate::pg::PgBackend>),
    #[cfg(feature = "mysql")]
    My(Arc<crate::my::MyBackend>),
}

#[derive(Clone)]
struct SqliteClient {
    conn: Arc<Mutex<Connection>>,
    cache: Arc<Mutex<QueryCache>>,
    /// Canonical open target — used to open extra read-only connections for
    /// the parallel table scan. For `:memory:` this is a process-unique
    /// shared-cache URI, so worker connections see the same database while
    /// separate `Client`s stay fully isolated from each other.
    url: Arc<str>,
    /// Pooled read-only worker connections for parallel scans.
    workers: Arc<Mutex<Vec<Connection>>>,
    /// In-memory databases cannot be written by connections outside this
    /// process, which is what makes the lock-free cache probe sound.
    in_memory: bool,
}

/// Bounded result cache: `(sql, params)` -> encoded PCB buffer.
///
/// Entries are shared `Arc`s, so a hit is a pointer clone — the encoded bytes
/// are handed straight back across the FFI with no copy. Correctness:
/// - every [`Client::execute`] clears the cache (in-process writes);
/// - on file-backed databases `PRAGMA data_version` is checked before any
///   cache read, catching writers on *other* connections/processes;
/// - only statements SQLite reports read-only (`sqlite3_stmt_readonly`) and
///   whose SQL contains no known non-deterministic function are inserted.
#[derive(Default)]
struct QueryCache {
    map: HashMap<Vec<u8>, Arc<Vec<u8>>>,
    order: VecDeque<Vec<u8>>,
    total_bytes: usize,
    /// Last observed `PRAGMA data_version` (file-backed databases only).
    data_version: Option<i64>,
}

const CACHE_MAX_ENTRIES: usize = 32;
const CACHE_MAX_TOTAL_BYTES: usize = 128 << 20;
const CACHE_MAX_RESULT_BYTES: usize = 64 << 20;

impl QueryCache {
    fn clear(&mut self) {
        self.map.clear();
        self.order.clear();
        self.total_bytes = 0;
    }

    fn insert(&mut self, key: Vec<u8>, bytes: Arc<Vec<u8>>) {
        if bytes.len() > CACHE_MAX_RESULT_BYTES {
            return;
        }
        while self.order.len() >= CACHE_MAX_ENTRIES
            || (self.total_bytes + bytes.len() > CACHE_MAX_TOTAL_BYTES && !self.order.is_empty())
        {
            if let Some(old) = self.order.pop_front() {
                if let Some(v) = self.map.remove(&old) {
                    self.total_bytes -= v.len();
                }
            } else {
                break;
            }
        }
        self.total_bytes += bytes.len();
        self.order.push_back(key.clone());
        self.map.insert(key, bytes);
    }
}

/// Canonical cache key: SQL text plus type-tagged parameter bytes.
fn cache_key(sql: &str, params: &[SqlValue]) -> Vec<u8> {
    let mut k = Vec::with_capacity(sql.len() + 1 + params.len() * 12);
    k.extend_from_slice(sql.as_bytes());
    k.push(0);
    for p in params {
        match p {
            SqlValue::Null => k.push(0),
            SqlValue::Integer(i) => {
                k.push(1);
                k.extend_from_slice(&i.to_le_bytes());
            }
            SqlValue::Real(f) => {
                k.push(2);
                k.extend_from_slice(&f.to_bits().to_le_bytes());
            }
            SqlValue::Text(s) => {
                k.push(3);
                k.extend_from_slice(&(s.len() as u32).to_le_bytes());
                k.extend_from_slice(s.as_bytes());
            }
            SqlValue::Blob(b) => {
                k.push(4);
                k.extend_from_slice(&(b.len() as u32).to_le_bytes());
                k.extend_from_slice(b);
            }
        }
    }
    k
}

/// Conservative determinism gate: SQL mentioning any of these is never cached
/// (same policy family as MySQL's query cache). False positives only make a
/// query uncached — never incorrect.
fn is_cache_safe(sql: &str) -> bool {
    const BANNED: &[&str] = &[
        "random",
        "current_",
        "datetime",
        "julianday",
        "strftime",
        "unixepoch",
        "date(",
        "time(",
        "last_insert_rowid",
        "changes",
        "sqlite_offset",
    ];
    // Allocation-free case-insensitive scan of the (short) SQL text.
    let bytes = sql.as_bytes();
    !BANNED.iter().any(|b| {
        let pat = b.as_bytes();
        bytes
            .windows(pat.len())
            .any(|w| w.eq_ignore_ascii_case(pat))
    })
}

impl Client {
    /// Connect using a URL.
    ///
    /// Accepts `sqlite::memory:`, `:memory:`, `sqlite://<path>`,
    /// `sqlite:<path>`, or a bare filesystem path. With the `postgres`
    /// feature enabled, `postgres://` / `postgresql://` URLs open a
    /// PostgreSQL connection instead.
    pub async fn connect(url: &str) -> Result<Self> {
        if url.starts_with("postgres://") || url.starts_with("postgresql://") {
            #[cfg(feature = "postgres")]
            {
                let url = url.to_string();
                let backend = tokio::task::spawn_blocking(move || crate::pg::PgBackend::connect(&url))
                    .await
                    .map_err(|e| Error::Join(e.to_string()))??;
                return Ok(Self {
                    inner: Backend::Pg(Arc::new(backend)),
                });
            }
            #[cfg(not(feature = "postgres"))]
            return Err(Error::InvalidUrl(
                "postgres:// URLs need powder-core built with the `postgres` feature".into(),
            ));
        }
        if url.starts_with("mysql://") || url.starts_with("mariadb://") {
            #[cfg(feature = "mysql")]
            {
                let url = url.replace("mariadb://", "mysql://");
                let backend = tokio::task::spawn_blocking(move || crate::my::MyBackend::connect(&url))
                    .await
                    .map_err(|e| Error::Join(e.to_string()))??;
                return Ok(Self {
                    inner: Backend::My(Arc::new(backend)),
                });
            }
            #[cfg(not(feature = "mysql"))]
            return Err(Error::InvalidUrl(
                "mysql:// URLs need powder-core built with the `mysql` feature".into(),
            ));
        }
        Ok(Self {
            inner: Backend::Sqlite(SqliteClient::connect(url).await?),
        })
    }

    /// Run a statement that does not return rows (INSERT/UPDATE/DDL); returns
    /// the number of affected rows.
    pub async fn execute(&self, sql: &str, params: Vec<Value>) -> Result<usize> {
        match &self.inner {
            Backend::Sqlite(s) => s.execute(sql, params).await,
            #[cfg(feature = "postgres")]
            Backend::Pg(p) => {
                let (p, sql) = (p.clone(), sql.to_string());
                tokio::task::spawn_blocking(move || p.execute(&sql, &params))
                    .await
                    .map_err(|e| Error::Join(e.to_string()))?
            }
            #[cfg(feature = "mysql")]
            Backend::My(m) => {
                let (m, sql) = (m.clone(), sql.to_string());
                tokio::task::spawn_blocking(move || m.execute(&sql, &params))
                    .await
                    .map_err(|e| Error::Join(e.to_string()))?
            }
        }
    }

    /// Run a query and return the result set as a columnar [`RecordBatch`].
    pub async fn query(&self, sql: &str, params: Vec<Value>) -> Result<RecordBatch> {
        match &self.inner {
            Backend::Sqlite(s) => s.query(sql, params).await,
            #[cfg(feature = "postgres")]
            Backend::Pg(p) => {
                let (p, sql) = (p.clone(), sql.to_string());
                tokio::task::spawn_blocking(move || p.query(&sql, &params))
                    .await
                    .map_err(|e| Error::Join(e.to_string()))?
            }
            #[cfg(feature = "mysql")]
            Backend::My(m) => {
                let (m, sql) = (m.clone(), sql.to_string());
                tokio::task::spawn_blocking(move || m.query(&sql, &params))
                    .await
                    .map_err(|e| Error::Join(e.to_string()))?
            }
        }
    }

    /// Synchronous, non-blocking cache probe (SQLite `:memory:` only —
    /// other backends always return `None`).
    pub fn probe_cache(&self, sql: &str, params: Vec<Value>) -> Option<Arc<Vec<u8>>> {
        match &self.inner {
            Backend::Sqlite(s) => s.probe_cache(sql, params),
            #[cfg(feature = "postgres")]
            Backend::Pg(_) => None,
            #[cfg(feature = "mysql")]
            Backend::My(_) => None,
        }
    }

    /// Run a query and return the result already serialized as a PCB buffer,
    /// shared through the query cache where the backend supports it.
    pub async fn query_bytes_shared(&self, sql: &str, params: Vec<Value>) -> Result<Arc<Vec<u8>>> {
        match &self.inner {
            Backend::Sqlite(s) => s.query_bytes_shared(sql, params).await,
            #[cfg(feature = "postgres")]
            Backend::Pg(_) => {
                let batch = self.query(sql, params).await?;
                Ok(Arc::new(batch.encode()))
            }
            #[cfg(feature = "mysql")]
            Backend::My(_) => {
                let batch = self.query(sql, params).await?;
                Ok(Arc::new(batch.encode()))
            }
        }
    }

    /// [`Self::query_bytes_shared`], materialized to an owned buffer.
    pub async fn query_bytes(&self, sql: &str, params: Vec<Value>) -> Result<Vec<u8>> {
        let shared = self.query_bytes_shared(sql, params).await?;
        Ok(Arc::try_unwrap(shared).unwrap_or_else(|arc| arc.as_ref().clone()))
    }
}

impl SqliteClient {
    async fn connect(url: &str) -> Result<Self> {
        let in_memory = url == ":memory:" || url == "sqlite::memory:";
        let url = url.to_string();
        let (conn, canonical) = tokio::task::spawn_blocking(move || open_connection(&url))
            .await
            .map_err(|e| Error::Join(e.to_string()))??;
        let client = Self {
            conn: Arc::new(Mutex::new(conn)),
            cache: Arc::new(Mutex::new(QueryCache::default())),
            url: canonical.into(),
            workers: Arc::new(Mutex::new(Vec::new())),
            in_memory,
        };
        // Pre-open scan workers off the critical path so the first parallel
        // query finds a filled pool instead of opening connections inline.
        {
            let url = client.url.clone();
            let workers = client.workers.clone();
            let want = std::thread::available_parallelism()
                .map(|n| n.get())
                .unwrap_or(1)
                .min(12);
            std::thread::spawn(move || {
                for _ in 0..want {
                    match open_worker(&url) {
                        Some(c) => match workers.lock() {
                            Ok(mut pool) => pool.push(c),
                            Err(_) => break,
                        },
                        None => break,
                    }
                }
            });
        }
        Ok(client)
    }

    /// Run a statement that does not return rows (INSERT/UPDATE/DDL); returns
    /// the number of affected rows.
    pub async fn execute(&self, sql: &str, params: Vec<Value>) -> Result<usize> {
        let sql = sql.to_string();
        let cache = self.cache.clone();
        self.with_conn(move |conn| {
            let sql_params = to_sql_values(params);
            let n = conn.execute(&sql, rusqlite::params_from_iter(sql_params.iter()))?;
            // Writes invalidate every cached result.
            if let Ok(mut c) = cache.lock() {
                c.clear();
            }
            Ok(n)
        })
        .await
    }

    /// Run a query and return the result set as a columnar [`RecordBatch`].
    pub async fn query(&self, sql: &str, params: Vec<Value>) -> Result<RecordBatch> {
        let sql = sql.to_string();
        let url = self.url.clone();
        let workers = self.workers.clone();
        self.with_conn(move |conn| {
            let ctx = ParallelCtx {
                url: &url,
                workers: &workers,
            };
            let sql_params = to_sql_values(params);
            Ok(run_query_params(Some(&ctx), conn, &sql, &sql_params)?.0)
        })
        .await
    }

    /// Synchronous, non-blocking cache probe: returns the encoded PCB buffer
    /// for `(sql, params)` iff it is already cached and provably fresh without
    /// touching the connection. Only `:memory:` databases qualify (no other
    /// connection can mutate them), which is what lets bindings answer a
    /// repeated query in microseconds without a thread hop.
    pub fn probe_cache(&self, sql: &str, params: Vec<Value>) -> Option<Arc<Vec<u8>>> {
        if !self.in_memory || !is_cache_safe(sql) {
            return None;
        }
        let key = cache_key(sql, &to_sql_values(params));
        self.cache.lock().ok()?.map.get(&key).cloned()
    }

    /// Run a query and return the result already serialized as an PCB buffer,
    /// shared through the query cache — a repeat of an identical read-only
    /// query on an unchanged database returns the same `Arc` with no re-scan
    /// and no copy. Encoding runs on the blocking pool alongside the query.
    pub async fn query_bytes_shared(&self, sql: &str, params: Vec<Value>) -> Result<Arc<Vec<u8>>> {
        let sql = sql.to_string();
        let cache = self.cache.clone();
        let in_memory = self.in_memory;
        let url = self.url.clone();
        let workers = self.workers.clone();
        self.with_conn(move |conn| {
            let sql_params = to_sql_values(params);
            let cacheable_sql = is_cache_safe(&sql);
            let key = cacheable_sql.then(|| cache_key(&sql, &sql_params));

            if let (Some(key), Ok(mut c)) = (&key, cache.lock()) {
                // File-backed databases can be written by other connections;
                // `data_version` changes whenever another connection commits.
                if !in_memory {
                    let dv: i64 = conn.query_row("PRAGMA data_version", [], |r| r.get(0))?;
                    if c.data_version != Some(dv) {
                        c.clear();
                        c.data_version = Some(dv);
                    }
                }
                if let Some(hit) = c.map.get(key) {
                    return Ok(hit.clone());
                }
            }

            let ctx = ParallelCtx {
                url: &url,
                workers: &workers,
            };
            let (encoded, stmt_readonly) = run_query_bytes(Some(&ctx), conn, &sql, &sql_params)?;
            let bytes = Arc::new(encoded);
            if let (Some(key), true) = (key, stmt_readonly) {
                if let Ok(mut c) = cache.lock() {
                    c.insert(key, bytes.clone());
                }
            }
            Ok(bytes)
        })
        .await
    }

    async fn with_conn<T, F>(&self, f: F) -> Result<T>
    where
        F: FnOnce(&Connection) -> Result<T> + Send + 'static,
        T: Send + 'static,
    {
        // Uncontended fast path: run inline on the current (runtime) thread.
        // The connection mutex serializes all work anyway, so this does not
        // reduce concurrency — it removes two thread hops. When the
        // connection is busy, fall back to the blocking pool and wait there.
        if let Ok(guard) = self.conn.try_lock() {
            return f(&guard);
        }
        let conn = self.conn.clone();
        tokio::task::spawn_blocking(move || {
            let guard = conn
                .lock()
                .map_err(|_| Error::Database("connection mutex poisoned".into()))?;
            f(&guard)
        })
        .await
        .map_err(|e| Error::Join(e.to_string()))?
    }
}

/// Open the primary connection; returns it plus the canonical URL that extra
/// worker connections should open to reach the same database.
fn open_connection(url: &str) -> Result<(Connection, String)> {
    if url == ":memory:" || url == "sqlite::memory:" {
        // Process-unique named in-memory database on the `memdb` VFS: behaves
        // exactly like `:memory:` from the outside (dies with its last
        // connection, invisible to other Clients), but lets this Client open
        // additional read-only connections for the parallel scan. Unlike
        // `cache=shared` — whose single shared pager cache serializes readers
        // behind a mutex — memdb gives every connection its own page cache,
        // so range-partitioned scans genuinely run in parallel.
        static MEM_ID: AtomicU64 = AtomicU64::new(0);
        let name = format!(
            "file:/powder-mem-{}?vfs=memdb",
            MEM_ID.fetch_add(1, Ordering::Relaxed)
        );
        let conn = Connection::open(&name)?; // default flags include URI
        return Ok((conn, name));
    }
    let path = url
        .strip_prefix("sqlite://")
        .or_else(|| url.strip_prefix("sqlite:"))
        .unwrap_or(url);
    Ok((Connection::open(path)?, path.to_string()))
}

/// Open one read-only worker connection to `url`.
fn open_worker(url: &str) -> Option<Connection> {
    Connection::open_with_flags(
        url,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_URI | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .ok()
}

fn to_sql_values(params: Vec<Value>) -> Vec<SqlValue> {
    params
        .into_iter()
        .map(|v| match v {
            Value::Null => SqlValue::Null,
            Value::Int(i) => SqlValue::Integer(i),
            Value::Float(f) => SqlValue::Real(f),
            Value::Text(s) => SqlValue::Text(s),
            Value::Bool(b) => SqlValue::Integer(b as i64),
        })
        .collect()
}

/// Extra connections used to range-partition a table scan across threads.
struct ParallelCtx<'a> {
    url: &'a str,
    workers: &'a Mutex<Vec<Connection>>,
}

/// Query directly to encoded PCB bytes. On the parallel-scan path this can
/// skip the chunk merge entirely: when every chunk is null-free, un-promoted,
/// class-uniform, and (for a trailing ORDER BY) already ordered across chunk
/// boundaries, the encoder writes the chunk slices straight into the output
/// buffer — one copy from scan buffers to wire bytes, nothing in between.
fn run_query_bytes(
    ctx: Option<&ParallelCtx>,
    conn: &Connection,
    sql: &str,
    sql_params: &[SqlValue],
) -> Result<(Vec<u8>, bool)> {
    let is_select = sql
        .trim_start()
        .get(..6)
        .is_some_and(|p| p.eq_ignore_ascii_case("SELECT"));
    if is_select && sql_params.is_empty() {
        let (scan_sql, order) = match split_simple_order_by(sql) {
            Some((inner, key, desc)) => (inner, Some((key, desc))),
            None => (sql, None),
        };
        if let Some((cols, table)) = parse_parallel_shape(scan_sql) {
            if let Some((chunks, names, readonly)) = try_parallel_scan(ctx, conn, &cols, table)? {
                if let Some(bytes) = encode_chunks_direct(&chunks, &names, order) {
                    return Ok((bytes, readonly));
                }
                let builders = merge_chunks(chunks);
                match order {
                    None => return Ok((finish_batch(builders, names)?.encode(), readonly)),
                    Some((key, desc)) => {
                        if let Some(b) = sort_built(builders, &names, key, desc) {
                            return Ok((finish_batch(b, names)?.encode(), readonly));
                        }
                        // Key shape needs SQLite's class-aware sorter.
                        let (b, n, ro) = stream_query(conn, sql, sql_params)?;
                        return Ok((finish_batch(b, n)?.encode(), ro));
                    }
                }
            }
        }
    }
    let (batch, readonly) = run_query_params(ctx, conn, sql, sql_params)?;
    Ok((batch.encode(), readonly))
}

/// Encode parallel-scan chunks without merging them. Returns `None` whenever
/// any gate fails; the caller then merges and takes the ordinary path.
fn encode_chunks_direct(
    chunks: &[Vec<AdaptiveCol>],
    names: &[String],
    order: Option<(&str, bool)>,
) -> Option<Vec<u8>> {
    let ncols = names.len();
    let nrows: usize = chunks
        .iter()
        .map(|ch| ch.first().map(|c| c.len).unwrap_or(0))
        .sum();

    fn class(d: &AdaptiveData) -> u8 {
        match d {
            AdaptiveData::Int(_) => 0,
            AdaptiveData::Float(_) => 1,
            AdaptiveData::Str { .. } => 2,
        }
    }

    // Gates: null-free, un-promoted, valid text, one storage class per column.
    for c in 0..ncols {
        let first_class = class(&chunks[0][c].data);
        for ch in chunks {
            let col = &ch[c];
            if !col.nulls.is_empty()
                || col.promoted
                || !col.text_valid
                || class(&col.data) != first_class
            {
                return None;
            }
        }
    }

    // A trailing ORDER BY is satisfied only if the key is an output column,
    // blob-free, sorted inside every chunk, and ordered across boundaries.
    if let Some((key, desc)) = order {
        let k = names.iter().position(|n| n.eq_ignore_ascii_case(key))?;
        if chunks.iter().any(|ch| ch[k].saw_blob) {
            return None;
        }
        {
            use rayon::prelude::*;
            if !chunks.par_iter().all(|ch| is_sorted(&ch[k].data, desc)) {
                return None;
            }
        }
        for w in chunks.windows(2) {
            let (prev, next) = (&w[0][k], &w[1][k]);
            if prev.len == 0 || next.len == 0 {
                continue;
            }
            if !boundary_ordered(&prev.data, &next.data, desc) {
                return None;
            }
        }
    }

    let cols: Vec<codec::ColParts<'_>> = (0..ncols)
        .map(|c| match &chunks[0][c].data {
            AdaptiveData::Int(_) => codec::ColParts::Int(
                chunks
                    .iter()
                    .map(|ch| match &ch[c].data {
                        AdaptiveData::Int(v) => &v[..],
                        _ => unreachable!(),
                    })
                    .collect(),
            ),
            AdaptiveData::Float(_) => codec::ColParts::Float(
                chunks
                    .iter()
                    .map(|ch| match &ch[c].data {
                        AdaptiveData::Float(v) => &v[..],
                        _ => unreachable!(),
                    })
                    .collect(),
            ),
            AdaptiveData::Str { .. } => codec::ColParts::Utf8 {
                parts: chunks
                    .iter()
                    .map(|ch| match &ch[c].data {
                        AdaptiveData::Str { offsets, data } => (&offsets[..], &data[..]),
                        _ => unreachable!(),
                    })
                    .collect(),
                ascii: chunks.iter().all(|ch| ch[c].ascii),
            },
        })
        .collect();
    Some(codec::encode_parts(names, &cols, nrows))
}

/// Is `last(prev) <= first(next)` (or `>=` for DESC)?
fn boundary_ordered(prev: &AdaptiveData, next: &AdaptiveData, desc: bool) -> bool {
    use std::cmp::Ordering;
    let ord = match (prev, next) {
        (AdaptiveData::Int(a), AdaptiveData::Int(b)) => a[a.len() - 1].cmp(&b[0]),
        (AdaptiveData::Float(a), AdaptiveData::Float(b)) => a[a.len() - 1].total_cmp(&b[0]),
        (
            AdaptiveData::Str { offsets: ao, data: ad },
            AdaptiveData::Str { offsets: bo, data: bd },
        ) => {
            let last = &ad[ao[ao.len() - 2] as usize..ao[ao.len() - 1] as usize];
            let first = &bd[bo[0] as usize..bo[1] as usize];
            last.cmp(first)
        }
        _ => return false,
    };
    if desc {
        ord != Ordering::Less
    } else {
        ord != Ordering::Greater
    }
}

/// Execute a query; returns the batch plus whether the statement was
/// read-only per `sqlite3_stmt_readonly` (the query-cache admission gate).
fn run_query_params(
    ctx: Option<&ParallelCtx>,
    conn: &Connection,
    sql: &str,
    sql_params: &[SqlValue],
) -> Result<(RecordBatch, bool)> {
    // Sort pull-up: for a trailing `ORDER BY <col> ASC|DESC`, scan without the
    // ORDER BY and order the columnar result in-engine. SQLite's external
    // sorter costs far more than sorting the finished column buffers — and
    // when the scan already comes back ordered (key correlated with insertion
    // order), the whole sort collapses to an O(n) check. Any query shape the
    // rewrite cannot prove safe falls back to SQLite's own sorter. Gated to
    // plain SELECTs so a DML `RETURNING` statement can never execute twice.
    let is_select = sql
        .trim_start()
        .get(..6)
        .is_some_and(|p| p.eq_ignore_ascii_case("SELECT"));
    if is_select {
        let (scan_sql, order) = match split_simple_order_by(sql) {
            Some((inner, key, desc)) => (inner, Some((key, desc))),
            None => (sql, None),
        };

        // Parallel range-partitioned scan for bare single-table selects.
        if sql_params.is_empty() {
            if let Some((cols, table)) = parse_parallel_shape(scan_sql) {
                if let Some((chunks, names, readonly)) =
                    try_parallel_scan(ctx, conn, &cols, table)?
                {
                    let builders = merge_chunks(chunks);
                    match order {
                        None => return Ok((finish_batch(builders, names)?, readonly)),
                        Some((key, desc)) => match sort_built(builders, &names, key, desc) {
                            Some(b) => return Ok((finish_batch(b, names)?, readonly)),
                            None => {} // fall through to SQLite's sorter below
                        },
                    }
                }
            }
        }

        if let Some((key, desc)) = order {
            let (builders, names, readonly) = stream_query(conn, scan_sql, sql_params)?;
            match sort_built(builders, &names, key, desc) {
                Some(builders) => return Ok((finish_batch(builders, names)?, readonly)),
                None => {} // unsupported key column shape — rerun with SQLite sort
            }
        }
    }

    let (builders, names, readonly) = stream_query(conn, sql, sql_params)?;
    Ok((finish_batch(builders, names)?, readonly))
}

/// Match `SELECT <bare idents | *> FROM <ident>` and nothing else — the only
/// shape the parallel scan handles. Anything richer (WHERE, JOIN, GROUP BY,
/// expressions, aggregates) returns `None` and runs serially: splitting an
/// aggregate or filter across rowid ranges would change its meaning.
fn parse_parallel_shape(sql: &str) -> Option<(Vec<String>, &str)> {
    fn is_simple_ident(s: &str) -> bool {
        let mut chars = s.chars();
        chars
            .next()
            .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
            && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
    }

    let s = sql.trim();
    let s = s.strip_suffix(';').unwrap_or(s).trim_end();
    if s.len() < 7 || !s[..6].eq_ignore_ascii_case("SELECT") {
        return None;
    }
    let rest = s[6..].trim_start();

    // Find the single ` FROM ` separator (bare idents cannot contain spaces);
    // allocation-free case-insensitive search.
    let from_pos = rest
        .as_bytes()
        .windows(6)
        .position(|w| w.eq_ignore_ascii_case(b" from "))?;
    let cols_part = &rest[..from_pos];
    let table = rest[from_pos + 6..].trim();
    if !is_simple_ident(table) {
        return None;
    }

    let cols: Vec<&str> = cols_part.split(',').map(str::trim).collect();
    if cols.is_empty() || cols.iter().any(|c| c.is_empty()) {
        return None;
    }
    if !(cols == ["*"] || cols.iter().all(|c| is_simple_ident(c))) {
        return None;
    }
    Some((cols.into_iter().map(str::to_string).collect(), table))
}

/// Minimum rowid span before a scan is worth partitioning across threads.
const PARALLEL_MIN_SPAN: i64 = 64 * 1024;

/// Range-partition `SELECT cols FROM table` by rowid across worker threads.
/// Returns `Ok(None)` whenever any precondition fails — the caller then runs
/// the ordinary serial scan. Chunks come back in rowid order, so their
/// concatenation is ordering-identical to a serial scan.
fn try_parallel_scan(
    ctx: Option<&ParallelCtx>,
    conn: &Connection,
    cols: &[String],
    table: &str,
) -> Result<Option<(Vec<Vec<AdaptiveCol>>, Vec<String>, bool)>> {
    let Some(ctx) = ctx else { return Ok(None) };

    let threads = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1)
        .min(12);
    if threads < 2 {
        return Ok(None);
    }

    // WITHOUT ROWID tables (or a missing table) fail here -> serial path.
    // min and max are separate scalar subqueries: SQLite's min/max
    // optimization (a b-tree endpoint seek instead of a full scan) only
    // fires for a single aggregate per SELECT.
    let endpoints = conn
        .query_row(
            &format!(
                "SELECT (SELECT min(rowid) FROM {table}), (SELECT max(rowid) FROM {table})"
            ),
            [],
            |r| Ok((r.get::<_, Option<i64>>(0)?, r.get::<_, Option<i64>>(1)?)),
        )
        .ok();
    let Some((Some(min), Some(max))) = endpoints else {
        return Ok(None);
    };
    if max.saturating_sub(min) < PARALLEL_MIN_SPAN {
        return Ok(None);
    }

    // A user column shadowing `rowid` would make range predicates lie (and a
    // NULL in it would drop rows entirely). Bail out to the serial scan.
    {
        let mut stmt = conn.prepare(&format!("PRAGMA table_info({table})"))?;
        let mut rows = stmt.query([])?;
        while let Some(row) = rows.next()? {
            let name: String = row.get(1)?;
            if name.eq_ignore_ascii_case("rowid")
                || name.eq_ignore_ascii_case("_rowid_")
                || name.eq_ignore_ascii_case("oid")
            {
                return Ok(None);
            }
        }
    }

    // Grab pooled worker connections; open the rest read-only on demand. The
    // whole scan runs on worker connections (the caller's connection is not
    // Sync and cannot enter the rayon pool).
    let mut worker_conns: Vec<Connection> = {
        let mut pool = ctx
            .workers
            .lock()
            .map_err(|_| Error::Database("worker pool mutex poisoned".into()))?;
        let keep = pool.len() - threads.min(pool.len());
        pool.split_off(keep)
    };
    while worker_conns.len() < threads {
        match open_worker(ctx.url) {
            Some(c) => worker_conns.push(c),
            None => break,
        }
    }
    if worker_conns.len() < 2 {
        if let Ok(mut pool) = ctx.workers.lock() {
            pool.append(&mut worker_conns);
        }
        return Ok(None);
    }

    let timing = std::env::var_os("POWDER_TIMING").is_some();
    let t_pre = std::time::Instant::now();

    let parts = worker_conns.len();
    let scan_sql = format!(
        "SELECT {} FROM {table} WHERE rowid BETWEEN ? AND ?",
        cols.join(", ")
    );

    // Contiguous rowid ranges covering [min, max].
    let span = (max - min) + 1;
    let step = span / parts as i64;
    let bounds: Vec<(i64, i64)> = (0..parts)
        .map(|i| {
            let lo = min + step * i as i64;
            let hi = if i == parts - 1 { max } else { min + step * (i as i64 + 1) - 1 };
            (lo, hi)
        })
        .collect();

    // `Connection` is Send but not Sync: move each one into its task and take
    // it back with the result so it can return to the pool. rayon's resident
    // pool avoids spawning OS threads on every query.
    let (chunks, mut returned) = {
        let mut slots: Vec<Option<(Connection, Result<(Vec<AdaptiveCol>, Vec<String>, bool)>)>> =
            worker_conns
                .drain(..)
                .map(|c| Some((c, Err(Error::Join("scan task did not run".into())))))
                .collect();
        rayon::scope(|s| {
            for (slot, &(lo, hi)) in slots.iter_mut().zip(&bounds) {
                let scan_sql = &scan_sql;
                s.spawn(move |_| {
                    let (wconn, _) = slot.take().expect("slot filled above");
                    let r = stream_query(
                        &wconn,
                        scan_sql,
                        &[SqlValue::Integer(lo), SqlValue::Integer(hi)],
                    );
                    *slot = Some((wconn, r));
                });
            }
        });
        let mut results = Vec::with_capacity(parts);
        let mut conns = Vec::with_capacity(parts);
        for slot in slots {
            let (c, r) = slot.expect("worker task completed");
            conns.push(c);
            results.push(r);
        }
        (results, conns)
    };

    // Return connections to the pool before touching the results.
    if let Ok(mut pool) = ctx.workers.lock() {
        pool.append(&mut returned);
    }
    let t_scan = t_pre.elapsed();

    let mut ok_chunks = Vec::with_capacity(parts);
    for c in chunks {
        match c {
            Ok(v) => ok_chunks.push(v),
            Err(_) => return Ok(None), // any worker failure -> serial retry
        }
    }

    let names = ok_chunks[0].1.clone();
    let readonly = ok_chunks.iter().all(|(_, _, ro)| *ro);
    let per_chunk: Vec<Vec<AdaptiveCol>> = ok_chunks.into_iter().map(|(b, _, _)| b).collect();
    if timing {
        eprintln!(
            "[powder] parallel scan: parts={parts} scan+build={:.2}ms",
            t_scan.as_secs_f64() * 1e3,
        );
    }
    Ok(Some((per_chunk, names, readonly)))
}

/// Concatenate per-range column chunks, unifying storage classes first so a
/// type promotion in one range applies to all of them. Columns are merged on
/// separate threads — they are fully independent.
fn merge_chunks(chunks: Vec<Vec<AdaptiveCol>>) -> Vec<AdaptiveCol> {
    let ncols = chunks.first().map(|c| c.len()).unwrap_or(0);
    let mut per_col: Vec<Vec<AdaptiveCol>> = (0..ncols).map(|_| Vec::new()).collect();
    for chunk in chunks {
        for (c, col) in chunk.into_iter().enumerate() {
            per_col[c].push(col);
        }
    }
    if per_col.len() < 2 {
        return per_col.into_iter().map(merge_col).collect();
    }
    use rayon::prelude::*;
    per_col.into_par_iter().map(merge_col).collect()
}

fn merge_col(mut parts: Vec<AdaptiveCol>) -> AdaptiveCol {
    fn class(d: &AdaptiveData) -> u8 {
        match d {
            AdaptiveData::Int(_) => 0,
            AdaptiveData::Float(_) => 1,
            AdaptiveData::Str { .. } => 2,
        }
    }

    let mut promoted = parts.iter().any(|p| p.promoted);
    let saw_blob = parts.iter().any(|p| p.saw_blob);
    let text_valid = parts.iter().all(|p| p.text_valid);

    // Only ranges holding at least one non-null value have a storage class.
    let classes: Vec<u8> = parts
        .iter()
        .filter(|p| p.len > p.nulls.len())
        .map(|p| class(&p.data))
        .collect();
    let target = classes.iter().copied().max().unwrap_or(0);
    promoted |= classes.iter().any(|&c| c != target);

    for p in &mut parts {
        match target {
            // promote_to_str converts Int directly (no lossy float hop).
            2 => p.promote_to_str(),
            1 => p.promote_to_float(),
            _ => {}
        }
    }

    // Reserve the final size up front so the concatenation never re-allocates.
    let total_rows: usize = parts.iter().map(|p| p.len).sum();
    let total_str_bytes: usize = parts
        .iter()
        .map(|p| match &p.data {
            AdaptiveData::Str { data, .. } => data.len(),
            _ => 0,
        })
        .sum();
    let mut it = parts.into_iter();
    let mut acc = it.next().expect("merge_col: no chunks");
    match &mut acc.data {
        AdaptiveData::Int(v) => v.reserve(total_rows.saturating_sub(v.len())),
        AdaptiveData::Float(v) => v.reserve(total_rows.saturating_sub(v.len())),
        AdaptiveData::Str { offsets, data } => {
            offsets.reserve(total_rows.saturating_sub(offsets.len() - 1) + 1);
            data.reserve(total_str_bytes.saturating_sub(data.len()));
        }
    }
    for p in it {
        let row_base = acc.len as u32;
        match (&mut acc.data, p.data) {
            (AdaptiveData::Int(a), AdaptiveData::Int(b)) => a.extend_from_slice(&b),
            (AdaptiveData::Float(a), AdaptiveData::Float(b)) => a.extend_from_slice(&b),
            (
                AdaptiveData::Str { offsets: ao, data: ad },
                AdaptiveData::Str { offsets: bo, data: bd },
            ) => {
                let byte_base = ad.len() as u32;
                ad.extend_from_slice(&bd);
                ao.extend(bo.into_iter().skip(1).map(|o| o + byte_base));
            }
            _ => unreachable!("chunks were unified to one storage class"),
        }
        acc.nulls.extend(p.nulls.into_iter().map(|i| i + row_base));
        acc.len += p.len;
    }
    acc.promoted = promoted;
    acc.saw_blob = saw_blob;
    acc.text_valid = text_valid;
    acc
}

/// RAII guard that finalizes a raw prepared statement on every exit path.
struct RawStmt(*mut ffi::sqlite3_stmt);

impl Drop for RawStmt {
    fn drop(&mut self) {
        unsafe {
            ffi::sqlite3_finalize(self.0);
        }
    }
}

/// Last-error helper for the raw FFI path.
fn db_error(db: *mut ffi::sqlite3) -> Error {
    let msg = unsafe {
        let p = ffi::sqlite3_errmsg(db);
        if p.is_null() {
            "unknown SQLite error".to_string()
        } else {
            std::ffi::CStr::from_ptr(p).to_string_lossy().into_owned()
        }
    };
    Error::Database(msg)
}

/// Stream rows straight into contiguous per-column buffers, driving SQLite
/// through raw `sqlite3_step` / `sqlite3_column_*` calls — the same loop a
/// hand-written C client would run, with no per-row driver wrapping. SQLite
/// is dynamically typed, so each column starts as Int64 and promotes
/// (Int64 -> Float64 -> Utf8) the moment a cell requires it, which yields
/// the same result as whole-column inference: any text/blob -> Utf8, else
/// any real -> Float64, else Int64.
///
/// Safety: the connection is exclusively held by the caller (mutex in
/// [`Client`]), every text/blob slice is copied before the next `step`, and
/// [`RawStmt`] finalizes the statement on all exit paths.
fn stream_query(
    conn: &Connection,
    sql: &str,
    sql_params: &[SqlValue],
) -> Result<(Vec<AdaptiveCol>, Vec<String>, bool)> {
    let csql = std::ffi::CString::new(sql)
        .map_err(|_| Error::Database("SQL contains an interior NUL byte".into()))?;

    unsafe {
        let db = conn.handle();
        let mut p: *mut ffi::sqlite3_stmt = std::ptr::null_mut();
        if ffi::sqlite3_prepare_v2(db, csql.as_ptr(), -1, &mut p, std::ptr::null_mut())
            != ffi::SQLITE_OK
        {
            return Err(db_error(db));
        }
        if p.is_null() {
            // Whitespace/comment-only SQL prepares to nothing.
            return Err(Error::Database("empty SQL statement".into()));
        }
        let stmt = RawStmt(p);
        let readonly = ffi::sqlite3_stmt_readonly(p) != 0;

        let expected = ffi::sqlite3_bind_parameter_count(p) as usize;
        if expected != sql_params.len() {
            return Err(Error::Database(format!(
                "wrong number of parameters: expected {expected}, got {}",
                sql_params.len()
            )));
        }
        for (i, v) in sql_params.iter().enumerate() {
            let idx = (i + 1) as std::os::raw::c_int;
            let rc = match v {
                SqlValue::Null => ffi::sqlite3_bind_null(p, idx),
                SqlValue::Integer(n) => ffi::sqlite3_bind_int64(p, idx, *n),
                SqlValue::Real(f) => ffi::sqlite3_bind_double(p, idx, *f),
                SqlValue::Text(s) => ffi::sqlite3_bind_text(
                    p,
                    idx,
                    s.as_ptr().cast(),
                    s.len() as std::os::raw::c_int,
                    ffi::SQLITE_TRANSIENT(),
                ),
                SqlValue::Blob(b) => ffi::sqlite3_bind_blob(
                    p,
                    idx,
                    b.as_ptr().cast(),
                    b.len() as std::os::raw::c_int,
                    ffi::SQLITE_TRANSIENT(),
                ),
            };
            if rc != ffi::SQLITE_OK {
                return Err(db_error(db));
            }
        }

        let ncols = ffi::sqlite3_column_count(p) as usize;
        let names: Vec<String> = (0..ncols)
            .map(|c| {
                let np = ffi::sqlite3_column_name(p, c as std::os::raw::c_int);
                if np.is_null() {
                    String::new()
                } else {
                    std::ffi::CStr::from_ptr(np).to_string_lossy().into_owned()
                }
            })
            .collect();

        let mut builders: Vec<AdaptiveCol> = (0..ncols).map(|_| AdaptiveCol::new()).collect();
        loop {
            match ffi::sqlite3_step(p) {
                ffi::SQLITE_ROW => {
                    for (c, b) in builders.iter_mut().enumerate() {
                        let c = c as std::os::raw::c_int;
                        match ffi::sqlite3_column_type(p, c) {
                            ffi::SQLITE_INTEGER => b.push_int(ffi::sqlite3_column_int64(p, c)),
                            ffi::SQLITE_FLOAT => b.push_real(ffi::sqlite3_column_double(p, c)),
                            ffi::SQLITE_TEXT => {
                                let len = ffi::sqlite3_column_bytes(p, c) as usize;
                                let ptr = ffi::sqlite3_column_text(p, c);
                                let bytes = if len == 0 || ptr.is_null() {
                                    &[][..]
                                } else {
                                    std::slice::from_raw_parts(ptr, len)
                                };
                                b.push_text(bytes);
                            }
                            ffi::SQLITE_BLOB => {
                                let len = ffi::sqlite3_column_bytes(p, c) as usize;
                                let ptr = ffi::sqlite3_column_blob(p, c);
                                let bytes = if len == 0 || ptr.is_null() {
                                    &[][..]
                                } else {
                                    std::slice::from_raw_parts(ptr.cast::<u8>(), len)
                                };
                                b.push_blob(bytes);
                            }
                            _ => b.push_null(),
                        }
                    }
                }
                ffi::SQLITE_DONE => break,
                _ => return Err(db_error(db)),
            }
        }
        drop(stmt);
        for b in &mut builders {
            b.validate_text();
        }
        Ok((builders, names, readonly))
    }
}

fn finish_batch(builders: Vec<AdaptiveCol>, names: Vec<String>) -> Result<RecordBatch> {
    let columns = builders
        .into_iter()
        .zip(names)
        .map(|(b, name)| b.finish(name))
        .collect();
    RecordBatch::try_new(columns)
}

/// If `sql` ends with a simple `ORDER BY <identifier> ASC|DESC` (single key,
/// no COLLATE / LIMIT / OFFSET / multi-key), return `(sql_without_order_by,
/// key, descending)`. Anything else returns `None` and keeps SQLite in charge.
fn split_simple_order_by(sql: &str) -> Option<(&str, &str, bool)> {
    fn rsplit_token(s: &str) -> Option<(&str, &str)> {
        let trimmed = s.trim_end();
        let cut = trimmed.rfind(char::is_whitespace)?;
        let token = trimmed[cut..].trim();
        (!token.is_empty()).then_some((&trimmed[..cut], token))
    }
    fn is_simple_ident(s: &str) -> bool {
        let mut chars = s.chars();
        chars
            .next()
            .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
            && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
    }

    let s = sql.trim_end();
    let s = s.strip_suffix(';').unwrap_or(s);
    let (rest, dir) = rsplit_token(s)?;
    let desc = dir.eq_ignore_ascii_case("DESC");
    if !desc && !dir.eq_ignore_ascii_case("ASC") {
        return None;
    }
    let (rest, ident) = rsplit_token(rest)?;
    if !is_simple_ident(ident) {
        return None;
    }
    let (rest, by) = rsplit_token(rest)?;
    if !by.eq_ignore_ascii_case("BY") {
        return None;
    }
    let (rest, order) = rsplit_token(rest)?;
    if !order.eq_ignore_ascii_case("ORDER") {
        return None;
    }
    let inner = rest.trim_end();
    (!inner.is_empty()).then_some((inner, ident, desc))
}

/// Order the built columns by `key`. Returns `None` when the key column's
/// shape can't be proven to sort identically to SQLite (missing from the
/// output, type-promoted, blob-typed, or nullable) — the caller then reruns
/// the original SQL and lets SQLite sort.
fn sort_built(
    mut builders: Vec<AdaptiveCol>,
    names: &[String],
    key: &str,
    desc: bool,
) -> Option<Vec<AdaptiveCol>> {
    let key_idx = names.iter().position(|n| n.eq_ignore_ascii_case(key))?;
    let k = &builders[key_idx];
    // Promotion means mixed storage classes; SQLite orders those by class
    // (NULL < numeric < text < blob), which the flattened column lost. NULLs
    // are excluded to keep the comparator total on plain values.
    if k.promoted || k.saw_blob || !k.nulls.is_empty() {
        return None;
    }

    let nrows = k.len;
    if nrows < 2 || is_sorted(&k.data, desc) {
        return Some(builders);
    }

    let mut idx: Vec<u32> = (0..nrows as u32).collect();
    match &k.data {
        AdaptiveData::Int(v) => {
            if desc {
                idx.sort_unstable_by_key(|&i| std::cmp::Reverse(v[i as usize]));
            } else {
                idx.sort_unstable_by_key(|&i| v[i as usize]);
            }
        }
        AdaptiveData::Float(v) => {
            // SQLite never stores NaN (it becomes NULL), so total_cmp matches.
            if desc {
                idx.sort_unstable_by(|&a, &b| v[b as usize].total_cmp(&v[a as usize]));
            } else {
                idx.sort_unstable_by(|&a, &b| v[a as usize].total_cmp(&v[b as usize]));
            }
        }
        AdaptiveData::Str { offsets, data } => {
            let get = |i: u32| &data[offsets[i as usize] as usize..offsets[i as usize + 1] as usize];
            if desc {
                idx.sort_unstable_by(|&a, &b| get(b).cmp(get(a)));
            } else {
                idx.sort_unstable_by(|&a, &b| get(a).cmp(get(b)));
            }
        }
    }

    for b in &mut builders {
        b.permute(&idx);
    }
    Some(builders)
}

fn is_sorted(data: &AdaptiveData, desc: bool) -> bool {
    fn check<T, F: Fn(&T, &T) -> bool>(v: &[T], ok: F) -> bool {
        v.windows(2).all(|w| ok(&w[0], &w[1]))
    }
    match data {
        AdaptiveData::Int(v) => {
            if desc {
                check(v, |a, b| a >= b)
            } else {
                check(v, |a, b| a <= b)
            }
        }
        AdaptiveData::Float(v) => {
            if desc {
                check(v, |a, b| a.total_cmp(b).is_ge())
            } else {
                check(v, |a, b| a.total_cmp(b).is_le())
            }
        }
        AdaptiveData::Str { offsets, data } => {
            let get = |i: usize| &data[offsets[i] as usize..offsets[i + 1] as usize];
            (1..offsets.len() - 1).all(|i| {
                let (a, b) = (get(i - 1), get(i));
                if desc {
                    a >= b
                } else {
                    a <= b
                }
            })
        }
    }
}

/// Column payload being built while streaming rows, in the promotion order
/// Int64 -> Float64 -> Utf8. NULL slots hold the type's zero/empty placeholder.
enum AdaptiveData {
    Int(Vec<i64>),
    Float(Vec<f64>),
    Str { offsets: Vec<u32>, data: Vec<u8> },
}

/// One column built in a single pass over the result rows.
struct AdaptiveCol {
    data: AdaptiveData,
    /// Number of rows pushed so far.
    len: usize,
    /// Row indices holding NULL, ascending. Stays empty (and costs nothing per
    /// row) for the common non-null column; bit-packed only at `finish`.
    nulls: Vec<u32>,
    /// Whether the column ever changed storage class (mixed SQLite types).
    promoted: bool,
    /// Whether any cell arrived as a blob (ordered after text by SQLite).
    saw_blob: bool,
    /// Whole-buffer UTF-8 check result, stamped by [`Self::validate_text`]
    /// on the scan thread. `false` routes the column through the repair path.
    text_valid: bool,
    /// Whole-buffer ASCII check result (implies `text_valid`); becomes the
    /// PCB "pure ASCII" directory hint that lets readers take an O(1)
    /// substring path per string.
    ascii: bool,
}

impl AdaptiveCol {
    fn new() -> Self {
        Self {
            data: AdaptiveData::Int(Vec::new()),
            len: 0,
            nulls: Vec::new(),
            promoted: false,
            saw_blob: false,
            text_valid: true,
            ascii: false,
        }
    }

    /// Validate the accumulated char data in one pass (word-at-a-time SIMD in
    /// core), instead of one check per short text cell during the scan.
    fn validate_text(&mut self) {
        if let AdaptiveData::Str { data, .. } = &self.data {
            if data.is_ascii() {
                self.ascii = true;
                self.text_valid = true;
            } else {
                self.text_valid = std::str::from_utf8(data).is_ok();
            }
        }
    }

    /// Reorder the column so row `r` of the result is old row `idx[r]`.
    fn permute(&mut self, idx: &[u32]) {
        self.data = match &self.data {
            AdaptiveData::Int(v) => {
                AdaptiveData::Int(idx.iter().map(|&i| v[i as usize]).collect())
            }
            AdaptiveData::Float(v) => {
                AdaptiveData::Float(idx.iter().map(|&i| v[i as usize]).collect())
            }
            AdaptiveData::Str { offsets, data } => {
                let mut new_offsets = Vec::with_capacity(offsets.len());
                new_offsets.push(0u32);
                let mut new_data = Vec::with_capacity(data.len());
                for &i in idx {
                    let (s, e) = (offsets[i as usize] as usize, offsets[i as usize + 1] as usize);
                    new_data.extend_from_slice(&data[s..e]);
                    new_offsets.push(new_data.len() as u32);
                }
                AdaptiveData::Str {
                    offsets: new_offsets,
                    data: new_data,
                }
            }
        };
        if !self.nulls.is_empty() {
            let mut is_null = vec![false; self.len];
            for &i in &self.nulls {
                is_null[i as usize] = true;
            }
            self.nulls = idx
                .iter()
                .enumerate()
                .filter(|&(_, &old)| is_null[old as usize])
                .map(|(r, _)| r as u32)
                .collect();
        }
    }

    #[inline]
    fn push_null(&mut self) {
        match &mut self.data {
            AdaptiveData::Int(v) => v.push(0),
            AdaptiveData::Float(v) => v.push(0.0),
            AdaptiveData::Str { offsets, data } => offsets.push(data.len() as u32),
        }
        self.nulls.push(self.len as u32);
        self.len += 1;
    }

    #[inline]
    fn push_int(&mut self, i: i64) {
        match &mut self.data {
            AdaptiveData::Int(v) => v.push(i),
            AdaptiveData::Float(v) => {
                self.promoted = true;
                v.push(i as f64);
            }
            AdaptiveData::Str { offsets, data } => {
                self.promoted = true;
                data.extend_from_slice(i.to_string().as_bytes());
                offsets.push(data.len() as u32);
            }
        }
        self.len += 1;
    }

    #[inline]
    fn push_real(&mut self, f: f64) {
        if matches!(self.data, AdaptiveData::Int(_)) {
            self.promote_to_float();
        }
        match &mut self.data {
            AdaptiveData::Float(v) => v.push(f),
            AdaptiveData::Str { offsets, data } => {
                self.promoted = true;
                data.extend_from_slice(f.to_string().as_bytes());
                offsets.push(data.len() as u32);
            }
            AdaptiveData::Int(_) => unreachable!(),
        }
        self.len += 1;
    }

    #[inline]
    fn push_text(&mut self, t: &[u8]) {
        self.promote_to_str();
        if let AdaptiveData::Str { offsets, data } = &mut self.data {
            // No per-cell validation: `finish` checks the whole char-data
            // buffer once (word-at-a-time), which is far cheaper than 100k+
            // short-slice checks. Invalid bytes (only possible when raw bytes
            // were bound as text) are repaired there.
            data.extend_from_slice(t);
            offsets.push(data.len() as u32);
        }
        self.len += 1;
    }

    #[inline]
    fn push_blob(&mut self, b: &[u8]) {
        self.saw_blob = true;
        self.promote_to_str();
        if let AdaptiveData::Str { offsets, data } = &mut self.data {
            match std::str::from_utf8(b) {
                Ok(_) => data.extend_from_slice(b),
                Err(_) => data.extend_from_slice(String::from_utf8_lossy(b).as_bytes()),
            }
            offsets.push(data.len() as u32);
        }
        self.len += 1;
    }

    fn promote_to_float(&mut self) {
        if let AdaptiveData::Int(v) = &self.data {
            // Only a real class mix if a non-null value was already buffered.
            self.promoted |= self.len > self.nulls.len();
            self.data = AdaptiveData::Float(v.iter().map(|&i| i as f64).collect());
        }
    }

    fn promote_to_str(&mut self) {
        if matches!(self.data, AdaptiveData::Str { .. }) {
            return;
        }
        self.promoted |= self.len > self.nulls.len();
        let (offsets, data) = match &self.data {
            AdaptiveData::Int(v) => stringify(v.iter().map(i64::to_string), &self.nulls),
            AdaptiveData::Float(v) => stringify(v.iter().map(f64::to_string), &self.nulls),
            AdaptiveData::Str { .. } => unreachable!(),
        };
        self.data = AdaptiveData::Str { offsets, data };
    }

    fn finish(self, name: String) -> Column {
        let nullable = !self.nulls.is_empty();
        let validity = if nullable {
            // All-valid template, then clear the (typically few) null bits.
            let mut bits = vec![0xFFu8; self.len.div_ceil(8)];
            if self.len % 8 != 0 {
                *bits.last_mut().unwrap() = (1u8 << (self.len % 8)) - 1;
            }
            for &i in &self.nulls {
                bits[i as usize / 8] &= !(1 << (i % 8));
            }
            Some(bits)
        } else {
            None
        };
        let (data_type, data) = match self.data {
            AdaptiveData::Int(v) => (DataType::Int64, ColumnData::Int64(v)),
            AdaptiveData::Float(v) => (DataType::Float64, ColumnData::Float64(v)),
            AdaptiveData::Str { offsets, data } => {
                let (offsets, data) = if self.text_valid {
                    (offsets, data)
                } else {
                    repair_utf8(offsets, data)
                };
                (DataType::Utf8, ColumnData::Utf8 { offsets, data })
            }
        };
        Column {
            field: Field::new(name, data_type, nullable),
            len: self.len,
            validity,
            data,
        }
    }
}

/// Rebuild a Utf8 column slice-by-slice, lossily re-encoding any value that is
/// not valid UTF-8 (only reachable when raw bytes were bound as SQL text).
fn repair_utf8(offsets: Vec<u32>, data: Vec<u8>) -> (Vec<u32>, Vec<u8>) {
    let mut new_data = Vec::with_capacity(data.len());
    let mut new_offsets = Vec::with_capacity(offsets.len());
    new_offsets.push(0u32);
    for w in offsets.windows(2) {
        let s = &data[w[0] as usize..w[1] as usize];
        match std::str::from_utf8(s) {
            Ok(_) => new_data.extend_from_slice(s),
            Err(_) => new_data.extend_from_slice(String::from_utf8_lossy(s).as_bytes()),
        }
        new_offsets.push(new_data.len() as u32);
    }
    (new_offsets, new_data)
}

/// Re-render already-buffered numeric values as strings when a column
/// promotes to Utf8; NULL slots become empty strings (validity marks them).
fn stringify(values: impl Iterator<Item = String>, nulls: &[u32]) -> (Vec<u32>, Vec<u8>) {
    let mut offsets = vec![0u32];
    let mut data = Vec::new();
    let mut next_null = 0usize;
    for (i, s) in values.enumerate() {
        if nulls.get(next_null) == Some(&(i as u32)) {
            next_null += 1;
        } else {
            data.extend_from_slice(s.as_bytes());
        }
        offsets.push(data.len() as u32);
    }
    (offsets, data)
}
