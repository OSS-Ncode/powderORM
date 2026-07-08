//! The async database client — connection management and query execution.
//!
//! The backend (SQLite via `rusqlite`) is synchronous, so blocking calls are
//! dispatched to Tokio's blocking pool. Every public method is `async`, which
//! is what lets the language bindings map a Rust `Future` cleanly onto a JS
//! `Promise` (napi) and a Python `asyncio` awaitable (PyO3).

use std::sync::{Arc, Mutex};

use rusqlite::types::{Value as SqlValue, ValueRef};
use rusqlite::Connection;

use crate::array::{Column, ColumnData};
use crate::batch::RecordBatch;
use crate::error::{Error, Result};
use crate::query::Value;
use crate::schema::{DataType, Field};

/// An async handle to a database connection.
///
/// Cloning is cheap (`Arc` bump) and every clone shares the same underlying
/// connection, serialized behind a mutex.
#[derive(Clone)]
pub struct Client {
    conn: Arc<Mutex<Connection>>,
}

impl Client {
    /// Connect using a URL.
    ///
    /// Accepts `sqlite::memory:`, `:memory:`, `sqlite://<path>`,
    /// `sqlite:<path>`, or a bare filesystem path.
    pub async fn connect(url: &str) -> Result<Self> {
        let url = url.to_string();
        let conn = tokio::task::spawn_blocking(move || open_connection(&url))
            .await
            .map_err(|e| Error::Join(e.to_string()))??;
        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
        })
    }

    /// Run a statement that does not return rows (INSERT/UPDATE/DDL); returns
    /// the number of affected rows.
    pub async fn execute(&self, sql: &str, params: Vec<Value>) -> Result<usize> {
        let sql = sql.to_string();
        self.with_conn(move |conn| {
            let sql_params = to_sql_values(params);
            let n = conn.execute(&sql, rusqlite::params_from_iter(sql_params.iter()))?;
            Ok(n)
        })
        .await
    }

    /// Run a query and return the result set as a columnar [`RecordBatch`].
    pub async fn query(&self, sql: &str, params: Vec<Value>) -> Result<RecordBatch> {
        let sql = sql.to_string();
        self.with_conn(move |conn| run_query(conn, &sql, params)).await
    }

    /// Run a query and return the result already serialized as an NCB buffer —
    /// the entry point the FFI bindings use to hand bytes back with no
    /// intermediate materialization on the host-language side. Encoding runs
    /// on the blocking pool alongside the query itself.
    pub async fn query_bytes(&self, sql: &str, params: Vec<Value>) -> Result<Vec<u8>> {
        let sql = sql.to_string();
        self.with_conn(move |conn| Ok(run_query(conn, &sql, params)?.encode()))
            .await
    }

    async fn with_conn<T, F>(&self, f: F) -> Result<T>
    where
        F: FnOnce(&Connection) -> Result<T> + Send + 'static,
        T: Send + 'static,
    {
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

fn open_connection(url: &str) -> Result<Connection> {
    let conn = if url == ":memory:" || url == "sqlite::memory:" {
        Connection::open_in_memory()?
    } else if let Some(path) = url
        .strip_prefix("sqlite://")
        .or_else(|| url.strip_prefix("sqlite:"))
    {
        Connection::open(path)?
    } else {
        Connection::open(url)?
    };
    Ok(conn)
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

fn run_query(conn: &Connection, sql: &str, params: Vec<Value>) -> Result<RecordBatch> {
    let sql_params = to_sql_values(params);

    // Sort pull-up: for a trailing `ORDER BY <col> ASC|DESC`, scan without the
    // ORDER BY and order the columnar result in-engine. SQLite's external
    // sorter costs far more than sorting the finished column buffers — and
    // when the scan already comes back ordered (key correlated with insertion
    // order), the whole sort collapses to an O(n) check. Any query shape the
    // rewrite cannot prove safe falls back to SQLite's own sorter.
    if let Some((inner, key, desc)) = split_simple_order_by(sql) {
        let (builders, names) = stream_query(conn, inner, &sql_params)?;
        match sort_built(builders, &names, key, desc) {
            Some(builders) => return finish_batch(builders, names),
            None => {} // unsupported key column shape — rerun with SQLite sort
        }
    }

    let (builders, names) = stream_query(conn, sql, &sql_params)?;
    finish_batch(builders, names)
}

/// Stream rows straight into contiguous per-column buffers via borrowed
/// `ValueRef`s — no per-cell heap value, no row-major intermediate. SQLite
/// is dynamically typed, so each column starts as Int64 and promotes
/// (Int64 -> Float64 -> Utf8) the moment a cell requires it, which yields
/// the same result as whole-column inference: any text/blob -> Utf8, else
/// any real -> Float64, else Int64.
fn stream_query(
    conn: &Connection,
    sql: &str,
    sql_params: &[SqlValue],
) -> Result<(Vec<AdaptiveCol>, Vec<String>)> {
    let mut stmt = conn.prepare(sql)?;
    let ncols = stmt.column_count();
    let names: Vec<String> = (0..ncols)
        .map(|i| stmt.column_name(i).map(str::to_string))
        .collect::<std::result::Result<_, _>>()?;

    let mut builders: Vec<AdaptiveCol> = (0..ncols).map(|_| AdaptiveCol::new()).collect();
    let mut rows = stmt.query(rusqlite::params_from_iter(sql_params.iter()))?;
    while let Some(row) = rows.next()? {
        for (i, b) in builders.iter_mut().enumerate() {
            b.push(row.get_ref(i)?);
        }
    }
    Ok((builders, names))
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
    if k.promoted || k.saw_blob || k.any_null {
        return None;
    }

    let nrows = k.valid.len();
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
    /// Per-row validity; bit-packed only at `finish`, and only if a NULL was seen.
    valid: Vec<bool>,
    any_null: bool,
    /// Whether the column ever changed storage class (mixed SQLite types).
    promoted: bool,
    /// Whether any cell arrived as a blob (ordered after text by SQLite).
    saw_blob: bool,
}

impl AdaptiveCol {
    fn new() -> Self {
        Self {
            data: AdaptiveData::Int(Vec::new()),
            valid: Vec::new(),
            any_null: false,
            promoted: false,
            saw_blob: false,
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
        if self.any_null {
            self.valid = idx.iter().map(|&i| self.valid[i as usize]).collect();
        }
    }

    fn push(&mut self, cell: ValueRef<'_>) {
        match cell {
            ValueRef::Null => {
                match &mut self.data {
                    AdaptiveData::Int(v) => v.push(0),
                    AdaptiveData::Float(v) => v.push(0.0),
                    AdaptiveData::Str { offsets, data } => offsets.push(data.len() as u32),
                }
                self.valid.push(false);
                self.any_null = true;
                return;
            }
            ValueRef::Integer(i) => match &mut self.data {
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
            },
            ValueRef::Real(f) => {
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
            }
            ValueRef::Text(t) | ValueRef::Blob(t) => {
                if matches!(cell, ValueRef::Blob(_)) {
                    self.saw_blob = true;
                }
                self.promote_to_str();
                if let AdaptiveData::Str { offsets, data } = &mut self.data {
                    match std::str::from_utf8(t) {
                        Ok(_) => data.extend_from_slice(t),
                        Err(_) => data.extend_from_slice(String::from_utf8_lossy(t).as_bytes()),
                    }
                    offsets.push(data.len() as u32);
                }
            }
        }
        self.valid.push(true);
    }

    fn promote_to_float(&mut self) {
        if let AdaptiveData::Int(v) = &self.data {
            // Only a real class mix if a non-null value was already buffered.
            self.promoted |= self.valid.iter().any(|&ok| ok);
            self.data = AdaptiveData::Float(v.iter().map(|&i| i as f64).collect());
        }
    }

    fn promote_to_str(&mut self) {
        if matches!(self.data, AdaptiveData::Str { .. }) {
            return;
        }
        self.promoted |= self.valid.iter().any(|&ok| ok);
        let (offsets, data) = match &self.data {
            AdaptiveData::Int(v) => stringify(v.iter().map(i64::to_string), &self.valid, v.len()),
            AdaptiveData::Float(v) => {
                stringify(v.iter().map(f64::to_string), &self.valid, v.len())
            }
            AdaptiveData::Str { .. } => unreachable!(),
        };
        self.data = AdaptiveData::Str { offsets, data };
    }

    fn finish(self, name: String) -> Column {
        let len = self.valid.len();
        let validity = if self.any_null {
            let mut bits = vec![0u8; len.div_ceil(8)];
            for (i, v) in self.valid.iter().enumerate() {
                if *v {
                    bits[i / 8] |= 1 << (i % 8);
                }
            }
            Some(bits)
        } else {
            None
        };
        let (data_type, data) = match self.data {
            AdaptiveData::Int(v) => (DataType::Int64, ColumnData::Int64(v)),
            AdaptiveData::Float(v) => (DataType::Float64, ColumnData::Float64(v)),
            AdaptiveData::Str { offsets, data } => (DataType::Utf8, ColumnData::Utf8 { offsets, data }),
        };
        Column {
            field: Field::new(name, data_type, self.any_null),
            len,
            validity,
            data,
        }
    }
}

/// Re-render already-buffered numeric values as strings when a column
/// promotes to Utf8; NULL slots become empty strings (validity marks them).
fn stringify(
    values: impl Iterator<Item = String>,
    valid: &[bool],
    len: usize,
) -> (Vec<u32>, Vec<u8>) {
    let mut offsets = Vec::with_capacity(len + 1);
    offsets.push(0u32);
    let mut data = Vec::new();
    for (i, s) in values.enumerate() {
        if valid[i] {
            data.extend_from_slice(s.as_bytes());
        }
        offsets.push(data.len() as u32);
    }
    (offsets, data)
}
