//! The async database client — connection management and query execution.
//!
//! The backend (SQLite via `rusqlite`) is synchronous, so blocking calls are
//! dispatched to Tokio's blocking pool. Every public method is `async`, which
//! is what lets the language bindings map a Rust `Future` cleanly onto a JS
//! `Promise` (napi) and a Python `asyncio` awaitable (PyO3).

use std::sync::{Arc, Mutex};

use rusqlite::types::Value as SqlValue;
use rusqlite::Connection;

use crate::array::ColumnBuilder;
use crate::batch::RecordBatch;
use crate::error::{Error, Result};
use crate::query::Value;
use crate::schema::DataType;

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
    /// intermediate materialization on the host-language side.
    pub async fn query_bytes(&self, sql: &str, params: Vec<Value>) -> Result<Vec<u8>> {
        Ok(self.query(sql, params).await?.encode())
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
    let mut stmt = conn.prepare(sql)?;
    let ncols = stmt.column_count();
    let names: Vec<String> = (0..ncols)
        .map(|i| stmt.column_name(i).map(str::to_string))
        .collect::<std::result::Result<_, _>>()?;

    // Materialize every cell as a dynamically-typed SQL value first; SQLite is
    // dynamically typed, so column types are inferred from the actual data.
    let mut rows = stmt.query(rusqlite::params_from_iter(sql_params.iter()))?;
    let mut cells: Vec<Vec<SqlValue>> = Vec::new();
    while let Some(row) = rows.next()? {
        let mut r = Vec::with_capacity(ncols);
        for i in 0..ncols {
            r.push(row.get::<usize, SqlValue>(i)?);
        }
        cells.push(r);
    }

    let mut columns = Vec::with_capacity(ncols);
    for i in 0..ncols {
        let dtype = infer_type(&cells, i);
        let mut builder = ColumnBuilder::new(dtype);
        for row in &cells {
            append_cell(&mut builder, dtype, &row[i])?;
        }
        columns.push(builder.finish(names[i].clone()));
    }

    RecordBatch::try_new(columns)
}

/// Infer a column's NCB type from the SQL values present in it.
///
/// Rule: any text/blob -> Utf8; else any real -> Float64; else Int64 (which
/// also covers all-NULL columns).
fn infer_type(cells: &[Vec<SqlValue>], col: usize) -> DataType {
    let mut has_real = false;
    for row in cells {
        match &row[col] {
            SqlValue::Text(_) | SqlValue::Blob(_) => return DataType::Utf8,
            SqlValue::Real(_) => has_real = true,
            _ => {}
        }
    }
    if has_real {
        DataType::Float64
    } else {
        DataType::Int64
    }
}

fn append_cell(builder: &mut ColumnBuilder, dtype: DataType, value: &SqlValue) -> Result<()> {
    match dtype {
        DataType::Int64 => match value {
            SqlValue::Null => builder.push_null(),
            SqlValue::Integer(i) => builder.push_i64(*i)?,
            SqlValue::Real(f) => builder.push_i64(*f as i64)?,
            _ => builder.push_null(),
        },
        DataType::Float64 => match value {
            SqlValue::Null => builder.push_null(),
            SqlValue::Integer(i) => builder.push_f64(*i as f64)?,
            SqlValue::Real(f) => builder.push_f64(*f)?,
            _ => builder.push_null(),
        },
        DataType::Utf8 => match value {
            SqlValue::Null => builder.push_null(),
            SqlValue::Text(s) => builder.push_str(s)?,
            SqlValue::Integer(i) => builder.push_str(&i.to_string())?,
            SqlValue::Real(f) => builder.push_str(&f.to_string())?,
            SqlValue::Blob(b) => builder.push_str(&String::from_utf8_lossy(b))?,
        },
        // The DB inference layer never produces Bool; it is reachable only via
        // hand-built batches, which use the builder API directly.
        DataType::Bool => builder.push_null(),
    }
    Ok(())
}
