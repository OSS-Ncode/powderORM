//! Microsoft SQL Server runtime backend (feature `mssql`).
//!
//! Mirrors the Postgres/MySQL paths: run a query, stream rows into the
//! shared [`ColumnBuilder`]s, hand back a [`RecordBatch`] the codec encodes
//! to PCB unchanged. `tiberius` is async-only, so the backend owns a small
//! current-thread runtime and exposes the same *synchronous* surface as the
//! other server backends — [`crate::Client`] already dispatches these calls
//! to Tokio's blocking pool.
//!
//! Dialect shims applied here so the bindings' shared SQL keeps working:
//! - `?` placeholders → `@P1..@Pn` (skipping literals and comments);
//! - `BEGIN [IMMEDIATE]` → `BEGIN TRANSACTION`, `SAVEPOINT x` →
//!   `SAVE TRANSACTION x`, `ROLLBACK TO x` → `ROLLBACK TRANSACTION x`,
//!   `RELEASE x` → no-op (T-SQL savepoints are released implicitly).
//!
//! URL forms: `mssql://user:pass@host[:port][/database]` (SQL auth) and
//! `mssql://host[:port][/database]` (Windows integrated auth).
//! `sqlserver://` is an accepted alias.

use std::sync::Mutex;

use tiberius::{AuthMethod, ColumnData, ColumnType, Config, EncryptionLevel};
use tokio::net::TcpStream;
use tokio_util::compat::{Compat, TokioAsyncWriteCompatExt};

use crate::array::ColumnBuilder;
use crate::batch::RecordBatch;
use crate::error::{Error, Result};
use crate::query::Value;
use crate::schema::DataType;

type TClient = tiberius::Client<Compat<TcpStream>>;

pub struct MsBackend {
    rt: tokio::runtime::Runtime,
    client: Mutex<TClient>,
}

impl MsBackend {
    pub fn connect(url: &str) -> Result<Self> {
        let cfg = parse_url(url)?;
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|e| Error::Database(format!("mssql runtime: {e}")))?;
        let host = cfg.get_addr();
        let client = rt
            .block_on(async {
                let tcp = TcpStream::connect(&host).await?;
                tcp.set_nodelay(true)?;
                tiberius::Client::connect(cfg, tcp.compat_write())
                    .await
                    .map_err(std::io::Error::other)
            })
            .map_err(|e| Error::Database(format!("mssql connect: {e}")))?;
        Ok(Self {
            rt,
            client: Mutex::new(client),
        })
    }

    pub fn execute(&self, sql: &str, params: &[Value]) -> Result<usize> {
        let Some(sql) = normalize_tx(sql) else {
            return Ok(0); // RELEASE <sp> — implicit in T-SQL
        };
        let sql = translate_placeholders(&sql);
        let bound = bind(params);
        let refs: Vec<&dyn tiberius::ToSql> =
            bound.iter().map(|b| b.as_ref() as &dyn tiberius::ToSql).collect();
        let mut client = self.lock()?;
        // Transaction control must run as a direct batch: inside
        // sp_executesql (tiberius `execute`) a lone BEGIN/COMMIT trips
        // T-SQL's "transaction count" check (error 266). Multi-statement
        // scripts need a batch anyway.
        let upper = sql.trim().to_ascii_uppercase();
        let is_tx_control = ["BEGIN", "COMMIT", "ROLLBACK", "SAVE "]
            .iter()
            .any(|kw| upper.starts_with(kw));
        if params.is_empty() && (is_tx_control || sql.contains(';')) {
            self.rt
                .block_on(async { client.simple_query(&sql).await?.into_results().await })
                .map_err(ms_err)?;
            return Ok(0);
        }
        let n = self
            .rt
            .block_on(client.execute(sql.as_str(), &refs))
            .map_err(ms_err)?
            .total();
        Ok(n as usize)
    }

    pub fn query(&self, sql: &str, params: &[Value]) -> Result<RecordBatch> {
        let sql = translate_placeholders(sql);
        let bound = bind(params);
        let refs: Vec<&dyn tiberius::ToSql> =
            bound.iter().map(|b| b.as_ref() as &dyn tiberius::ToSql).collect();
        let mut client = self.lock()?;
        let results = self
            .rt
            .block_on(async { client.query(sql.as_str(), &refs).await?.into_results().await })
            .map_err(ms_err)?;
        drop(client);
        rows_to_batch(results.first().map(Vec::as_slice).unwrap_or(&[]))
    }

    fn lock(&self) -> Result<std::sync::MutexGuard<'_, TClient>> {
        self.client
            .lock()
            .map_err(|_| Error::Database("mssql connection mutex poisoned".into()))
    }
}

/// `mssql://[user[:pass]@]host[:port][/database][?encrypt=true]`.
fn parse_url(url: &str) -> Result<Config> {
    let rest = url
        .strip_prefix("mssql://")
        .or_else(|| url.strip_prefix("sqlserver://"))
        .ok_or_else(|| Error::InvalidUrl(format!("not an mssql url: {url}")))?;

    let (rest, query) = match rest.split_once('?') {
        Some((r, q)) => (r, Some(q)),
        None => (rest, None),
    };
    let (creds, hostpart) = match rest.rsplit_once('@') {
        Some((c, h)) => (Some(c), h),
        None => (None, rest),
    };
    let (hostport, database) = match hostpart.split_once('/') {
        Some((h, d)) if !d.is_empty() => (h, Some(d)),
        Some((h, _)) => (h, None),
        None => (hostpart, None),
    };
    let (host, port) = match hostport.rsplit_once(':') {
        Some((h, p)) => (
            h,
            p.parse::<u16>()
                .map_err(|_| Error::InvalidUrl(format!("mssql port `{p}`")))?,
        ),
        None => (hostport, 1433),
    };

    let mut config = Config::new();
    config.host(host);
    config.port(port);
    if let Some(db) = database {
        config.database(db);
    }
    match creds {
        Some(c) if !c.is_empty() => {
            let (user, pass) = c.split_once(':').unwrap_or((c, ""));
            config.authentication(AuthMethod::sql_server(user, pass));
        }
        _ => {
            // AuthMethod::Integrated exists only on Windows in tiberius.
            #[cfg(windows)]
            config.authentication(AuthMethod::Integrated);
            #[cfg(not(windows))]
            return Err(Error::InvalidUrl(
                "mssql integrated auth is Windows-only; provide user:pass in the URL".into(),
            ));
        }
    }
    // Older servers (2008) ship without a TLS certificate; encryption is
    // opt-in via `?encrypt=true`.
    if query.is_some_and(|q| q.split('&').any(|kv| kv == "encrypt=true")) {
        config.encryption(EncryptionLevel::Required);
        config.trust_cert();
    } else {
        config.encryption(EncryptionLevel::NotSupported);
    }
    Ok(config)
}

/// SQLite-dialect transaction statements → T-SQL. `None` means "swallow the
/// statement" (T-SQL has no RELEASE; savepoints vanish on commit).
fn normalize_tx(sql: &str) -> Option<String> {
    let t = sql.trim();
    let upper = t.to_ascii_uppercase();
    if upper == "BEGIN" || upper == "BEGIN IMMEDIATE" {
        return Some("BEGIN TRANSACTION".into());
    }
    if let Some(name) = strip_keyword(t, "SAVEPOINT ") {
        return Some(format!("SAVE TRANSACTION {name}"));
    }
    if let Some(rest) = strip_keyword(t, "ROLLBACK TO ") {
        let name = strip_keyword(rest, "SAVEPOINT ").unwrap_or(rest);
        return Some(format!("ROLLBACK TRANSACTION {name}"));
    }
    if let Some(rest) = strip_keyword(t, "RELEASE ") {
        let _ = strip_keyword(rest, "SAVEPOINT ").unwrap_or(rest);
        return None;
    }
    Some(sql.into())
}

fn strip_keyword<'a>(s: &'a str, kw: &str) -> Option<&'a str> {
    if s.len() > kw.len() && s[..kw.len()].eq_ignore_ascii_case(kw) {
        Some(s[kw.len()..].trim_start())
    } else {
        None
    }
}

/// `?` → `@P1..@Pn`, leaving `'…'` / `"…"` / `-- …` / `/* … */` untouched.
/// Same scanner as the Postgres rewrite, different spelling.
fn translate_placeholders(sql: &str) -> String {
    let bytes = sql.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(sql.len() + 8);
    let mut n = 0usize;
    let mut i = 0usize;
    while i < bytes.len() {
        match bytes[i] {
            quote @ (b'\'' | b'"') => {
                out.push(quote);
                i += 1;
                while i < bytes.len() {
                    out.push(bytes[i]);
                    if bytes[i] == quote {
                        if i + 1 < bytes.len() && bytes[i + 1] == quote {
                            out.push(quote);
                            i += 2;
                            continue;
                        }
                        i += 1;
                        break;
                    }
                    i += 1;
                }
            }
            b'-' if i + 1 < bytes.len() && bytes[i + 1] == b'-' => {
                while i < bytes.len() && bytes[i] != b'\n' {
                    out.push(bytes[i]);
                    i += 1;
                }
            }
            b'/' if i + 1 < bytes.len() && bytes[i + 1] == b'*' => {
                out.extend_from_slice(b"/*");
                i += 2;
                while i < bytes.len() {
                    if bytes[i] == b'*' && i + 1 < bytes.len() && bytes[i + 1] == b'/' {
                        out.extend_from_slice(b"*/");
                        i += 2;
                        break;
                    }
                    out.push(bytes[i]);
                    i += 1;
                }
            }
            b'?' => {
                n += 1;
                out.push(b'@');
                out.push(b'P');
                out.extend_from_slice(n.to_string().as_bytes());
                i += 1;
            }
            c => {
                out.push(c);
                i += 1;
            }
        }
    }
    String::from_utf8(out).expect("only ASCII inserted into valid UTF-8")
}

fn bind(params: &[Value]) -> Vec<Box<dyn tiberius::ToSql>> {
    params
        .iter()
        .map(|v| -> Box<dyn tiberius::ToSql> {
            match v {
                // T-SQL implicitly converts an nvarchar NULL to any slot.
                Value::Null => Box::new(Option::<String>::None),
                Value::Int(i) => Box::new(*i),
                Value::Float(f) => Box::new(*f),
                Value::Text(s) => Box::new(s.clone()),
                Value::Bool(b) => Box::new(*b),
            }
        })
        .collect()
}

/// Map an MSSQL column type onto one of the four PCB types.
fn pcb_type(ty: ColumnType, name: &str) -> Result<DataType> {
    use ColumnType::*;
    Ok(match ty {
        Int1 | Int2 | Int4 | Int8 | Intn => DataType::Int64,
        Float4 | Float8 | Floatn => DataType::Float64,
        Bit | Bitn => DataType::Bool,
        BigVarChar | BigChar | NVarchar | NChar | Text | NText => DataType::Utf8,
        other => {
            return Err(Error::Unsupported(format!(
                "mssql column `{name}` has type {other:?}; cast it in SQL (e.g. CAST(col AS NVARCHAR(...)))"
            )))
        }
    })
}

fn rows_to_batch(rows: &[tiberius::Row]) -> Result<RecordBatch> {
    if rows.is_empty() {
        return RecordBatch::try_new(vec![]);
    }
    let cols = rows[0].columns().to_vec();
    let mut out = Vec::with_capacity(cols.len());
    for (ci, col) in cols.iter().enumerate() {
        let name = col.name().to_string();
        let dt = pcb_type(col.column_type(), &name)?;
        let mut b = ColumnBuilder::new(dt);
        for r in rows {
            push_cell(&mut b, r, ci, dt, &name)?;
        }
        out.push(b.finish(name));
    }
    RecordBatch::try_new(out)
}

fn push_cell(
    b: &mut ColumnBuilder,
    row: &tiberius::Row,
    ci: usize,
    dt: DataType,
    name: &str,
) -> Result<()> {
    let cell = row
        .cells()
        .nth(ci)
        .map(|(_, data)| data)
        .ok_or_else(|| Error::Database(format!("column `{name}`: missing cell")))?;
    use ColumnData::*;
    match (dt, cell) {
        (DataType::Int64, U8(v)) => push_opt_i64(b, v.map(|x| x as i64)),
        (DataType::Int64, I16(v)) => push_opt_i64(b, v.map(|x| x as i64)),
        (DataType::Int64, I32(v)) => push_opt_i64(b, v.map(|x| x as i64)),
        (DataType::Int64, I64(v)) => push_opt_i64(b, *v),
        (DataType::Float64, F32(v)) => push_opt_f64(b, v.map(|x| x as f64)),
        (DataType::Float64, F64(v)) => push_opt_f64(b, *v),
        (DataType::Bool, Bit(v)) => match v {
            Some(x) => b.push_bool(*x).map(|_| ()),
            None => {
                b.push_null();
                Ok(())
            }
        },
        (DataType::Utf8, String(v)) => match v {
            Some(s) => b.push_str(s.as_ref()),
            None => {
                b.push_null();
                Ok(())
            }
        },
        (_, other) => Err(Error::Database(format!(
            "column `{name}`: unexpected value {other:?} for {dt:?}"
        ))),
    }
}

fn push_opt_i64(b: &mut ColumnBuilder, v: Option<i64>) -> Result<()> {
    match v {
        Some(x) => b.push_i64(x).map(|_| ()),
        None => {
            b.push_null();
            Ok(())
        }
    }
}

fn push_opt_f64(b: &mut ColumnBuilder, v: Option<f64>) -> Result<()> {
    match v {
        Some(x) => b.push_f64(x).map(|_| ()),
        None => {
            b.push_null();
            Ok(())
        }
    }
}

fn ms_err(e: tiberius::error::Error) -> Error {
    Error::Database(format!("mssql: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn placeholders_become_at_pn() {
        assert_eq!(
            translate_placeholders("SELECT * FROM t WHERE a = ? AND b IN (?, ?)"),
            "SELECT * FROM t WHERE a = @P1 AND b IN (@P2, @P3)"
        );
        assert_eq!(
            translate_placeholders("SELECT '?' WHERE y = ? -- t?\n AND z = ?"),
            "SELECT '?' WHERE y = @P1 -- t?\n AND z = @P2"
        );
    }

    #[test]
    fn tx_statements_normalize() {
        assert_eq!(normalize_tx("BEGIN IMMEDIATE").as_deref(), Some("BEGIN TRANSACTION"));
        assert_eq!(normalize_tx("BEGIN").as_deref(), Some("BEGIN TRANSACTION"));
        assert_eq!(normalize_tx("SAVEPOINT sp_1").as_deref(), Some("SAVE TRANSACTION sp_1"));
        assert_eq!(
            normalize_tx("ROLLBACK TO sp_1").as_deref(),
            Some("ROLLBACK TRANSACTION sp_1")
        );
        assert_eq!(
            normalize_tx("ROLLBACK TO SAVEPOINT sp_1").as_deref(),
            Some("ROLLBACK TRANSACTION sp_1")
        );
        assert_eq!(normalize_tx("RELEASE sp_1"), None);
        assert_eq!(normalize_tx("RELEASE SAVEPOINT sp_1"), None);
        assert_eq!(normalize_tx("COMMIT").as_deref(), Some("COMMIT"));
        assert_eq!(normalize_tx("ROLLBACK").as_deref(), Some("ROLLBACK"));
        assert_eq!(
            normalize_tx("DELETE FROM t WHERE id = ?").as_deref(),
            Some("DELETE FROM t WHERE id = ?")
        );
    }

    #[test]
    fn url_forms_parse() {
        assert!(parse_url("mssql://127.0.0.1/master").is_ok());
        assert!(parse_url("mssql://sa:pw@db.host:1444/app").is_ok());
        assert!(parse_url("sqlserver://127.0.0.1").is_ok());
        assert!(parse_url("postgres://x").is_err());
    }
}
