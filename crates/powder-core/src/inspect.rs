//! Blocking engine handle + live-database introspection, shared by the CLI
//! and the studio dashboard. One `Engine` = one connection, so it stays
//! correct on `sqlite::memory:` (a second connection would see a different
//! database). All queries run through [`crate::Client`] — every backend and
//! the SQL-injection guard apply unchanged.

use std::fmt::Write as _;
use std::path::Path;

use serde_json::{Map, Value as J};

/// One live column, as reported by the backend's catalog.
#[derive(Debug, Clone, PartialEq)]
pub struct DbColumn {
    pub name: String,
    pub sql_type: String,
    pub notnull: bool,
    /// 0 = not part of the primary key; otherwise the 1-based position
    /// within a (possibly composite) primary key.
    pub pk: i64,
}

/// One live foreign key, composite columns grouped in declaration order.
#[derive(Debug, Clone, PartialEq)]
pub struct DbForeignKey {
    pub from: Vec<String>,
    pub table: String,
    pub to: Vec<String>,
}

/// Anchor relative SQLite paths at `cwd`, pass every server URL through.
fn resolve_url(url: &str, cwd: &Path) -> String {
    if url == ":memory:" || url == "sqlite::memory:" {
        return url.into();
    }
    const SCHEMES: [&str; 7] = [
        "postgres://",
        "postgresql://",
        "mysql://",
        "mariadb://",
        "mssql://",
        "sqlserver://",
        "libsql://",
    ];
    if SCHEMES.iter().any(|s| url.starts_with(s)) {
        return url.into();
    }
    let path = url
        .strip_prefix("sqlite://")
        .or_else(|| url.strip_prefix("sqlite:"))
        .unwrap_or(url);
    let p = Path::new(path);
    if p.is_absolute() {
        path.into()
    } else {
        cwd.join(p).to_string_lossy().into_owned()
    }
}

/// A blocking handle over the async engine client.
pub struct Engine {
    rt: tokio::runtime::Runtime,
    client: crate::Client,
}

impl Engine {
    pub fn connect(url: &str, cwd: &Path) -> Result<Self, String> {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|e| e.to_string())?;
        let resolved = resolve_url(url, cwd);
        let client = rt
            .block_on(crate::Client::connect(&resolved))
            .map_err(|e| e.to_string())?;
        Ok(Self { rt, client })
    }

    pub fn query(&self, sql: &str) -> Result<crate::RecordBatch, String> {
        self.query_params(sql, vec![])
    }

    pub fn execute(&self, sql: &str) -> Result<usize, String> {
        self.execute_params(sql, vec![])
    }

    pub fn query_params(
        &self,
        sql: &str,
        params: Vec<crate::Value>,
    ) -> Result<crate::RecordBatch, String> {
        self.rt
            .block_on(self.client.query(sql, params))
            .map_err(|e| e.to_string())
    }

    pub fn execute_params(
        &self,
        sql: &str,
        params: Vec<crate::Value>,
    ) -> Result<usize, String> {
        self.rt
            .block_on(self.client.execute(sql, params))
            .map_err(|e| e.to_string())
    }

    /// The connected backend's SQL flavor, as a lowercase name.
    pub fn flavor(&self) -> &'static str {
        use crate::client::SqlFlavor::*;
        match self.client.flavor() {
            Sqlite => "sqlite",
            Postgres => "postgres",
            MySql => "mysql",
            MsSql => "mssql",
            LibSql => "libsql",
        }
    }
}
// ---------------------------------------------------------------------------
// Engine-based introspection — one connection for everything, so it works on
// `sqlite::memory:` too (a second connection would see a different database).
// ---------------------------------------------------------------------------

fn s(batch: &crate::RecordBatch, col: &str, row: usize) -> String {
    batch.column(col).and_then(|c| c.str(row)).unwrap_or("").to_string()
}
fn n(batch: &crate::RecordBatch, col: &str, row: usize) -> i64 {
    batch.column(col).and_then(|c| c.i64(row)).unwrap_or(0)
}

pub fn table_names(e: &Engine) -> Result<Vec<String>, String> {
    let sql = match e.flavor() {
        "sqlite" | "libsql" => {
            "SELECT name FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%' ORDER BY name"
        }
        "postgres" => {
            "SELECT table_name AS name FROM information_schema.tables
             WHERE table_schema = current_schema() AND table_type = 'BASE TABLE' ORDER BY table_name"
        }
        "mysql" => {
            "SELECT TABLE_NAME AS name FROM information_schema.TABLES
             WHERE TABLE_SCHEMA = DATABASE() AND TABLE_TYPE = 'BASE TABLE' ORDER BY TABLE_NAME"
        }
        _ => {
            "SELECT TABLE_NAME AS name FROM INFORMATION_SCHEMA.TABLES
             WHERE TABLE_TYPE = 'BASE TABLE' ORDER BY TABLE_NAME"
        }
    };
    let batch = e.query(sql)?;
    Ok((0..batch.num_rows).map(|r| s(&batch, "name", r)).collect())
}

pub fn table_columns(e: &Engine, table: &str) -> Result<Vec<DbColumn>, String> {
    if !table.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
        return Err(format!("invalid table name `{table}`"));
    }
    let t = crate::Value::Text(table.into());
    match e.flavor() {
        "sqlite" | "libsql" => {
            let batch = e.query(&format!("PRAGMA table_info({table})"))?;
            Ok((0..batch.num_rows)
                .map(|r| DbColumn {
                    name: s(&batch, "name", r),
                    sql_type: s(&batch, "type", r).to_ascii_uppercase(),
                    notnull: n(&batch, "notnull", r) != 0,
                    pk: n(&batch, "pk", r),
                })
                .collect())
        }
        "postgres" => {
            let cols = e.query_params(
                "SELECT column_name, data_type, is_nullable FROM information_schema.columns
                 WHERE table_name = ? AND table_schema = current_schema() ORDER BY ordinal_position",
                vec![t.clone()],
            )?;
            let pks = e.query_params(
                "SELECT a.attname AS name, k.ord::bigint AS ord
                 FROM pg_constraint con
                 JOIN pg_class rel ON rel.oid = con.conrelid
                 CROSS JOIN LATERAL unnest(con.conkey) WITH ORDINALITY k(attnum, ord)
                 JOIN pg_attribute a ON a.attrelid = con.conrelid AND a.attnum = k.attnum
                 WHERE con.contype = 'p' AND rel.relname = ?",
                vec![t],
            )?;
            let pk_of = |name: &str| {
                (0..pks.num_rows)
                    .find(|&r| s(&pks, "name", r) == name)
                    .map(|r| n(&pks, "ord", r))
                    .unwrap_or(0)
            };
            Ok((0..cols.num_rows)
                .map(|r| {
                    let name = s(&cols, "column_name", r);
                    DbColumn {
                        sql_type: s(&cols, "data_type", r).to_ascii_uppercase(),
                        notnull: s(&cols, "is_nullable", r) == "NO",
                        pk: pk_of(&name),
                        name,
                    }
                })
                .collect())
        }
        "mysql" => {
            let cols = e.query_params(
                "SELECT COLUMN_NAME, DATA_TYPE, IS_NULLABLE FROM information_schema.COLUMNS
                 WHERE TABLE_NAME = ? AND TABLE_SCHEMA = DATABASE() ORDER BY ORDINAL_POSITION",
                vec![t.clone()],
            )?;
            let pks = e.query_params(
                "SELECT COLUMN_NAME AS name, CAST(ORDINAL_POSITION AS SIGNED) AS ord
                 FROM information_schema.KEY_COLUMN_USAGE
                 WHERE TABLE_NAME = ? AND TABLE_SCHEMA = DATABASE() AND CONSTRAINT_NAME = 'PRIMARY'",
                vec![t],
            )?;
            let pk_of = |name: &str| {
                (0..pks.num_rows)
                    .find(|&r| s(&pks, "name", r) == name)
                    .map(|r| n(&pks, "ord", r))
                    .unwrap_or(0)
            };
            Ok((0..cols.num_rows)
                .map(|r| {
                    let name = s(&cols, "COLUMN_NAME", r);
                    DbColumn {
                        sql_type: my_normalize(&s(&cols, "DATA_TYPE", r)),
                        notnull: s(&cols, "IS_NULLABLE", r) == "NO",
                        pk: pk_of(&name),
                        name,
                    }
                })
                .collect())
        }
        _ => {
            let cols = e.query_params(
                "SELECT COLUMN_NAME, DATA_TYPE, IS_NULLABLE FROM INFORMATION_SCHEMA.COLUMNS
                 WHERE TABLE_NAME = ? ORDER BY ORDINAL_POSITION",
                vec![t.clone()],
            )?;
            let pks = e.query_params(
                "SELECT col.name AS name, CAST(ic.key_ordinal AS BIGINT) AS ord
                 FROM sys.indexes i
                 JOIN sys.index_columns ic ON ic.object_id = i.object_id AND ic.index_id = i.index_id
                 JOIN sys.columns col ON col.object_id = ic.object_id AND col.column_id = ic.column_id
                 WHERE i.is_primary_key = 1 AND i.object_id = OBJECT_ID(?)",
                vec![t],
            )?;
            let pk_of = |name: &str| {
                (0..pks.num_rows)
                    .find(|&r| s(&pks, "name", r) == name)
                    .map(|r| n(&pks, "ord", r))
                    .unwrap_or(0)
            };
            Ok((0..cols.num_rows)
                .map(|r| {
                    let name = s(&cols, "COLUMN_NAME", r);
                    DbColumn {
                        sql_type: ms_normalize(&s(&cols, "DATA_TYPE", r)),
                        notnull: s(&cols, "IS_NULLABLE", r) == "NO",
                        pk: pk_of(&name),
                        name,
                    }
                })
                .collect())
        }
    }
}

fn my_normalize(data_type: &str) -> String {
    match data_type.to_ascii_lowercase().as_str() {
        "tinyint" => "TINYINT(1)".into(),
        "varchar" | "char" => "TEXT".into(),
        other => other.to_ascii_uppercase(),
    }
}
fn ms_normalize(data_type: &str) -> String {
    match data_type.to_ascii_lowercase().as_str() {
        "nvarchar" | "nchar" | "varchar" | "char" => "NVARCHAR(400)".into(),
        other => other.to_ascii_uppercase(),
    }
}

pub fn table_fks(e: &Engine, table: &str) -> Result<Vec<DbForeignKey>, String> {
    if !table.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
        return Err(format!("invalid table name `{table}`"));
    }
    let t = crate::Value::Text(table.into());
    // Uniform shape: one row per FK column — (fk id/name, from, ref table, to).
    let (batch, id_col, from_col, table_col, to_col): (_, &str, &str, &str, &str) = match e.flavor() {
        "sqlite" | "libsql" => (
            e.query(&format!("PRAGMA foreign_key_list({table})"))?,
            "id",
            "from",
            "table",
            "to",
        ),
        "postgres" => (
            e.query_params(
                "SELECT con.conname AS fk, att.attname AS from_col, confrel.relname AS ref_table,
                        att2.attname AS to_col
                 FROM pg_constraint con
                 JOIN pg_class rel ON rel.oid = con.conrelid
                 JOIN pg_class confrel ON confrel.oid = con.confrelid
                 CROSS JOIN LATERAL unnest(con.conkey, con.confkey) WITH ORDINALITY k(attnum, fattnum, ord)
                 JOIN pg_attribute att ON att.attrelid = con.conrelid AND att.attnum = k.attnum
                 JOIN pg_attribute att2 ON att2.attrelid = con.confrelid AND att2.attnum = k.fattnum
                 WHERE con.contype = 'f' AND rel.relname = ?
                 ORDER BY con.conname, k.ord",
                vec![t],
            )?,
            "fk",
            "from_col",
            "ref_table",
            "to_col",
        ),
        "mysql" => (
            e.query_params(
                "SELECT CONSTRAINT_NAME AS fk, COLUMN_NAME AS from_col,
                        REFERENCED_TABLE_NAME AS ref_table, REFERENCED_COLUMN_NAME AS to_col
                 FROM information_schema.KEY_COLUMN_USAGE
                 WHERE TABLE_NAME = ? AND TABLE_SCHEMA = DATABASE()
                   AND REFERENCED_TABLE_NAME IS NOT NULL
                 ORDER BY CONSTRAINT_NAME, ORDINAL_POSITION",
                vec![t],
            )?,
            "fk",
            "from_col",
            "ref_table",
            "to_col",
        ),
        _ => (
            e.query_params(
                "SELECT fk.name AS fk, pc.name AS from_col, rt.name AS ref_table, rc.name AS to_col
                 FROM sys.foreign_keys fk
                 JOIN sys.foreign_key_columns fkc ON fkc.constraint_object_id = fk.object_id
                 JOIN sys.columns pc ON pc.object_id = fkc.parent_object_id AND pc.column_id = fkc.parent_column_id
                 JOIN sys.tables rt ON rt.object_id = fk.referenced_object_id
                 JOIN sys.columns rc ON rc.object_id = fkc.referenced_object_id AND rc.column_id = fkc.referenced_column_id
                 WHERE fk.parent_object_id = OBJECT_ID(?)
                 ORDER BY fk.name, fkc.constraint_column_id",
                vec![t],
            )?,
            "fk",
            "from_col",
            "ref_table",
            "to_col",
        ),
    };
    let key_of = |r: usize| -> String {
        // SQLite keys by integer id; the rest by constraint name.
        let by_name = s(&batch, id_col, r);
        if by_name.is_empty() {
            n(&batch, id_col, r).to_string()
        } else {
            by_name
        }
    };
    let mut out: Vec<DbForeignKey> = Vec::new();
    let mut cur: Option<String> = None;
    for r in 0..batch.num_rows {
        let k = key_of(r);
        if cur.as_deref() != Some(k.as_str()) {
            cur = Some(k);
            out.push(DbForeignKey {
                from: Vec::new(),
                table: s(&batch, table_col, r),
                to: Vec::new(),
            });
        }
        let fk = out.last_mut().unwrap();
        fk.from.push(s(&batch, from_col, r));
        fk.to.push(s(&batch, to_col, r));
    }
    Ok(out)
}

/// One-line-per-table schema summary (AI prompt context).
pub fn schema_summary(e: &Engine) -> Result<String, String> {
    let mut out = String::new();
    for t in table_names(e)? {
        let cols = table_columns(e, &t)?;
        let cols_s: Vec<String> = cols
            .iter()
            .map(|c| {
                let mut s = format!("{} {}", c.name, c.sql_type);
                if c.pk > 0 {
                    s.push_str(" PK");
                }
                if c.notnull {
                    s.push_str(" NOT NULL");
                }
                s
            })
            .collect();
        let _ = writeln!(out, "- {t}({})", cols_s.join(", "));
        for fk in table_fks(e, &t)? {
            let _ = writeln!(out, "  FK ({}) -> {}({})", fk.from.join(","), fk.table, fk.to.join(","));
        }
    }
    Ok(out)
}

/// JSON value → engine bind value (used by studio row edits).
pub fn json_to_value(v: &J) -> Result<crate::Value, String> {
    use crate::Value;
    match v {
        J::Null => Ok(Value::Null),
        J::Bool(b) => Ok(Value::Bool(*b)),
        J::Number(n) => Ok(if let Some(i) = n.as_i64() {
            Value::Int(i)
        } else {
            Value::Float(n.as_f64().unwrap_or(0.0))
        }),
        J::String(s) => Ok(Value::Text(s.clone())),
        other => Err(format!("unsupported value: {other}")),
    }
}

/// Decode a batch into JSON rows (column storage type, NULL-aware).
pub fn batch_rows(batch: &crate::RecordBatch) -> Vec<Map<String, J>> {
    let mut out = Vec::with_capacity(batch.num_rows);
    for r in 0..batch.num_rows {
        let mut row = Map::new();
        for col in &batch.columns {
            let v = if !col.is_valid(r) {
                J::Null
            } else if let Some(i) = col.i64(r) {
                J::Number(i.into())
            } else if let Some(f) = col.f64(r) {
                serde_json::Number::from_f64(f).map(J::Number).unwrap_or(J::Null)
            } else if let Some(b) = col.bool(r) {
                J::Bool(b)
            } else if let Some(s) = col.str(r) {
                J::String(s.into())
            } else {
                J::Null
            };
            row.insert(col.field.name.clone(), v);
        }
        out.push(row);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn url_resolution_anchors_relative_paths() {
        let cwd = Path::new("C:/work");
        assert_eq!(resolve_url("sqlite::memory:", cwd), "sqlite::memory:");
        assert_eq!(resolve_url("postgres://u@h/db", cwd), "postgres://u@h/db");
        assert!(resolve_url("app.db", cwd).ends_with("app.db"));
        assert!(resolve_url("sqlite://data/app.db", cwd).contains("work"));
    }
}
