//! CLI-facing live-database helpers: output rendering (`query`), `dump`,
//! `introspect`, `describe`. The blocking engine handle and the raw
//! introspection live in `powder_core::inspect` (shared with the studio
//! dashboard) and are re-exported here.

use std::fmt::Write as _;

use serde_json::{json, Map, Value as J};

pub use powder_core::inspect::{
    batch_rows, json_to_value, schema_summary, table_columns, table_fks, table_names, DbColumn,
    DbForeignKey, Engine,
};


// ---------------------------------------------------------------------------
// Output formats
// ---------------------------------------------------------------------------

fn cell_text(v: &J) -> String {
    match v {
        J::Null => "NULL".into(),
        J::String(s) => s.clone(),
        other => other.to_string(),
    }
}

/// Render rows as an aligned text table (display-width aware enough for
/// monospace terminals; wide CJK glyphs count as 2 columns).
pub fn render_table(headers: &[String], rows: &[Map<String, J>]) -> String {
    fn width(s: &str) -> usize {
        s.chars()
            .map(|c| if (c as u32) > 0x1100 { 2 } else { 1 })
            .sum()
    }
    let cells: Vec<Vec<String>> = rows
        .iter()
        .map(|r| headers.iter().map(|h| cell_text(r.get(h).unwrap_or(&J::Null))).collect())
        .collect();
    let mut widths: Vec<usize> = headers.iter().map(|h| width(h)).collect();
    for row in &cells {
        for (i, c) in row.iter().enumerate() {
            widths[i] = widths[i].max(width(c));
        }
    }
    let mut out = String::new();
    let line = |out: &mut String, vals: &[String]| {
        for (i, v) in vals.iter().enumerate() {
            let pad = widths[i].saturating_sub(width(v));
            let _ = write!(out, "{}{}{}", if i == 0 { "" } else { "  " }, v, " ".repeat(pad));
        }
        out.push('\n');
    };
    line(&mut out, headers);
    line(
        &mut out,
        &widths.iter().map(|w| "-".repeat(*w)).collect::<Vec<_>>(),
    );
    for row in &cells {
        line(&mut out, row);
    }
    out
}

pub fn render_csv(headers: &[String], rows: &[Map<String, J>]) -> String {
    fn esc(s: &str) -> String {
        if s.contains(',') || s.contains('"') || s.contains('\n') {
            format!("\"{}\"", s.replace('"', "\"\""))
        } else {
            s.into()
        }
    }
    let mut out = String::new();
    let _ = writeln!(out, "{}", headers.iter().map(|h| esc(h)).collect::<Vec<_>>().join(","));
    for r in rows {
        let vals: Vec<String> = headers
            .iter()
            .map(|h| match r.get(h).unwrap_or(&J::Null) {
                J::Null => String::new(),
                J::String(s) => esc(s),
                other => other.to_string(),
            })
            .collect();
        let _ = writeln!(out, "{}", vals.join(","));
    }
    out
}

/// Format a query result per `--format` (table | json | csv).
pub fn render(batch: &powder_core::RecordBatch, format: &str) -> Result<String, String> {
    let headers: Vec<String> = batch.columns.iter().map(|c| c.field.name.clone()).collect();
    let rows = batch_rows(batch);
    match format {
        "table" => Ok(format!("{}({} row(s))\n", render_table(&headers, &rows), rows.len())),
        "json" => serde_json::to_string_pretty(&rows).map_err(|e| e.to_string()),
        "csv" => Ok(render_csv(&headers, &rows)),
        other => Err(format!("unknown format `{other}` (expected table, json, or csv)")),
    }
}

// ---------------------------------------------------------------------------
// dump — data export in `powder seed` JSON shape
// ---------------------------------------------------------------------------

/// Order tables parents-first so the dump reseeds under FK enforcement.
pub fn dependency_order(e: &Engine, tables: &[String]) -> Result<Vec<String>, String> {
    let mut deps: Vec<(String, Vec<String>)> = Vec::new();
    for t in tables {
        let refs: Vec<String> = table_fks(e, t)?
            .into_iter()
            .map(|fk| fk.table)
            .filter(|r| r != t && tables.contains(r))
            .collect();
        deps.push((t.clone(), refs));
    }
    let mut out: Vec<String> = Vec::new();
    while out.len() < deps.len() {
        let before = out.len();
        for (t, refs) in &deps {
            if !out.contains(t) && refs.iter().all(|r| out.contains(r)) {
                out.push(t.clone());
            }
        }
        if out.len() == before {
            // FK cycle — fall back to name order for the remainder.
            for (t, _) in &deps {
                if !out.contains(t) {
                    out.push(t.clone());
                }
            }
        }
    }
    Ok(out)
}

pub fn dump(engine: &Engine, only: Option<Vec<String>>) -> Result<String, String> {
    let all = table_names(engine)?;
    let tables: Vec<String> = match only {
        Some(list) => {
            for t in &list {
                if !all.contains(t) {
                    return Err(format!("dump: table `{t}` not found (have: {})", all.join(", ")));
                }
            }
            list
        }
        None => all,
    };
    let ordered = dependency_order(engine, &tables)?;
    let mut doc = Map::new();
    for t in &ordered {
        // Identifier safety: table names come from the live catalog and were
        // validated by introspection, but quote-by-whitelist anyway.
        if !t.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
            return Err(format!("dump: table name `{t}` is not a plain identifier"));
        }
        let batch = engine.query(&format!("SELECT * FROM {t}"))?;
        doc.insert(t.clone(), J::Array(batch_rows(&batch).into_iter().map(J::Object).collect()));
    }
    serde_json::to_string_pretty(&J::Object(doc)).map_err(|e| e.to_string())
}

// ---------------------------------------------------------------------------
// introspect — powder.schema.json from a live database
// ---------------------------------------------------------------------------

/// Reverse-map a live column type onto a schema type for this backend.
fn schema_type(flavor: &str, sql_type: &str) -> Option<&'static str> {
    let t = sql_type.to_ascii_uppercase();
    let base: &str = t.split('(').next().unwrap_or(&t).trim();
    let is_mysql = flavor == "mysql";
    let is_mssql = flavor == "mssql";
    match base {
        "TINYINT" if is_mysql && t.contains("(1)") => Some("bool"),
        "BIT" if is_mssql => Some("bool"),
        "BOOLEAN" | "BOOL" => Some("bool"),
        "INTEGER" | "INT" | "BIGINT" | "SMALLINT" | "TINYINT" | "MEDIUMINT" | "INT2" | "INT4"
        | "INT8" => Some("int"),
        "REAL" | "FLOAT" | "DOUBLE" | "DOUBLE PRECISION" | "FLOAT4" | "FLOAT8" => Some("float"),
        "TEXT" | "VARCHAR" | "NVARCHAR" | "CHAR" | "NCHAR" | "CHARACTER VARYING" | "CHARACTER"
        | "CLOB" | "STRING" | "NTEXT" => Some("text"),
        _ => None,
    }
}

pub fn introspect(e: &Engine, loose: bool) -> Result<String, String> {
    let names = table_names(e)?;
    if names.is_empty() {
        return Err("introspect: no user tables found".into());
    }
    let mut tables = Map::new();
    let mut unmapped: Vec<String> = Vec::new();
    for t in &names {
        let cols = table_columns(e, t)?;
        let fks = table_fks(e, t)?;
        // pk ordinal order for composite keys.
        let mut pk_cols: Vec<&DbColumn> = cols.iter().filter(|c| c.pk > 0).collect();
        pk_cols.sort_by_key(|c| c.pk);

        let single_fk = |col: &str| -> Option<&DbForeignKey> {
            fks.iter().find(|fk| fk.from.len() == 1 && fk.from[0] == col)
        };

        let mut cols_json = Map::new();
        for c in &cols {
            let ty = match schema_type(e.flavor(), &c.sql_type) {
                Some(ty) => ty,
                None if loose => "text",
                None => {
                    unmapped.push(format!("{t}.{} ({})", c.name, c.sql_type));
                    continue;
                }
            };
            let mut def = Map::new();
            def.insert("type".into(), json!(ty));
            if c.pk > 0 {
                def.insert("primaryKey".into(), json!(true));
            } else if !c.notnull {
                def.insert("nullable".into(), json!(true));
            }
            if let Some(fk) = single_fk(&c.name) {
                let to = fk.to.first().cloned().unwrap_or_default();
                if !to.is_empty() {
                    def.insert(
                        "references".into(),
                        json!({ "table": fk.table, "column": to }),
                    );
                }
            }
            cols_json.insert(c.name.clone(), J::Object(def));
        }

        let mut tdef = Map::new();
        tdef.insert("columns".into(), J::Object(cols_json));
        let composite: Vec<&DbForeignKey> = fks.iter().filter(|fk| fk.from.len() > 1).collect();
        if !composite.is_empty() {
            let list: Vec<J> = composite
                .iter()
                .map(|fk| {
                    json!({
                        "columns": fk.from,
                        "references": { "table": fk.table, "columns": fk.to }
                    })
                })
                .collect();
            tdef.insert("foreignKeys".into(), J::Array(list));
        }
        tables.insert(t.clone(), J::Object(tdef));
    }
    if !unmapped.is_empty() {
        return Err(format!(
            "introspect: {} column(s) have no schema type mapping:\n  {}\nre-run with --loose to map them to `text`",
            unmapped.len(),
            unmapped.join("\n  ")
        ));
    }
    serde_json::to_string_pretty(&json!({ "tables": tables })).map_err(|e| e.to_string())
}

// ---------------------------------------------------------------------------
// describe — one table's live shape as text
// ---------------------------------------------------------------------------

pub fn describe(e: &Engine, table: &str) -> Result<String, String> {
    let cols = table_columns(e, table)?;
    if cols.is_empty() {
        return Err(format!("describe: table `{table}` not found"));
    }
    let fks = table_fks(e, table)?;
    let headers: Vec<String> = ["column", "type", "null", "pk"].iter().map(|s| s.to_string()).collect();
    let rows: Vec<Map<String, J>> = cols
        .iter()
        .map(|c| {
            let mut m = Map::new();
            m.insert("column".into(), json!(c.name));
            m.insert("type".into(), json!(c.sql_type));
            m.insert("null".into(), json!(if c.notnull { "NOT NULL" } else { "" }));
            m.insert(
                "pk".into(),
                json!(if c.pk > 0 { format!("PK{}", c.pk) } else { String::new() }),
            );
            m
        })
        .collect();
    let mut out = render_table(&headers, &rows);
    for fk in &fks {
        let _ = writeln!(
            out,
            "FK: ({}) -> {}({})",
            fk.from.join(", "),
            fk.table,
            fk.to.join(", ")
        );
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn table_render_pads_and_counts() {
        let headers = vec!["id".to_string(), "name".to_string()];
        let mut r1 = Map::new();
        r1.insert("id".into(), json!(1));
        r1.insert("name".into(), json!("alice"));
        let mut r2 = Map::new();
        r2.insert("id".into(), json!(2));
        r2.insert("name".into(), J::Null);
        let t = render_table(&headers, &[r1, r2]);
        assert!(t.contains("id  name"), "{t}");
        assert!(t.contains("2   NULL"), "{t}");
    }

    #[test]
    fn csv_escapes() {
        let headers = vec!["v".to_string()];
        let mut r = Map::new();
        r.insert("v".into(), json!("a,\"b\"\nc"));
        let csv = render_csv(&headers, &[r]);
        assert_eq!(csv, "v\n\"a,\"\"b\"\"\nc\"\n");
    }
}
