//! Powder ORM engine — one implementation, every language.
//!
//! The TS and Python ORMs compile Prisma-style operations (`findMany`,
//! `create`, nested `where`, `groupBy`, ...) to SQL in their own runtimes.
//! Every other binding (Rust, Go, Java, Kotlin, C, C++, C#) shares *this*
//! module instead: an operation arrives as one JSON object, the engine renders
//! the same SQL the TS/Python ORMs would, executes it, and hands back either
//! an affected-row count or JSON rows. The wrappers stay thin and the syntax
//! stays unified because the semantics live in exactly one place.
//!
//! ## Operation JSON
//!
//! Every op is `{"op": <name>, "table": <table>, ...}`:
//!
//! | op | extra keys | returns |
//! |---|---|---|
//! | `findMany` | `where`, `orderBy`, `limit`, `offset`, `include`, `join` | rows array |
//! | `findFirst` | same as `findMany` | row or `null` |
//! | `groupBy` | `by`, `where`, `count`, `sum`, `avg`, `min`, `max`, `having`, `orderBy`, `limit`, `offset` | rows array |
//! | `aggregate` | `fn` (sum/avg/min/max), `column`, `where` | number or `null` |
//! | `count` | `where` | integer |
//! | `create` | `data` | affected count |
//! | `createMany` | `rows`, `chunkSize` | affected count |
//! | `update` | `where`, `data` | affected count |
//! | `delete` | `where` (must be non-empty) | affected count |
//! | `deleteAll` | — | affected count |
//!
//! `where` follows the TS/Python ORM exactly: bare values are equality, an
//! object holds operators (`eq`/`ne`/`gt`/`gte`/`lt`/`lte`/`like`/`in`),
//! `null` renders `IS NULL`, and `AND`/`OR`/`NOT` nest to any depth.

use std::collections::HashMap;
use std::sync::Arc;

use serde_json::{Map, Number, Value as J};

use crate::array::Column;
use crate::error::{Error, Result};
use crate::{Client, RecordBatch, Value};

// ---------------------------------------------------------------------------
// Schema
// ---------------------------------------------------------------------------

/// Logical column types — the PCB-transportable set from `powder.schema.json`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColType {
    Int,
    Float,
    Text,
    Bool,
}

#[derive(Debug, Clone)]
pub struct OrmColumn {
    pub name: String,
    pub ty: ColType,
    pub nullable: bool,
    pub primary_key: bool,
}

/// A relation derived from a foreign key, mirroring `powder generate`:
/// a single-column FK `<x>_id` surfaces as belongsTo `<x>`; the reverse
/// direction surfaces as hasMany named after the referring table.
#[derive(Debug, Clone)]
pub struct OrmRelation {
    pub name: String,
    pub belongs_to: bool,
    /// Columns on the owning table, aligned with `foreign_columns`.
    pub local_columns: Vec<String>,
    pub foreign_columns: Vec<String>,
    pub target: String,
}

#[derive(Debug, Clone)]
pub struct OrmTable {
    pub name: String,
    pub columns: Vec<OrmColumn>,
    pub relations: Vec<OrmRelation>,
}

impl OrmTable {
    fn column(&self, name: &str) -> Option<&OrmColumn> {
        self.columns.iter().find(|c| c.name == name)
    }

    fn relation(&self, name: &str) -> Option<&OrmRelation> {
        self.relations.iter().find(|r| r.name == name)
    }

    /// Validated bare identifier for a column, or an ORM error.
    fn ident(&self, name: &str) -> Result<&str> {
        match self.column(name) {
            Some(c) => Ok(&c.name),
            None => Err(orm_err(format!(
                "unknown column `{name}` on table `{}`",
                self.name
            ))),
        }
    }

    fn select_all(&self) -> String {
        let cols: Vec<&str> = self.columns.iter().map(|c| c.name.as_str()).collect();
        format!("SELECT {} FROM {}", cols.join(", "), self.name)
    }
}

/// The parsed `powder.schema.json`, plus the FK-derived relations — the same
/// metadata `powder generate` bakes into the TS/Python models, resolved at
/// runtime so bindings without a codegen step get the full ORM.
#[derive(Debug, Clone)]
pub struct OrmSchema {
    pub tables: Vec<OrmTable>,
    index: HashMap<String, usize>,
}

fn orm_err(msg: impl Into<String>) -> Error {
    Error::Orm(msg.into())
}

fn is_ident(s: &str) -> bool {
    !s.is_empty()
        && s.chars().next().map(|c| c.is_ascii_alphabetic() || c == '_') == Some(true)
        && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

impl OrmSchema {
    /// Parse the `powder.schema.json` text (the `tables` section; other keys
    /// are ignored) and derive relations.
    pub fn parse(json: &str) -> Result<Self> {
        let root: J =
            serde_json::from_str(json).map_err(|e| orm_err(format!("schema is not valid JSON: {e}")))?;
        let tables_j = root
            .get("tables")
            .and_then(J::as_object)
            .ok_or_else(|| orm_err("schema needs a `tables` object"))?;

        // (table, fk columns, fk target table, fk target columns)
        let mut fks: Vec<(String, Vec<String>, String, Vec<String>)> = Vec::new();
        let mut tables: Vec<OrmTable> = Vec::new();

        for (tname, tdef) in tables_j {
            if !is_ident(tname) {
                return Err(orm_err(format!("invalid table name `{tname}`")));
            }
            let cols_j = tdef
                .get("columns")
                .and_then(J::as_object)
                .ok_or_else(|| orm_err(format!("table `{tname}` needs a `columns` object")))?;
            let mut columns = Vec::new();
            for (cname, cdef) in cols_j {
                if !is_ident(cname) {
                    return Err(orm_err(format!("invalid column name `{tname}.{cname}`")));
                }
                let ty = match cdef.get("type").and_then(J::as_str) {
                    Some("int") => ColType::Int,
                    Some("float") => ColType::Float,
                    Some("text") => ColType::Text,
                    Some("bool") => ColType::Bool,
                    other => {
                        return Err(orm_err(format!(
                            "column `{tname}.{cname}` has unsupported type {other:?} (expected int, float, text, bool)"
                        )))
                    }
                };
                let primary_key = cdef.get("primaryKey").and_then(J::as_bool).unwrap_or(false);
                let nullable =
                    cdef.get("nullable").and_then(J::as_bool).unwrap_or(false) && !primary_key;
                if let Some(r) = cdef.get("references") {
                    let rt = r.get("table").and_then(J::as_str).unwrap_or_default();
                    let rc = r.get("column").and_then(J::as_str).unwrap_or_default();
                    if !is_ident(rt) || !is_ident(rc) {
                        return Err(orm_err(format!(
                            "column `{tname}.{cname}` has a malformed `references`"
                        )));
                    }
                    fks.push((tname.clone(), vec![cname.clone()], rt.into(), vec![rc.into()]));
                }
                columns.push(OrmColumn {
                    name: cname.clone(),
                    ty,
                    nullable,
                    primary_key,
                });
            }
            // Table-level composite foreign keys.
            if let Some(list) = tdef.get("foreignKeys").and_then(J::as_array) {
                for fk in list {
                    let cols: Vec<String> = fk
                        .get("columns")
                        .and_then(J::as_array)
                        .map(|a| a.iter().filter_map(J::as_str).map(String::from).collect())
                        .unwrap_or_default();
                    let rt = fk
                        .pointer("/references/table")
                        .and_then(J::as_str)
                        .unwrap_or_default();
                    let rcols: Vec<String> = fk
                        .pointer("/references/columns")
                        .and_then(J::as_array)
                        .map(|a| a.iter().filter_map(J::as_str).map(String::from).collect())
                        .unwrap_or_default();
                    if cols.is_empty() || cols.len() != rcols.len() || !is_ident(rt) {
                        return Err(orm_err(format!("table `{tname}` has a malformed foreign key")));
                    }
                    fks.push((tname.clone(), cols, rt.into(), rcols));
                }
            }
            tables.push(OrmTable {
                name: tname.clone(),
                columns,
                relations: Vec::new(),
            });
        }

        // Derive relations with the same naming as `powder generate`:
        // names are de-duplicated against columns and each other with `_`.
        let table_names: Vec<String> = tables.iter().map(|t| t.name.clone()).collect();
        for i in 0..tables.len() {
            let mut used: Vec<String> = Vec::new();
            let mut rels: Vec<OrmRelation> = Vec::new();
            let claim = |mut name: String, t: &OrmTable, used: &mut Vec<String>| -> String {
                while used.iter().any(|u| u == &name) || t.column(&name).is_some() {
                    name.push('_');
                }
                used.push(name.clone());
                name
            };
            // belongsTo: this table's FKs.
            for (owner, cols, rt, rcols) in fks.iter().filter(|f| f.0 == table_names[i]) {
                let _ = owner;
                let base = if cols.len() == 1 {
                    cols[0]
                        .strip_suffix("_id")
                        .filter(|s| !s.is_empty())
                        .map(str::to_string)
                        .unwrap_or_else(|| rt.clone())
                } else {
                    rt.clone()
                };
                let name = claim(base, &tables[i], &mut used);
                rels.push(OrmRelation {
                    name,
                    belongs_to: true,
                    local_columns: cols.clone(),
                    foreign_columns: rcols.clone(),
                    target: rt.clone(),
                });
            }
            // hasMany: other tables' FKs pointing here.
            for (owner, cols, rt, rcols) in &fks {
                if owner == &table_names[i] || rt != &table_names[i] {
                    continue;
                }
                let name = claim(owner.clone(), &tables[i], &mut used);
                rels.push(OrmRelation {
                    name,
                    belongs_to: false,
                    local_columns: rcols.clone(),
                    foreign_columns: cols.clone(),
                    target: owner.clone(),
                });
            }
            tables[i].relations = rels;
        }

        let index = tables
            .iter()
            .enumerate()
            .map(|(i, t)| (t.name.clone(), i))
            .collect();
        Ok(Self { tables, index })
    }

    pub fn table(&self, name: &str) -> Result<&OrmTable> {
        self.index
            .get(name)
            .map(|&i| &self.tables[i])
            .ok_or_else(|| orm_err(format!("unknown table `{name}`")))
    }
}

// ---------------------------------------------------------------------------
// JSON <-> bound values
// ---------------------------------------------------------------------------

fn to_param(v: &J) -> Result<Value> {
    match v {
        J::Null => Ok(Value::Null),
        J::Bool(b) => Ok(Value::Bool(*b)),
        J::Number(n) => {
            if let Some(i) = n.as_i64() {
                Ok(Value::Int(i))
            } else {
                Ok(Value::Float(n.as_f64().unwrap_or(0.0)))
            }
        }
        J::String(s) => Ok(Value::Text(s.clone())),
        other => Err(orm_err(format!("unsupported bound value: {other}"))),
    }
}

fn num(v: f64) -> J {
    Number::from_f64(v).map(J::Number).unwrap_or(J::Null)
}

/// Read one cell as JSON, coerced to the model's declared column type
/// (SQLite stores bools as 0/1 integers).
fn cell(col: &Column, row: usize, ty: ColType) -> J {
    if !col.is_valid(row) {
        return J::Null;
    }
    match ty {
        ColType::Bool => col
            .bool(row)
            .or_else(|| col.i64(row).map(|v| v != 0))
            .map(J::Bool)
            .unwrap_or(J::Null),
        ColType::Int => col
            .i64(row)
            .map(|v| J::Number(v.into()))
            .or_else(|| col.f64(row).map(num))
            .unwrap_or(J::Null),
        ColType::Float => col
            .f64(row)
            .map(num)
            .or_else(|| col.i64(row).map(|v| J::Number(v.into())))
            .unwrap_or(J::Null),
        ColType::Text => col.str(row).map(|s| J::String(s.into())).unwrap_or(J::Null),
    }
}

/// Read one cell by the column's own storage type (for groupBy aggregates).
fn cell_raw(col: &Column, row: usize) -> J {
    if !col.is_valid(row) {
        return J::Null;
    }
    col.i64(row)
        .map(|v| J::Number(v.into()))
        .or_else(|| col.f64(row).map(num))
        .or_else(|| col.bool(row).map(J::Bool))
        .or_else(|| col.str(row).map(|s| J::String(s.into())))
        .unwrap_or(J::Null)
}

// ---------------------------------------------------------------------------
// WHERE compilation (ported 1:1 from the TS/Python ORM)
// ---------------------------------------------------------------------------

const OPS: [(&str, &str); 8] = [
    ("eq", "="),
    ("ne", "<>"),
    ("gt", ">"),
    ("gte", ">="),
    ("lt", "<"),
    ("lte", "<="),
    ("like", "LIKE"),
    ("in", "IN"),
];

fn sql_op(op: &str) -> Option<&'static str> {
    OPS.iter().find(|(k, _)| *k == op).map(|(_, v)| *v)
}

/// Render one where group to a SQL fragment (no leading WHERE) and collect
/// bound params in placeholder order. Sub-groups are parenthesized so
/// precedence is preserved; an empty group renders "".
fn render_group(table: &OrmTable, where_: &Map<String, J>, q: &str, params: &mut Vec<Value>) -> Result<String> {
    let mut parts: Vec<String> = Vec::new();
    for (col, cond) in where_ {
        if col == "AND" || col == "OR" || col == "NOT" {
            continue;
        }
        let bare = table.ident(col)?;
        let ident = format!("{q}{bare}");
        match cond {
            J::Null => parts.push(format!("{ident} IS NULL")),
            J::Object(ops) => {
                for (op, value) in ops {
                    let Some(sop) = sql_op(op) else { continue };
                    if op == "in" {
                        let list = value.as_array().ok_or_else(|| {
                            orm_err(format!("`in` on `{col}` needs an array"))
                        })?;
                        if list.is_empty() {
                            parts.push("1 = 0".into()); // IN () matches nothing
                        } else {
                            let ph = vec!["?"; list.len()].join(", ");
                            parts.push(format!("{ident} IN ({ph})"));
                            for v in list {
                                params.push(to_param(v)?);
                            }
                        }
                    } else if op == "ne" && value.is_null() {
                        parts.push(format!("{ident} IS NOT NULL"));
                    } else if op == "eq" && value.is_null() {
                        parts.push(format!("{ident} IS NULL"));
                    } else {
                        parts.push(format!("{ident} {sop} ?"));
                        params.push(to_param(value)?);
                    }
                }
            }
            other => {
                parts.push(format!("{ident} = ?"));
                params.push(to_param(other)?);
            }
        }
    }

    let as_group_list = |v: &J| -> Result<Vec<Map<String, J>>> {
        match v {
            J::Array(items) => items
                .iter()
                .map(|w| {
                    w.as_object()
                        .cloned()
                        .ok_or_else(|| orm_err("logical groups must be objects"))
                })
                .collect(),
            J::Object(o) => Ok(vec![o.clone()]),
            _ => Err(orm_err("logical groups must be objects")),
        }
    };

    if let Some(v) = where_.get("AND") {
        for w in as_group_list(v)? {
            let s = render_group(table, &w, q, params)?;
            if !s.is_empty() {
                parts.push(format!("({s})"));
            }
        }
    }
    if let Some(v) = where_.get("OR") {
        let list = v
            .as_array()
            .ok_or_else(|| orm_err("`OR` takes an array of groups"))?;
        if list.is_empty() {
            parts.push("1 = 0".into()); // OR of nothing matches nothing
        } else {
            let mut subs: Vec<String> = Vec::new();
            for w in list {
                let o = w
                    .as_object()
                    .ok_or_else(|| orm_err("logical groups must be objects"))?;
                let s = render_group(table, o, q, params)?;
                if !s.is_empty() {
                    subs.push(format!("({s})"));
                }
            }
            if !subs.is_empty() {
                parts.push(format!("({})", subs.join(" OR ")));
            }
        }
    }
    if let Some(v) = where_.get("NOT") {
        for w in as_group_list(v)? {
            let s = render_group(table, &w, q, params)?;
            if !s.is_empty() {
                parts.push(format!("NOT ({s})"));
            }
        }
    }
    Ok(parts.join(" AND "))
}

/// ` WHERE ...` (or "") plus bound params for an op's `where` value.
fn compile_where(table: &OrmTable, where_: Option<&J>, qualify: Option<&str>) -> Result<(String, Vec<Value>)> {
    let Some(w) = where_ else {
        return Ok((String::new(), Vec::new()));
    };
    if w.is_null() {
        return Ok((String::new(), Vec::new()));
    }
    let obj = w
        .as_object()
        .ok_or_else(|| orm_err("`where` must be an object"))?;
    let q = qualify.map(|t| format!("{t}.")).unwrap_or_default();
    let mut params = Vec::new();
    let frag = render_group(table, obj, &q, &mut params)?;
    if frag.is_empty() {
        Ok((String::new(), params))
    } else {
        Ok((format!(" WHERE {frag}"), params))
    }
}

/// ` ORDER BY ... LIMIT n OFFSET m` from `orderBy`/`limit`/`offset` keys.
fn compile_tail(table: &OrmTable, op: &Map<String, J>, qualify: Option<&str>) -> Result<String> {
    let mut tail = String::new();
    let q = qualify.map(|t| format!("{t}.")).unwrap_or_default();
    if let Some(ob) = op.get("orderBy").and_then(J::as_object) {
        let mut frags: Vec<String> = Vec::new();
        for (col, dir) in ob {
            let bare = table.ident(col)?;
            let d = if dir.as_str().unwrap_or("asc").eq_ignore_ascii_case("desc") {
                "DESC"
            } else {
                "ASC"
            };
            frags.push(format!("{q}{bare} {d}"));
        }
        if !frags.is_empty() {
            tail.push_str(&format!(" ORDER BY {}", frags.join(", ")));
        }
    }
    if let Some(n) = op.get("limit").and_then(J::as_f64) {
        tail.push_str(&format!(" LIMIT {}", n.floor() as i64));
    }
    if let Some(n) = op.get("offset").and_then(J::as_f64) {
        tail.push_str(&format!(" OFFSET {}", n.floor() as i64));
    }
    Ok(tail)
}

/// Keys of `data` that are real columns (relation names are skipped, so rows
/// fetched with `include`/`join` can be written back as-is).
fn column_keys<'a>(table: &OrmTable, data: &'a Map<String, J>) -> Vec<&'a String> {
    data.keys()
        .filter(|k| table.relation(k).is_none())
        .collect()
}

// ---------------------------------------------------------------------------
// Engine
// ---------------------------------------------------------------------------

/// The shared ORM engine: a client plus a parsed schema.
///
/// ```no_run
/// # async fn demo() -> powder_core::Result<()> {
/// use powder_core::{Client, orm::{Orm, OrmSchema}};
/// use serde_json::json;
///
/// let db = Client::connect("app.db").await?;
/// let schema = OrmSchema::parse(&std::fs::read_to_string("powder.schema.json").unwrap())?;
/// let orm = Orm::new(db, schema);
///
/// orm.execute(&json!({"op": "create", "table": "users",
///                     "data": {"id": 1, "name": "alice", "score": 9.5, "active": true}})).await?;
/// let rows = orm.find_json(&json!({"op": "findMany", "table": "users",
///                                  "where": {"active": true, "score": {"gte": 5}},
///                                  "orderBy": {"score": "desc"}, "limit": 10})).await?;
/// # Ok(()) }
/// ```
#[derive(Clone)]
pub struct Orm {
    client: Client,
    schema: Arc<OrmSchema>,
}

/// Stay safely under SQLite's default bound-variable ceiling (32766).
const MAX_VARS: usize = 32000;
/// Batch size for relation `IN` queries and createMany chunks.
const CHUNK: usize = 500;

impl Orm {
    pub fn new(client: Client, schema: OrmSchema) -> Self {
        Self {
            client,
            schema: Arc::new(schema),
        }
    }

    pub fn schema(&self) -> &OrmSchema {
        &self.schema
    }

    pub fn client(&self) -> &Client {
        &self.client
    }

    fn op_parts<'a>(&'a self, op: &'a J) -> Result<(&'a Map<String, J>, &'a OrmTable)> {
        let obj = op
            .as_object()
            .ok_or_else(|| orm_err("op must be a JSON object"))?;
        let tname = obj
            .get("table")
            .and_then(J::as_str)
            .ok_or_else(|| orm_err("op needs a `table` string"))?;
        Ok((obj, self.schema.table(tname)?))
    }

    /// Run a mutation (or `count`) op; returns the affected/row count.
    pub async fn execute(&self, op: &J) -> Result<i64> {
        let (obj, table) = self.op_parts(op)?;
        let name = obj.get("op").and_then(J::as_str).unwrap_or_default();
        match name {
            "create" => {
                let data = obj
                    .get("data")
                    .and_then(J::as_object)
                    .ok_or_else(|| orm_err("create needs a `data` object"))?;
                let keys = column_keys(table, data);
                if keys.is_empty() {
                    return Err(orm_err("create() has no insertable columns"));
                }
                let mut idents = Vec::new();
                let mut params = Vec::new();
                for k in &keys {
                    idents.push(table.ident(k)?.to_string());
                    params.push(to_param(&data[k.as_str()])?);
                }
                let sql = format!(
                    "INSERT INTO {} ({}) VALUES ({})",
                    table.name,
                    idents.join(", "),
                    vec!["?"; keys.len()].join(", ")
                );
                Ok(self.client.execute(&sql, params).await? as i64)
            }
            "createMany" => {
                let rows = obj
                    .get("rows")
                    .and_then(J::as_array)
                    .ok_or_else(|| orm_err("createMany needs a `rows` array"))?;
                if rows.is_empty() {
                    return Ok(0);
                }
                let first = rows[0]
                    .as_object()
                    .ok_or_else(|| orm_err("createMany rows must be objects"))?;
                let keys: Vec<String> = column_keys(table, first).into_iter().cloned().collect();
                if keys.is_empty() {
                    return Err(orm_err("createMany() rows have no insertable columns"));
                }
                let mut idents = Vec::new();
                for k in &keys {
                    idents.push(table.ident(k)?.to_string());
                }
                let chunk_size = obj
                    .get("chunkSize")
                    .and_then(J::as_u64)
                    .map(|n| n as usize)
                    .unwrap_or(CHUNK)
                    .max(1)
                    .min((MAX_VARS / keys.len()).max(1));
                let row_ph = format!("({})", vec!["?"; keys.len()].join(", "));
                let mut affected: i64 = 0;
                for (start, chunk) in rows.chunks(chunk_size).enumerate().map(|(i, c)| (i * chunk_size, c)) {
                    let sql = format!(
                        "INSERT INTO {} ({}) VALUES {}",
                        table.name,
                        idents.join(", "),
                        vec![row_ph.as_str(); chunk.len()].join(", ")
                    );
                    let mut params = Vec::with_capacity(chunk.len() * keys.len());
                    for (i, row) in chunk.iter().enumerate() {
                        let r = row
                            .as_object()
                            .ok_or_else(|| orm_err("createMany rows must be objects"))?;
                        let row_keys = column_keys(table, r);
                        if row_keys.len() != keys.len()
                            || row_keys.iter().any(|k| !keys.contains(k))
                        {
                            return Err(orm_err(format!(
                                "createMany() row {} has a different column set than row 0; all rows must share one shape",
                                start + i
                            )));
                        }
                        for k in &keys {
                            params.push(to_param(&r[k.as_str()])?);
                        }
                    }
                    affected += self.client.execute(&sql, params).await? as i64;
                }
                Ok(affected)
            }
            "update" => {
                let data = obj
                    .get("data")
                    .and_then(J::as_object)
                    .ok_or_else(|| orm_err("update needs a `data` object"))?;
                let sets: Vec<&String> = column_keys(table, data);
                if sets.is_empty() {
                    return Ok(0);
                }
                let mut set_frags = Vec::new();
                let mut params = Vec::new();
                for k in &sets {
                    set_frags.push(format!("{} = ?", table.ident(k)?));
                    params.push(to_param(&data[k.as_str()])?);
                }
                let (clause, wparams) = compile_where(table, obj.get("where"), None)?;
                params.extend(wparams);
                let sql = format!("UPDATE {} SET {}{}", table.name, set_frags.join(", "), clause);
                Ok(self.client.execute(&sql, params).await? as i64)
            }
            "delete" => {
                let (clause, params) = compile_where(table, obj.get("where"), None)?;
                if clause.is_empty() {
                    return Err(orm_err(
                        "delete() requires a non-empty where clause; use deleteAll() to clear the table",
                    ));
                }
                let sql = format!("DELETE FROM {}{}", table.name, clause);
                Ok(self.client.execute(&sql, params).await? as i64)
            }
            "deleteAll" => {
                let sql = format!("DELETE FROM {}", table.name);
                Ok(self.client.execute(&sql, Vec::new()).await? as i64)
            }
            "count" => {
                let (clause, params) = compile_where(table, obj.get("where"), None)?;
                let sql = format!("SELECT COUNT(*) AS n FROM {}{}", table.name, clause);
                let batch = self.client.query(&sql, params).await?;
                Ok(batch
                    .column("n")
                    .and_then(|c| c.i64(0))
                    .unwrap_or(0))
            }
            other => Err(orm_err(format!(
                "unknown execute op `{other}` (expected create, createMany, update, delete, deleteAll, count)"
            ))),
        }
    }

    /// Run a row-returning op; returns JSON (`findMany`/`groupBy` → array,
    /// `findFirst` → object or null, `aggregate` → number or null).
    pub async fn find_json(&self, op: &J) -> Result<J> {
        let (obj, table) = self.op_parts(op)?;
        let name = obj.get("op").and_then(J::as_str).unwrap_or_default();
        match name {
            "findMany" => Ok(J::Array(self.find_many(obj, table).await?)),
            "findFirst" => {
                let mut o = obj.clone();
                o.insert("limit".into(), J::Number(1.into()));
                let mut rows = self.find_many(&o, table).await?;
                Ok(if rows.is_empty() { J::Null } else { rows.remove(0) })
            }
            "groupBy" => self.group_by(obj, table).await,
            "aggregate" => self.aggregate(obj, table).await,
            other => Err(orm_err(format!(
                "unknown find op `{other}` (expected findMany, findFirst, groupBy, aggregate)"
            ))),
        }
    }

    async fn find_many(&self, obj: &Map<String, J>, table: &OrmTable) -> Result<Vec<J>> {
        let joined = obj
            .get("join")
            .and_then(J::as_object)
            .filter(|m| m.values().any(truthy));
        let mut rows = if let Some(join) = joined {
            self.find_many_joined(obj, table, join).await?
        } else {
            let (clause, params) = compile_where(table, obj.get("where"), None)?;
            let sql = format!("{}{}{}", table.select_all(), clause, compile_tail(table, obj, None)?);
            let batch = self.client.query(&sql, params).await?;
            materialize(&batch, table)
        };
        if let Some(include) = obj.get("include").and_then(J::as_object) {
            self.attach_relations(&mut rows, include, table).await?;
        }
        Ok(rows)
    }

    /// Single-query path: LEFT JOIN each requested belongsTo relation and
    /// hydrate the nested object from `<rel>__<col>` aliases.
    async fn find_many_joined(
        &self,
        obj: &Map<String, J>,
        table: &OrmTable,
        join: &Map<String, J>,
    ) -> Result<Vec<J>> {
        let mut rels: Vec<&OrmRelation> = Vec::new();
        for (name, want) in join {
            if !truthy(want) {
                continue;
            }
            let rel = table
                .relation(name)
                .ok_or_else(|| orm_err(format!("unknown relation `{name}`")))?;
            if !rel.belongs_to {
                return Err(orm_err(format!(
                    "relation `{name}` is hasMany; use include (a JOIN would multiply rows)"
                )));
            }
            rels.push(rel);
        }

        let mut selects: Vec<String> = table
            .columns
            .iter()
            .map(|c| format!("{}.{} AS {}", table.name, c.name, c.name))
            .collect();
        let mut joins: Vec<String> = Vec::new();
        for rel in &rels {
            let target = self.schema.table(&rel.target)?;
            let alias = format!("j_{}", rel.name);
            for c in &target.columns {
                selects.push(format!("{alias}.{} AS {}__{}", c.name, rel.name, c.name));
            }
            let on: Vec<String> = rel
                .local_columns
                .iter()
                .zip(&rel.foreign_columns)
                .map(|(lc, fc)| format!("{}.{lc} = {alias}.{fc}", table.name))
                .collect();
            joins.push(format!("LEFT JOIN {} AS {alias} ON {}", target.name, on.join(" AND ")));
        }

        let (clause, params) = compile_where(table, obj.get("where"), Some(&table.name))?;
        let sql = format!(
            "SELECT {} FROM {} {}{}{}",
            selects.join(", "),
            table.name,
            joins.join(" "),
            clause,
            compile_tail(table, obj, Some(&table.name))?
        );
        let batch = self.client.query(&sql, params).await?;

        let mut out: Vec<J> = Vec::with_capacity(batch.num_rows);
        for r in 0..batch.num_rows {
            let mut row = Map::new();
            for c in &table.columns {
                let v = batch.column(&c.name).map(|col| cell(col, r, c.ty)).unwrap_or(J::Null);
                row.insert(c.name.clone(), v);
            }
            for rel in &rels {
                let target = self.schema.table(&rel.target)?;
                // A LEFT JOIN miss leaves every target column null.
                let mut present = false;
                let mut nested = Map::new();
                for c in &target.columns {
                    let v = batch
                        .column(&format!("{}__{}", rel.name, c.name))
                        .map(|col| cell(col, r, c.ty))
                        .unwrap_or(J::Null);
                    if !v.is_null() {
                        present = true;
                    }
                    nested.insert(c.name.clone(), v);
                }
                row.insert(rel.name.clone(), if present { J::Object(nested) } else { J::Null });
            }
            out.push(J::Object(row));
        }
        Ok(out)
    }

    /// Batch-load `include`d relations and attach them: one `IN` query per
    /// relation per level (no N+1). Object-form entries recurse.
    fn attach_relations<'a>(
        &'a self,
        rows: &'a mut [J],
        include: &'a Map<String, J>,
        table: &'a OrmTable,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<()>> + Send + 'a>> {
        Box::pin(async move {
            for (name, spec) in include {
                if !truthy(spec) {
                    continue;
                }
                let rel = table.relation(name).ok_or_else(|| {
                    orm_err(format!(
                        "unknown relation `{name}` (no foreign key on {} defines it)",
                        table.name
                    ))
                })?;
                let target = self.schema.table(&rel.target)?;

                // Distinct local key tuples (rows with a null key column are skipped).
                let tuple_of = |row: &J, cols: &[String]| -> Option<Vec<J>> {
                    let obj = row.as_object()?;
                    let mut vals = Vec::with_capacity(cols.len());
                    for c in cols {
                        let v = obj.get(c)?;
                        if v.is_null() {
                            return None;
                        }
                        vals.push(v.clone());
                    }
                    Some(vals)
                };
                let mut seen: Vec<String> = Vec::new();
                let mut tuples: Vec<Vec<J>> = Vec::new();
                for row in rows.iter() {
                    if let Some(t) = tuple_of(row, &rel.local_columns) {
                        let key = J::Array(t.clone()).to_string();
                        if !seen.contains(&key) {
                            seen.push(key);
                            tuples.push(t);
                        }
                    }
                }

                let mut fidents = Vec::new();
                for c in &rel.foreign_columns {
                    fidents.push(target.ident(c)?.to_string());
                }
                let single = fidents.len() == 1;
                let mut loaded: Vec<J> = Vec::new();
                for chunk in tuples.chunks(CHUNK) {
                    let mut params: Vec<Value> = Vec::new();
                    let clause = if single {
                        for t in chunk {
                            params.push(to_param(&t[0])?);
                        }
                        format!("{} IN ({})", fidents[0], vec!["?"; chunk.len()].join(", "))
                    } else {
                        let row_ph = format!("({})", vec!["?"; fidents.len()].join(", "));
                        for t in chunk {
                            for v in t {
                                params.push(to_param(v)?);
                            }
                        }
                        format!(
                            "({}) IN ({})",
                            fidents.join(", "),
                            vec![row_ph.as_str(); chunk.len()].join(", ")
                        )
                    };
                    let sql = format!("{} WHERE {}", target.select_all(), clause);
                    let batch = self.client.query(&sql, params).await?;
                    loaded.extend(materialize(&batch, target));
                }

                // Nested include: recurse over the loaded target rows first.
                if let Some(nested) = spec.get("include").and_then(J::as_object) {
                    if !loaded.is_empty() {
                        self.attach_relations(&mut loaded, nested, target).await?;
                    }
                }

                // Group target rows by their foreign-key tuple.
                let mut grouped: HashMap<String, Vec<J>> = HashMap::new();
                for trow in loaded {
                    if let Some(t) = tuple_of(&trow, &rel.foreign_columns) {
                        grouped
                            .entry(J::Array(t).to_string())
                            .or_default()
                            .push(trow);
                    }
                }

                for row in rows.iter_mut() {
                    let matches = tuple_of(row, &rel.local_columns)
                        .and_then(|t| grouped.get(&J::Array(t).to_string()));
                    let value = if rel.belongs_to {
                        matches.and_then(|m| m.first()).cloned().unwrap_or(J::Null)
                    } else {
                        J::Array(matches.cloned().unwrap_or_default())
                    };
                    if let Some(obj) = row.as_object_mut() {
                        obj.insert(rel.name.clone(), value);
                    }
                }
            }
            Ok(())
        })
    }

    async fn aggregate(&self, obj: &Map<String, J>, table: &OrmTable) -> Result<J> {
        let f = obj
            .get("fn")
            .and_then(J::as_str)
            .ok_or_else(|| orm_err("aggregate needs `fn` (sum, avg, min, max)"))?;
        if !matches!(f, "sum" | "avg" | "min" | "max") {
            return Err(orm_err(format!("unknown aggregate fn `{f}`")));
        }
        let column = obj
            .get("column")
            .and_then(J::as_str)
            .ok_or_else(|| orm_err("aggregate needs a `column`"))?;
        let ident = table.ident(column)?;
        let (clause, params) = compile_where(table, obj.get("where"), None)?;
        let sql = format!(
            "SELECT {}({ident}) AS v FROM {}{}",
            f.to_uppercase(),
            table.name,
            clause
        );
        let batch = self.client.query(&sql, params).await?;
        Ok(batch.column("v").map(|c| cell_raw(c, 0)).unwrap_or(J::Null))
    }

    /// GROUP BY with aggregates; returns plain rows. Aggregates alias as
    /// `_count`, `_sum_<col>`, `_avg_<col>`, `_min_<col>`, `_max_<col>`;
    /// HAVING and ORDER BY may reference those aliases.
    async fn group_by(&self, obj: &Map<String, J>, table: &OrmTable) -> Result<J> {
        let by: Vec<&str> = obj
            .get("by")
            .and_then(J::as_array)
            .map(|a| a.iter().filter_map(J::as_str).collect())
            .unwrap_or_default();
        if by.is_empty() {
            return Err(orm_err("groupBy requires at least one column"));
        }
        let mut by_idents = Vec::new();
        for c in &by {
            by_idents.push(table.ident(c)?.to_string());
        }
        let mut selects = by_idents.clone();
        let mut agg_expr: HashMap<String, String> = HashMap::new();
        if obj.get("count").and_then(J::as_bool).unwrap_or(false) {
            selects.push("COUNT(*) AS _count".into());
            agg_expr.insert("_count".into(), "COUNT(*)".into());
        }
        for f in ["sum", "avg", "min", "max"] {
            if let Some(cols) = obj.get(f).and_then(J::as_array) {
                for c in cols {
                    let c = c
                        .as_str()
                        .ok_or_else(|| orm_err(format!("`{f}` takes column names")))?;
                    let expr = format!("{}({})", f.to_uppercase(), table.ident(c)?);
                    let alias = format!("_{f}_{c}");
                    selects.push(format!("{expr} AS {alias}"));
                    agg_expr.insert(alias, expr);
                }
            }
        }

        let (where_clause, mut params) = compile_where(table, obj.get("where"), None)?;

        let mut having_parts: Vec<String> = Vec::new();
        if let Some(having) = obj.get("having").and_then(J::as_object) {
            for (alias, cond) in having {
                let expr = agg_expr
                    .get(alias)
                    .ok_or_else(|| orm_err(format!("having references unknown aggregate `{alias}`")))?;
                let cond = cond
                    .as_object()
                    .ok_or_else(|| orm_err("having conditions must be objects"))?;
                for (op, value) in cond {
                    let sop = match op.as_str() {
                        "eq" => "=",
                        "ne" => "<>",
                        "gt" => ">",
                        "gte" => ">=",
                        "lt" => "<",
                        "lte" => "<=",
                        other => {
                            return Err(orm_err(format!(
                                "having supports only comparison operators, got `{other}`"
                            )))
                        }
                    };
                    having_parts.push(format!("{expr} {sop} ?"));
                    params.push(to_param(value)?);
                }
            }
        }
        let having_clause = if having_parts.is_empty() {
            String::new()
        } else {
            format!(" HAVING {}", having_parts.join(" AND "))
        };

        let mut order_clause = String::new();
        if let Some(ob) = obj.get("orderBy").and_then(J::as_object) {
            let mut frags: Vec<String> = Vec::new();
            for (key, dir) in ob {
                let target = if agg_expr.contains_key(key) {
                    key.clone()
                } else {
                    table.ident(key)?.to_string()
                };
                let d = if dir.as_str().unwrap_or("asc").eq_ignore_ascii_case("desc") {
                    "DESC"
                } else {
                    "ASC"
                };
                frags.push(format!("{target} {d}"));
            }
            if !frags.is_empty() {
                order_clause = format!(" ORDER BY {}", frags.join(", "));
            }
        }

        let mut limit_clause = String::new();
        if let Some(n) = obj.get("limit").and_then(J::as_f64) {
            limit_clause.push_str(" LIMIT ?");
            params.push(Value::Int(n.floor() as i64));
        }
        if let Some(n) = obj.get("offset").and_then(J::as_f64) {
            limit_clause.push_str(" OFFSET ?");
            params.push(Value::Int(n.floor() as i64));
        }

        let sql = format!(
            "SELECT {} FROM {}{} GROUP BY {}{}{}{}",
            selects.join(", "),
            table.name,
            where_clause,
            by_idents.join(", "),
            having_clause,
            order_clause,
            limit_clause
        );
        let batch = self.client.query(&sql, params).await?;

        let mut out: Vec<J> = Vec::with_capacity(batch.num_rows);
        for r in 0..batch.num_rows {
            let mut row = Map::new();
            for col in &batch.columns {
                let name = &col.field.name;
                // Group columns keep their model type; aggregates read raw.
                let v = match table.column(name) {
                    Some(cm) => cell(col, r, cm.ty),
                    None => cell_raw(col, r),
                };
                row.insert(name.clone(), v);
            }
            out.push(J::Object(row));
        }
        Ok(J::Array(out))
    }
}

fn truthy(v: &J) -> bool {
    match v {
        J::Bool(b) => *b,
        J::Null => false,
        J::Object(_) => true,
        _ => false,
    }
}

/// Materialize a batch into JSON objects following the table's column types.
fn materialize(batch: &RecordBatch, table: &OrmTable) -> Vec<J> {
    let cols: Vec<(&OrmColumn, Option<&Column>)> = table
        .columns
        .iter()
        .map(|c| (c, batch.column(&c.name)))
        .collect();
    let mut out = Vec::with_capacity(batch.num_rows);
    for r in 0..batch.num_rows {
        let mut row = Map::new();
        for (cm, col) in &cols {
            let v = col.map(|c| cell(c, r, cm.ty)).unwrap_or(J::Null);
            row.insert(cm.name.clone(), v);
        }
        out.push(J::Object(row));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const SCHEMA: &str = r#"{
        "tables": {
            "users": {
                "columns": {
                    "id":     { "type": "int", "primaryKey": true },
                    "name":   { "type": "text" },
                    "score":  { "type": "float", "nullable": true },
                    "active": { "type": "bool" }
                }
            },
            "posts": {
                "columns": {
                    "id":      { "type": "int", "primaryKey": true },
                    "user_id": { "type": "int", "references": { "table": "users", "column": "id" } },
                    "title":   { "type": "text" }
                }
            }
        }
    }"#;

    async fn setup() -> Orm {
        let db = Client::connect("sqlite::memory:").await.unwrap();
        db.execute(
            "CREATE TABLE users (id INTEGER PRIMARY KEY, name TEXT, score REAL, active INTEGER)",
            vec![],
        )
        .await
        .unwrap();
        db.execute(
            "CREATE TABLE posts (id INTEGER PRIMARY KEY, user_id INTEGER, title TEXT)",
            vec![],
        )
        .await
        .unwrap();
        Orm::new(db, OrmSchema::parse(SCHEMA).unwrap())
    }

    #[test]
    fn schema_derives_relations_like_codegen() {
        let s = OrmSchema::parse(SCHEMA).unwrap();
        let posts = s.table("posts").unwrap();
        let rel = &posts.relations[0];
        assert_eq!(rel.name, "user"); // user_id -> user
        assert!(rel.belongs_to);
        assert_eq!(rel.local_columns, vec!["user_id"]);
        assert_eq!(rel.foreign_columns, vec!["id"]);
        let users = s.table("users").unwrap();
        let rel = &users.relations[0];
        assert_eq!(rel.name, "posts"); // reverse hasMany
        assert!(!rel.belongs_to);
    }

    #[tokio::test]
    async fn crud_roundtrip() {
        let orm = setup().await;
        let n = orm
            .execute(&json!({"op": "create", "table": "users",
                "data": {"id": 1, "name": "alice", "score": 9.5, "active": true}}))
            .await
            .unwrap();
        assert_eq!(n, 1);
        orm.execute(&json!({"op": "createMany", "table": "users", "rows": [
            {"id": 2, "name": "bob",   "score": 3.0, "active": false},
            {"id": 3, "name": "carol", "score": 7.5, "active": true}
        ]}))
        .await
        .unwrap();

        let rows = orm
            .find_json(&json!({"op": "findMany", "table": "users",
                "where": {"active": true, "score": {"gte": 5}},
                "orderBy": {"score": "desc"}}))
            .await
            .unwrap();
        let rows = rows.as_array().unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0]["name"], "alice");
        assert_eq!(rows[0]["active"], true); // bool coercion from INTEGER

        let n = orm
            .execute(&json!({"op": "update", "table": "users",
                "where": {"id": 2}, "data": {"score": 10}}))
            .await
            .unwrap();
        assert_eq!(n, 1);

        let count = orm
            .execute(&json!({"op": "count", "table": "users", "where": {"score": {"gte": 7}}}))
            .await
            .unwrap();
        assert_eq!(count, 3);

        let n = orm
            .execute(&json!({"op": "delete", "table": "users", "where": {"id": 3}}))
            .await
            .unwrap();
        assert_eq!(n, 1);
        assert!(orm
            .execute(&json!({"op": "delete", "table": "users", "where": {}}))
            .await
            .is_err());
        let n = orm
            .execute(&json!({"op": "deleteAll", "table": "users"}))
            .await
            .unwrap();
        assert_eq!(n, 2);
    }

    #[tokio::test]
    async fn nested_where_and_operators() {
        let orm = setup().await;
        orm.execute(&json!({"op": "createMany", "table": "users", "rows": [
            {"id": 1, "name": "alice", "score": 9.5,  "active": true},
            {"id": 2, "name": "bob",   "score": 3.0,  "active": false},
            {"id": 3, "name": "carol", "score": null, "active": true}
        ]}))
        .await
        .unwrap();

        // OR + operator objects + IS NULL.
        let rows = orm
            .find_json(&json!({"op": "findMany", "table": "users",
                "where": {"OR": [
                    {"score": {"gt": 5}},
                    {"score": null}
                ]},
                "orderBy": {"id": "asc"}}))
            .await
            .unwrap();
        let names: Vec<&str> = rows
            .as_array()
            .unwrap()
            .iter()
            .map(|r| r["name"].as_str().unwrap())
            .collect();
        assert_eq!(names, vec!["alice", "carol"]);

        // NOT + in + empty-IN matches nothing.
        let rows = orm
            .find_json(&json!({"op": "findMany", "table": "users",
                "where": {"NOT": {"name": {"in": ["bob"]}}, "active": true}}))
            .await
            .unwrap();
        assert_eq!(rows.as_array().unwrap().len(), 2);
        let rows = orm
            .find_json(&json!({"op": "findMany", "table": "users",
                "where": {"id": {"in": []}}}))
            .await
            .unwrap();
        assert_eq!(rows.as_array().unwrap().len(), 0);

        // ne: null -> IS NOT NULL, like.
        let rows = orm
            .find_json(&json!({"op": "findMany", "table": "users",
                "where": {"score": {"ne": null}, "name": {"like": "%li%"}}}))
            .await
            .unwrap();
        assert_eq!(rows.as_array().unwrap()[0]["name"], "alice");
    }

    #[tokio::test]
    async fn include_and_join_load_relations() {
        let orm = setup().await;
        orm.execute(&json!({"op": "createMany", "table": "users", "rows": [
            {"id": 1, "name": "alice", "score": 9.5, "active": true},
            {"id": 2, "name": "bob",   "score": 3.0, "active": false}
        ]}))
        .await
        .unwrap();
        orm.execute(&json!({"op": "createMany", "table": "posts", "rows": [
            {"id": 10, "user_id": 1, "title": "hi"},
            {"id": 11, "user_id": 1, "title": "again"},
            {"id": 12, "user_id": 2, "title": "yo"}
        ]}))
        .await
        .unwrap();

        // include: belongsTo.
        let rows = orm
            .find_json(&json!({"op": "findMany", "table": "posts",
                "include": {"user": true}, "orderBy": {"id": "asc"}}))
            .await
            .unwrap();
        assert_eq!(rows[0]["user"]["name"], "alice");
        assert_eq!(rows[2]["user"]["name"], "bob");

        // include: hasMany + nested include back down.
        let rows = orm
            .find_json(&json!({"op": "findMany", "table": "users",
                "include": {"posts": {"include": {"user": true}}},
                "orderBy": {"id": "asc"}}))
            .await
            .unwrap();
        assert_eq!(rows[0]["posts"].as_array().unwrap().len(), 2);
        assert_eq!(rows[0]["posts"][0]["user"]["name"], "alice");
        assert_eq!(rows[1]["posts"].as_array().unwrap().len(), 1);

        // join: single-query belongsTo hydration.
        let rows = orm
            .find_json(&json!({"op": "findMany", "table": "posts",
                "join": {"user": true}, "where": {"id": 10}}))
            .await
            .unwrap();
        assert_eq!(rows[0]["user"]["name"], "alice");

        // join on hasMany is rejected.
        assert!(orm
            .find_json(&json!({"op": "findMany", "table": "users", "join": {"posts": true}}))
            .await
            .is_err());
    }

    #[tokio::test]
    async fn group_by_and_aggregate() {
        let orm = setup().await;
        orm.execute(&json!({"op": "createMany", "table": "users", "rows": [
            {"id": 1, "name": "a", "score": 10, "active": true},
            {"id": 2, "name": "b", "score": 20, "active": true},
            {"id": 3, "name": "c", "score": 5,  "active": false}
        ]}))
        .await
        .unwrap();

        let rows = orm
            .find_json(&json!({"op": "groupBy", "table": "users",
                "by": ["active"], "count": true, "avg": ["score"],
                "having": {"_count": {"gte": 2}},
                "orderBy": {"_count": "desc"}}))
            .await
            .unwrap();
        let rows = rows.as_array().unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["_count"], 2);
        assert_eq!(rows[0]["_avg_score"], 15.0);
        assert_eq!(rows[0]["active"], true);

        let v = orm
            .find_json(&json!({"op": "aggregate", "table": "users",
                "fn": "max", "column": "score", "where": {"active": true}}))
            .await
            .unwrap();
        assert_eq!(v, json!(20.0));
        let v = orm
            .find_json(&json!({"op": "aggregate", "table": "users",
                "fn": "sum", "column": "score", "where": {"id": {"gt": 99}}}))
            .await
            .unwrap();
        assert!(v.is_null());

        let n = orm
            .execute(&json!({"op": "count", "table": "users"}))
            .await
            .unwrap();
        assert_eq!(n, 3);
    }

    #[tokio::test]
    async fn find_first_and_errors() {
        let orm = setup().await;
        let v = orm
            .find_json(&json!({"op": "findFirst", "table": "users"}))
            .await
            .unwrap();
        assert!(v.is_null());
        assert!(orm
            .find_json(&json!({"op": "findMany", "table": "nope"}))
            .await
            .is_err());
        assert!(orm
            .find_json(&json!({"op": "findMany", "table": "users", "where": {"ghost": 1}}))
            .await
            .is_err());
    }
}
