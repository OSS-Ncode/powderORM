//! `powder.schema.json` — the single source of truth for Powder ORM.
//!
//! The same file drives migration DDL, live-database validation, and the AOT
//! code generators for TypeScript and Python, which is what keeps every
//! language binding's model layer in lockstep with the database.

use serde::Deserialize;
use serde_json::Map;

/// Logical column types. Deliberately the PCB type set: what the wire format
/// can carry zero-copy is exactly what a model may declare.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ColumnType {
    Int,
    Float,
    Text,
    Bool,
}

impl ColumnType {
    /// SQLite storage type used in DDL and expected by `validate`.
    pub fn sql_type(self) -> &'static str {
        match self {
            ColumnType::Int | ColumnType::Bool => "INTEGER",
            ColumnType::Float => "REAL",
            ColumnType::Text => "TEXT",
        }
    }

    pub fn ts_type(self) -> &'static str {
        match self {
            ColumnType::Int | ColumnType::Float => "number",
            ColumnType::Text => "string",
            ColumnType::Bool => "boolean",
        }
    }

    pub fn py_type(self) -> &'static str {
        match self {
            ColumnType::Int => "int",
            ColumnType::Float => "float",
            ColumnType::Text => "str",
            ColumnType::Bool => "bool",
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            ColumnType::Int => "int",
            ColumnType::Float => "float",
            ColumnType::Text => "text",
            ColumnType::Bool => "bool",
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Reference {
    pub table: String,
    pub column: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ColumnDef {
    #[serde(rename = "type")]
    pub column_type: ColumnType,
    #[serde(default)]
    pub nullable: bool,
    #[serde(default, rename = "primaryKey")]
    pub primary_key: bool,
    /// Foreign key: this column references `table.column`.
    #[serde(default)]
    pub references: Option<Reference>,
}

#[derive(Debug, Clone)]
pub struct Column {
    pub name: String,
    pub def: ColumnDef,
}

/// A normalized foreign key. Single-column `references` sugar and table-level
/// `foreignKeys` (which may span multiple columns) both reduce to this.
#[derive(Debug, Clone)]
pub struct ForeignKey {
    /// Local columns, in order.
    pub columns: Vec<String>,
    pub ref_table: String,
    /// Referenced columns, same arity/order as `columns`.
    pub ref_columns: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct Table {
    pub name: String,
    pub columns: Vec<Column>,
    /// All foreign keys (per-column + table-level), normalized.
    pub foreign_keys: Vec<ForeignKey>,
}

/// A custom named query declared in the schema. The `:name` placeholders are
/// compiled to positional `?` at parse time (AOT); `param_order` records which
/// declared parameter feeds each positional slot, in order (a parameter may
/// appear more than once).
#[derive(Debug, Clone)]
pub struct NamedQuery {
    pub name: String,
    /// Original SQL with `:name` placeholders (for docs/comments).
    pub source_sql: String,
    /// Compiled SQL with positional `?` placeholders.
    pub sql: String,
    /// Declared parameters `(name, type)` in declaration order.
    pub params: Vec<(String, ColumnType)>,
    /// Parameter name per positional `?`, in order of appearance.
    pub param_order: Vec<String>,
    /// Optional table whose row shape the query returns (typed rows).
    pub returns: Option<String>,
}

#[derive(Debug, Clone)]
pub struct Schema {
    pub tables: Vec<Table>,
    pub queries: Vec<NamedQuery>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawSchema {
    /// Editor JSON-Schema pointer (`powder generate` ignores it).
    #[serde(default, rename = "$schema")]
    #[allow(dead_code)]
    schema_ref: Option<String>,
    #[serde(default)]
    #[allow(dead_code)]
    database: Option<String>,
    tables: Map<String, serde_json::Value>,
    #[serde(default)]
    queries: Map<String, serde_json::Value>,
}

/// Compile `:name` placeholders to positional `?`, recording the parameter
/// order. Skips single-quoted string literals and the `::` operator; every
/// placeholder must be declared and every declared parameter must be used.
fn compile_named_sql(
    qname: &str,
    sql: &str,
    declared: &[(String, ColumnType)],
) -> Result<(String, Vec<String>), String> {
    let bytes = sql.as_bytes();
    // Byte-wise copy keeps multi-byte UTF-8 intact; only ASCII `?` is inserted.
    let mut out: Vec<u8> = Vec::with_capacity(sql.len());
    let mut order: Vec<String> = Vec::new();
    let mut i = 0;
    let mut in_str = false;
    while i < bytes.len() {
        let c = bytes[i];
        if in_str {
            out.push(c);
            if c == b'\'' {
                in_str = false; // '' escapes toggle out/in and stay literal
            }
            i += 1;
            continue;
        }
        match c {
            b'\'' => {
                in_str = true;
                out.push(c);
                i += 1;
            }
            b':' if i + 1 < bytes.len() && bytes[i + 1] == b':' => {
                out.extend_from_slice(b"::"); // cast operator, not a placeholder
                i += 2;
            }
            b':' if i + 1 < bytes.len()
                && (bytes[i + 1].is_ascii_alphabetic() || bytes[i + 1] == b'_') =>
            {
                let start = i + 1;
                let mut end = start;
                while end < bytes.len()
                    && (bytes[end].is_ascii_alphanumeric() || bytes[end] == b'_')
                {
                    end += 1;
                }
                let name = &sql[start..end];
                if !declared.iter().any(|(p, _)| p == name) {
                    return Err(format!(
                        "query `{qname}`: placeholder `:{name}` is not declared in params"
                    ));
                }
                out.push(b'?');
                order.push(name.to_string());
                i = end;
            }
            _ => {
                out.push(c);
                i += 1;
            }
        }
    }
    for (p, _) in declared {
        if !order.iter().any(|o| o == p) {
            return Err(format!("query `{qname}`: declared param `{p}` is never used"));
        }
    }
    let out = String::from_utf8(out).expect("verbatim copy of valid UTF-8 stays valid");
    Ok((out, order))
}

/// A simple identifier: what Powder allows for table/column names. Everything
/// generated (SQL, TS, Python) interpolates these bare, so the gate is strict.
fn is_ident(s: &str) -> bool {
    let mut chars = s.chars();
    chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

impl Schema {
    /// Parse and structurally validate a `powder.schema.json` document.
    pub fn parse(json: &str) -> Result<Schema, String> {
        let raw: RawSchema =
            serde_json::from_str(json).map_err(|e| format!("invalid schema JSON: {e}"))?;
        let mut tables = Vec::with_capacity(raw.tables.len());
        for (tname, tval) in raw.tables {
            if !is_ident(&tname) {
                return Err(format!("table `{tname}`: not a valid identifier"));
            }
            #[derive(Deserialize)]
            #[serde(deny_unknown_fields)]
            struct RawCompRef {
                table: String,
                columns: Vec<String>,
            }
            #[derive(Deserialize)]
            #[serde(deny_unknown_fields)]
            struct RawForeignKey {
                columns: Vec<String>,
                references: RawCompRef,
            }
            #[derive(Deserialize)]
            #[serde(deny_unknown_fields)]
            struct RawTable {
                columns: Map<String, serde_json::Value>,
                #[serde(default, rename = "foreignKeys")]
                foreign_keys: Vec<RawForeignKey>,
            }
            let rt: RawTable = serde_json::from_value(tval)
                .map_err(|e| format!("table `{tname}`: {e}"))?;
            if rt.columns.is_empty() {
                return Err(format!("table `{tname}`: has no columns"));
            }
            let mut columns = Vec::with_capacity(rt.columns.len());
            let mut foreign_keys = Vec::new();
            for (cname, cval) in rt.columns {
                if !is_ident(&cname) {
                    return Err(format!("table `{tname}`: column `{cname}` is not a valid identifier"));
                }
                let def: ColumnDef = serde_json::from_value(cval)
                    .map_err(|e| format!("table `{tname}`, column `{cname}`: {e}"))?;
                if def.primary_key && def.nullable {
                    return Err(format!(
                        "table `{tname}`, column `{cname}`: a primary key cannot be nullable"
                    ));
                }
                if let Some(r) = &def.references {
                    if !is_ident(&r.table) || !is_ident(&r.column) {
                        return Err(format!(
                            "table `{tname}`, column `{cname}`: reference target is not a valid identifier"
                        ));
                    }
                    foreign_keys.push(ForeignKey {
                        columns: vec![cname.clone()],
                        ref_table: r.table.clone(),
                        ref_columns: vec![r.column.clone()],
                    });
                }
                columns.push(Column { name: cname, def });
            }
            // Table-level (possibly composite) foreign keys.
            for fk in rt.foreign_keys {
                if fk.columns.is_empty() || fk.columns.len() != fk.references.columns.len() {
                    return Err(format!(
                        "table `{tname}`: foreignKeys entry must list an equal, non-zero number of local and referenced columns"
                    ));
                }
                if !is_ident(&fk.references.table)
                    || fk.columns.iter().chain(&fk.references.columns).any(|c| !is_ident(c))
                {
                    return Err(format!(
                        "table `{tname}`: foreignKeys entry has an invalid identifier"
                    ));
                }
                foreign_keys.push(ForeignKey {
                    columns: fk.columns,
                    ref_table: fk.references.table,
                    ref_columns: fk.references.columns,
                });
            }
            tables.push(Table {
                name: tname,
                columns,
                foreign_keys,
            });
        }
        if tables.is_empty() {
            return Err("schema has no tables".into());
        }

        // Custom named queries: `:name` placeholders compile to `?` up front.
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct RawQuery {
            sql: String,
            #[serde(default)]
            params: Map<String, serde_json::Value>,
            #[serde(default)]
            returns: Option<String>,
        }
        let mut queries = Vec::with_capacity(raw.queries.len());
        for (qname, qval) in raw.queries {
            if !is_ident(&qname) {
                return Err(format!("query `{qname}`: not a valid identifier"));
            }
            let rq: RawQuery = serde_json::from_value(qval)
                .map_err(|e| format!("query `{qname}`: {e}"))?;
            let mut params: Vec<(String, ColumnType)> = Vec::with_capacity(rq.params.len());
            for (pname, pval) in rq.params {
                if !is_ident(&pname) {
                    return Err(format!("query `{qname}`: param `{pname}` is not a valid identifier"));
                }
                let ptype: ColumnType = serde_json::from_value(pval)
                    .map_err(|e| format!("query `{qname}`, param `{pname}`: {e}"))?;
                params.push((pname, ptype));
            }
            if let Some(ret) = &rq.returns {
                if !tables.iter().any(|t| &t.name == ret) {
                    return Err(format!(
                        "query `{qname}`: returns unknown table `{ret}`"
                    ));
                }
            }
            let (sql, param_order) = compile_named_sql(&qname, &rq.sql, &params)?;
            queries.push(NamedQuery {
                name: qname,
                source_sql: rq.sql,
                sql,
                params,
                param_order,
                returns: rq.returns,
            });
        }

        let schema = Schema { tables, queries };
        schema.check_references()?;
        Ok(schema)
    }

    /// Every foreign key (single or composite) must point at an existing
    /// table and columns, with matching arity and per-position column types.
    fn check_references(&self) -> Result<(), String> {
        for table in &self.tables {
            for fk in &table.foreign_keys {
                let Some(target) = self.tables.iter().find(|t| t.name == fk.ref_table) else {
                    return Err(format!(
                        "table `{}`: foreign key references unknown table `{}`",
                        table.name, fk.ref_table
                    ));
                };
                for (local, refc) in fk.columns.iter().zip(&fk.ref_columns) {
                    let Some(lcol) = table.columns.iter().find(|c| &c.name == local) else {
                        return Err(format!(
                            "table `{}`: foreign key uses unknown local column `{}`",
                            table.name, local
                        ));
                    };
                    let Some(tcol) = target.columns.iter().find(|c| &c.name == refc) else {
                        return Err(format!(
                            "table `{}`: foreign key references unknown column `{}.{}`",
                            table.name, fk.ref_table, refc
                        ));
                    };
                    if tcol.def.column_type != lcol.def.column_type {
                        return Err(format!(
                            "table `{}`, column `{}`: type `{}` does not match referenced `{}.{}` type `{}`",
                            table.name,
                            local,
                            lcol.def.column_type.name(),
                            fk.ref_table,
                            refc,
                            tcol.def.column_type.name()
                        ));
                    }
                }
            }
        }
        Ok(())
    }

    /// Tables ordered so referenced tables come before referencing ones
    /// (best-effort: cycles fall back to declaration order).
    pub fn tables_in_dependency_order(&self) -> Vec<&Table> {
        let mut placed: Vec<&Table> = Vec::with_capacity(self.tables.len());
        let mut remaining: Vec<&Table> = self.tables.iter().collect();
        while !remaining.is_empty() {
            let before = placed.len();
            remaining.retain(|t| {
                let deps_ok = t.foreign_keys.iter().all(|fk| {
                    fk.ref_table == t.name || placed.iter().any(|p| p.name == fk.ref_table)
                });
                if deps_ok {
                    placed.push(t);
                    false
                } else {
                    true
                }
            });
            if placed.len() == before {
                // Cycle: append the rest in declaration order.
                placed.append(&mut remaining);
            }
        }
        placed
    }
}

impl Table {
    /// `CREATE TABLE IF NOT EXISTS ...` DDL for this table (SQLite dialect).
    pub fn create_ddl(&self) -> String {
        use crate::dialect::{Sqlite, SqlDialect};
        Sqlite.create_table(self)
    }

    /// The columns forming the primary key, in declaration order.
    pub fn primary_key(&self) -> Vec<&Column> {
        self.columns.iter().filter(|c| c.def.primary_key).collect()
    }
}

/// The default schema written by `powder init` / `powder new`.
pub const SAMPLE_SCHEMA: &str = r#"{
  "$schema": "./powder.schema.schema.json",
  "database": "sqlite",
  "tables": {
    "users": {
      "columns": {
        "id": { "type": "int", "primaryKey": true },
        "name": { "type": "text" },
        "score": { "type": "float", "nullable": true },
        "active": { "type": "bool" }
      }
    },
    "posts": {
      "columns": {
        "id": { "type": "int", "primaryKey": true },
        "user_id": { "type": "int", "references": { "table": "users", "column": "id" } },
        "title": { "type": "text" }
      }
    }
  },
  "queries": {
    "topUsers": {
      "sql": "SELECT id, name, score, active FROM users WHERE active = :active AND score >= :minScore ORDER BY score DESC",
      "params": { "active": "bool", "minScore": "float" },
      "returns": "users"
    }
  }
}
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_sample_and_generates_ddl() {
        let schema = Schema::parse(SAMPLE_SCHEMA).unwrap();
        assert_eq!(schema.tables.len(), 2);
        let users = &schema.tables[0];
        assert_eq!(users.name, "users");
        assert_eq!(
            users.create_ddl(),
            "CREATE TABLE IF NOT EXISTS users (id INTEGER PRIMARY KEY, name TEXT NOT NULL, score REAL, active INTEGER NOT NULL)"
        );
        let posts = &schema.tables[1];
        assert!(posts.create_ddl().contains("FOREIGN KEY (user_id) REFERENCES users(id)"));
    }

    #[test]
    fn rejects_bad_identifiers_and_shapes() {
        assert!(Schema::parse(r#"{"tables":{"bad name":{"columns":{"a":{"type":"int"}}}}}"#).is_err());
        assert!(Schema::parse(r#"{"tables":{"t":{"columns":{}}}}"#).is_err());
        assert!(Schema::parse(r#"{"tables":{"t":{"columns":{"a":{"type":"wat"}}}}}"#).is_err());
        assert!(Schema::parse(
            r#"{"tables":{"t":{"columns":{"a":{"type":"int","primaryKey":true,"nullable":true}}}}}"#
        )
        .is_err());
        assert!(Schema::parse(r#"{"tables":{}}"#).is_err());
    }

    #[test]
    fn composite_primary_keys_are_supported() {
        let schema = Schema::parse(
            r#"{"tables":{"m":{"columns":{
                "a":{"type":"int","primaryKey":true},
                "b":{"type":"text","primaryKey":true}
            }}}}"#,
        )
        .unwrap();
        assert_eq!(schema.tables[0].primary_key().len(), 2);
    }

    #[test]
    fn references_are_validated() {
        // Unknown table.
        assert!(Schema::parse(
            r#"{"tables":{"t":{"columns":{"x":{"type":"int","references":{"table":"nope","column":"id"}}}}}}"#
        )
        .is_err());
        // Type mismatch with the referenced column.
        assert!(Schema::parse(
            r#"{"tables":{
                "u":{"columns":{"id":{"type":"int","primaryKey":true}}},
                "t":{"columns":{"x":{"type":"text","references":{"table":"u","column":"id"}}}}
            }}"#
        )
        .is_err());
    }

    #[test]
    fn named_queries_compile_placeholders_aot() {
        let schema = Schema::parse(SAMPLE_SCHEMA).unwrap();
        assert_eq!(schema.queries.len(), 1);
        let q = &schema.queries[0];
        assert_eq!(q.name, "topUsers");
        assert_eq!(
            q.sql,
            "SELECT id, name, score, active FROM users WHERE active = ? AND score >= ? ORDER BY score DESC"
        );
        assert_eq!(q.param_order, ["active", "minScore"]);
        assert_eq!(q.returns.as_deref(), Some("users"));

        // A param used twice binds twice, in order.
        let s = Schema::parse(
            r#"{"tables":{"t":{"columns":{"a":{"type":"int"}}}},
                "queries":{"q":{"sql":"SELECT a FROM t WHERE a > :x OR a < :x","params":{"x":"int"}}}}"#,
        )
        .unwrap();
        assert_eq!(s.queries[0].param_order, ["x", "x"]);

        // `:name` inside a string literal and `::` casts stay untouched.
        let s2 = Schema::parse(
            r#"{"tables":{"t":{"columns":{"a":{"type":"text"}}}},
                "queries":{"q":{"sql":"SELECT ':notaparam' AS lit, a FROM t WHERE a = :v","params":{"v":"text"}}}}"#,
        )
        .unwrap();
        assert!(s2.queries[0].sql.contains("':notaparam'"), "{}", s2.queries[0].sql);
        assert_eq!(s2.queries[0].param_order, ["v"]);

        // Undeclared placeholder, unused param, unknown returns table -> errors.
        assert!(Schema::parse(
            r#"{"tables":{"t":{"columns":{"a":{"type":"int"}}}},
                "queries":{"q":{"sql":"SELECT :ghost","params":{}}}}"#
        )
        .is_err());
        assert!(Schema::parse(
            r#"{"tables":{"t":{"columns":{"a":{"type":"int"}}}},
                "queries":{"q":{"sql":"SELECT 1","params":{"x":"int"}}}}"#
        )
        .is_err());
        assert!(Schema::parse(
            r#"{"tables":{"t":{"columns":{"a":{"type":"int"}}}},
                "queries":{"q":{"sql":"SELECT 1","params":{},"returns":"nope"}}}"#
        )
        .is_err());
    }

    #[test]
    fn dependency_order_puts_referenced_tables_first() {
        let schema = Schema::parse(
            r#"{"tables":{
                "posts":{"columns":{"user_id":{"type":"int","references":{"table":"users","column":"id"}}}},
                "users":{"columns":{"id":{"type":"int","primaryKey":true}}}
            }}"#,
        )
        .unwrap();
        let order: Vec<&str> = schema
            .tables_in_dependency_order()
            .iter()
            .map(|t| t.name.as_str())
            .collect();
        assert_eq!(order, ["users", "posts"]);
    }
}
