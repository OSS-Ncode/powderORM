//! AOT code generation: `powder.schema.json` -> typed model modules.
//!
//! This is the "AOT query compilation" step of the plan: the SQL skeletons for
//! every base CRUD operation and every quoted column identifier are rendered
//! here, at build time. The generated module hands them to the language's
//! Powder runtime, which only binds parameters at run time — no query text is
//! parsed or assembled from scratch per call.

use crate::schema::{Column, Schema, Table};

/// Whether a relation yields one row or many.
#[derive(PartialEq, Clone, Copy)]
enum RelKind {
    /// This table's foreign key points at `target` — a single related row.
    BelongsTo,
    /// `target`'s foreign key points back at this table — an array of rows.
    HasMany,
}

/// A relation surfaced on `table`, derived from a foreign key in either
/// direction. Columns are ordered pairs so composite keys join correctly.
struct Relation {
    name: String,
    kind: RelKind,
    /// Columns on `table` used to match.
    local_columns: Vec<String>,
    /// Columns on `target_table` used to match, aligned with `local_columns`.
    foreign_columns: Vec<String>,
    target_table: String,
}

/// Derive belongs-to relations (this table's FKs) and has-many relations
/// (other tables' FKs pointing here). Names are de-duplicated against the
/// table's own columns and each other.
fn relations_of(schema: &Schema, table: &Table) -> Vec<Relation> {
    let mut used: Vec<String> = Vec::new();
    let mut out: Vec<Relation> = Vec::new();

    let claim = |mut name: String, table: &Table, used: &mut Vec<String>| -> String {
        while used.iter().any(|u| u == &name) || table.columns.iter().any(|c| c.name == name) {
            name.push('_');
        }
        used.push(name.clone());
        name
    };

    // belongsTo: this table -> referenced tables.
    for fk in &table.foreign_keys {
        // Single-column `<x>_id` collapses to a `<x>` relation; otherwise use
        // the target table name.
        let base = if fk.columns.len() == 1 {
            fk.columns[0]
                .strip_suffix("_id")
                .filter(|s| !s.is_empty())
                .map(str::to_string)
                .unwrap_or_else(|| fk.ref_table.clone())
        } else {
            fk.ref_table.clone()
        };
        let name = claim(base, table, &mut used);
        out.push(Relation {
            name,
            kind: RelKind::BelongsTo,
            local_columns: fk.columns.clone(),
            foreign_columns: fk.ref_columns.clone(),
            target_table: fk.ref_table.clone(),
        });
    }

    // hasMany: other tables whose FK references this table.
    for other in &schema.tables {
        if other.name == table.name {
            continue;
        }
        for fk in &other.foreign_keys {
            if fk.ref_table != table.name {
                continue;
            }
            // Plural-ish default: the referring table's name.
            let name = claim(other.name.clone(), table, &mut used);
            out.push(Relation {
                name,
                kind: RelKind::HasMany,
                local_columns: fk.ref_columns.clone(),
                foreign_columns: fk.columns.clone(),
                target_table: other.name.clone(),
            });
        }
    }

    out
}

/// Render a `["a", "b"]` string-array literal for TS/Python (identical syntax).
fn arr_lit(items: &[String]) -> String {
    let inner: Vec<String> = items.iter().map(|s| format!("\"{s}\"")).collect();
    format!("[{}]", inner.join(", "))
}

fn column_default_py(col: &Column) -> &'static str {
    let _ = col;
    " = None"
}

fn pascal(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    let mut upper = true;
    for c in name.chars() {
        if c == '_' {
            upper = true;
        } else if upper {
            out.extend(c.to_uppercase());
            upper = false;
        } else {
            out.push(c);
        }
    }
    out
}

fn shout(name: &str) -> String {
    name.to_ascii_uppercase()
}

/// camelCase → snake_case. Python codegen exposes named queries with
/// snake_case names/kwargs (matching the rest of the Python runtime API)
/// while the SQL param names keep their original schema spelling.
fn snake(name: &str) -> String {
    let mut out = String::with_capacity(name.len() + 4);
    for c in name.chars() {
        if c.is_ascii_uppercase() {
            if !out.is_empty() && !out.ends_with('_') {
                out.push('_');
            }
            out.push(c.to_ascii_lowercase());
        } else {
            out.push(c);
        }
    }
    out
}

fn column_list(table: &Table) -> String {
    table
        .columns
        .iter()
        .map(|c| c.name.as_str())
        .collect::<Vec<_>>()
        .join(", ")
}

fn insert_sql(table: &Table) -> String {
    format!(
        "INSERT INTO {} ({}) VALUES ({})",
        table.name,
        column_list(table),
        vec!["?"; table.columns.len()].join(", ")
    )
}

/// Generate the TypeScript model module.
pub fn typescript(schema: &Schema, import_from: &str) -> String {
    let mut out = String::new();
    out.push_str("// AUTO-GENERATED by `powder generate` — do not edit.\n");
    out.push_str("// Regenerate after changing powder.schema.json.\n\n");
    if schema.queries.is_empty() {
        out.push_str(&format!(
            "import {{ Client, PowderTable, extendPowder, type ExtendedClient, type PowderExtensions, type TableMeta }} from \"{import_from}\";\n\n"
        ));
    } else {
        out.push_str(&format!(
            "import {{ Client, PowderTable, extendPowder, runNamedQuery, type ExtendedClient, type PowderExtensions, type TableMeta }} from \"{import_from}\";\n\n"
        ));
    }

    for table in &schema.tables {
        let ty = pascal(&table.name);
        let relations = relations_of(schema, table);
        out.push_str(&format!("/** Row shape of the `{}` table. */\n", table.name));
        out.push_str(&format!("export interface {ty} {{\n"));
        for col in &table.columns {
            let base = col.def.column_type.ts_type();
            let ts = if col.def.nullable {
                format!("{base} | null")
            } else {
                base.to_string()
            };
            out.push_str(&format!("  {}: {};\n", col.name, ts));
        }
        for rel in &relations {
            let target = pascal(&rel.target_table);
            let ty = match rel.kind {
                RelKind::BelongsTo => format!("{target} | null"),
                RelKind::HasMany => format!("{target}[]"),
            };
            out.push_str(&format!(
                "  /** Loaded via `include: {{ {}: true }}`. */\n  {}?: {};\n",
                rel.name, rel.name, ty
            ));
        }
        out.push_str("}\n\n");

        // Per-table include type: relation names (and their nesting) become
        // editor completions instead of free-form strings.
        out.push_str(&format!(
            "/** Relations loadable on `{}` — powers `include`/`join` autocompletion. */\n",
            table.name
        ));
        if relations.is_empty() {
            out.push_str(&format!(
                "export type {ty}Include = Record<never, never>;\n\n"
            ));
        } else {
            out.push_str(&format!("export type {ty}Include = {{\n"));
            for rel in &relations {
                let target = pascal(&rel.target_table);
                out.push_str(&format!(
                    "  {}?: boolean | {{ include?: {target}Include }};\n",
                    rel.name
                ));
            }
            out.push_str("};\n\n");
        }

        out.push_str(&format!(
            "/** AOT-compiled metadata for `{}` (SQL precompiled at generation time). */\n",
            table.name
        ));
        out.push_str(&format!("export const {}_META: TableMeta = {{\n", shout(&table.name)));
        out.push_str(&format!("  table: \"{}\",\n", table.name));
        out.push_str("  columns: [\n");
        for col in &table.columns {
            let mut attrs = format!("name: \"{}\", type: \"{}\"", col.name, col.def.column_type.name());
            if col.def.nullable {
                attrs.push_str(", nullable: true");
            }
            if col.def.primary_key {
                attrs.push_str(", primaryKey: true");
            }
            out.push_str(&format!("    {{ {attrs} }},\n"));
        }
        out.push_str("  ],\n");
        out.push_str("  sql: {\n");
        out.push_str(&format!(
            "    selectAll: \"SELECT {} FROM {}\",\n",
            column_list(table),
            table.name
        ));
        out.push_str(&format!("    insert: \"{}\",\n", insert_sql(table)));
        out.push_str(&format!(
            "    countAll: \"SELECT COUNT(*) AS n FROM {}\",\n",
            table.name
        ));
        out.push_str(&format!("    deleteAll: \"DELETE FROM {}\",\n", table.name));
        out.push_str("    ident: {\n");
        for col in &table.columns {
            out.push_str(&format!("      {}: \"{}\",\n", col.name, col.name));
        }
        out.push_str("    },\n  },\n");
        if !relations.is_empty() {
            out.push_str("  relations: [\n");
            for rel in &relations {
                let kind = match rel.kind {
                    RelKind::BelongsTo => "belongsTo",
                    RelKind::HasMany => "hasMany",
                };
                out.push_str(&format!(
                    "    {{ name: \"{}\", kind: \"{}\", localColumns: {}, foreignColumns: {}, target: () => {}_META }},\n",
                    rel.name,
                    kind,
                    arr_lit(&rel.local_columns),
                    arr_lit(&rel.foreign_columns),
                    shout(&rel.target_table)
                ));
            }
            out.push_str("  ],\n");
        }
        out.push_str("};\n\n");
    }

    out.push_str("/** The unified Powder client: one typed handle per table. */\n");
    // Named queries: one typed method per schema `queries` entry, AOT SQL.
    if !schema.queries.is_empty() {
        out.push_str("/** Custom named queries from powder.schema.json (SQL compiled at generation time). */\n");
        out.push_str("export interface PowderQueries {\n");
        for q in &schema.queries {
            let args: Vec<String> = q
                .params
                .iter()
                .map(|(n, t)| format!("{n}: {}", t.ts_type()))
                .collect();
            let ret = match &q.returns {
                Some(table) => format!("{}[]", pascal(table)),
                None => "Record<string, unknown>[]".to_string(),
            };
            out.push_str(&format!("  /** `{}` */\n", q.source_sql.replace('\n', " ")));
            if args.is_empty() {
                out.push_str(&format!("  {}(): Promise<{ret}>;\n", q.name));
            } else {
                out.push_str(&format!(
                    "  {}(args: {{ {} }}): Promise<{ret}>;\n",
                    q.name,
                    args.join("; ")
                ));
            }
        }
        out.push_str("}\n\n");
    }

    out.push_str("export interface PowderClient {\n");
    for table in &schema.tables {
        out.push_str(&format!(
            "  {}: PowderTable<{ty}, {ty}Include>;\n",
            table.name,
            ty = pascal(&table.name)
        ));
    }
    if !schema.queries.is_empty() {
        out.push_str("  /** Custom named queries: `db.$queries.name({...})`. */\n");
        out.push_str("  $queries: PowderQueries;\n");
    }
    out.push_str(
        "  /** Graft your own table methods: `db.$extend({ posts: { publishAll() {...} } })`. */\n",
    );
    out.push_str("  $extend<E extends PowderExtensions>(ext: E): ExtendedClient<PowderClient, E>;\n");
    out.push_str(
        "  /** Run `fn` inside BEGIN IMMEDIATE; commits on return, rolls back on throw. */\n",
    );
    out.push_str("  $transaction<T>(fn: (tx: PowderClient) => Promise<T>): Promise<T>;\n");
    out.push_str("}\n\n");
    out.push_str("/** Wrap a connected Powder client with the Powder model layer. */\n");
    out.push_str("export function powder(client: Client): PowderClient {\n");
    out.push_str("  const api: PowderClient = {\n");
    for table in &schema.tables {
        out.push_str(&format!(
            "    {}: new PowderTable<{ty}, {ty}Include>(client, {}_META),\n",
            table.name,
            shout(&table.name),
            ty = pascal(&table.name)
        ));
    }
    if !schema.queries.is_empty() {
        out.push_str("    $queries: {\n");
        for q in &schema.queries {
            let order = arr_lit(&q.param_order);
            let meta = match &q.returns {
                Some(table) => format!(", {}_META", shout(table)),
                None => String::new(),
            };
            let cast = match &q.returns {
                Some(table) => format!(" as unknown as Promise<{}[]>", pascal(table)),
                None => String::new(),
            };
            if q.params.is_empty() {
                out.push_str(&format!(
                    "      {}: () => runNamedQuery(client, {}, {order}, {{}}{meta}){cast},\n",
                    q.name,
                    serde_json::to_string(&q.sql).expect("sql is a valid JSON string"),
                ));
            } else {
                out.push_str(&format!(
                    "      {}: (args) => runNamedQuery(client, {}, {order}, args{meta}){cast},\n",
                    q.name,
                    serde_json::to_string(&q.sql).expect("sql is a valid JSON string"),
                ));
            }
        }
        out.push_str("    },\n");
    }
    out.push_str("    $extend: (ext) => extendPowder(api, ext),\n");
    out.push_str("    $transaction: (fn) => client.transaction(() => fn(api)),\n");
    out.push_str("  };\n  return api;\n}\n");
    out
}

/// Generate the Python model module.
pub fn python(schema: &Schema) -> String {
    let mut out = String::new();
    out.push_str("# AUTO-GENERATED by `powder generate` — do not edit.\n");
    out.push_str("# Regenerate after changing powder.schema.json.\n\n");
    out.push_str("from __future__ import annotations\n\n");
    out.push_str("from dataclasses import dataclass, field\nfrom typing import List, Optional\n\n");
    out.push_str("from powder import Client\n");
    out.push_str(
        "from powder.orm import ColumnMeta, PowderTable, RelationMeta, TableMeta, run_named_query\n\n",
    );

    for table in &schema.tables {
        let ty = pascal(&table.name);
        let relations = relations_of(schema, table);
        // kw_only: nullable/relation fields carry defaults, and Python
        // requires defaulted fields to trail positional ones otherwise.
        out.push_str("@dataclass(kw_only=True)\n");
        out.push_str(&format!("class {ty}:\n"));
        out.push_str(&format!("    \"\"\"Row shape of the `{}` table.\"\"\"\n\n", table.name));
        for col in &table.columns {
            let base = col.def.column_type.py_type();
            let py = if col.def.nullable {
                format!("Optional[{base}]{}", column_default_py(col))
            } else {
                base.to_string()
            };
            out.push_str(&format!("    {}: {}\n", col.name, py));
        }
        for rel in &relations {
            let target = pascal(&rel.target_table);
            let ty = match rel.kind {
                RelKind::BelongsTo => format!("Optional[{target}] = None"),
                RelKind::HasMany => format!("List[{target}] = field(default_factory=list)"),
            };
            out.push_str(&format!(
                "    # Loaded via include={{\"{}\": True}}.\n    {}: {}\n",
                rel.name, rel.name, ty
            ));
        }
        out.push('\n');

        out.push_str(&format!("{}_META = TableMeta(\n", shout(&table.name)));
        out.push_str(&format!("    table=\"{}\",\n", table.name));
        out.push_str("    columns=[\n");
        for col in &table.columns {
            out.push_str(&format!(
                "        ColumnMeta(name=\"{}\", type=\"{}\", nullable={}, primary_key={}),\n",
                col.name,
                col.def.column_type.name(),
                if col.def.nullable { "True" } else { "False" },
                if col.def.primary_key { "True" } else { "False" },
            ));
        }
        out.push_str("    ],\n");
        out.push_str(&format!(
            "    select_all=\"SELECT {} FROM {}\",\n",
            column_list(table),
            table.name
        ));
        out.push_str(&format!("    insert=\"{}\",\n", insert_sql(table)));
        out.push_str(&format!(
            "    count_all=\"SELECT COUNT(*) AS n FROM {}\",\n",
            table.name
        ));
        out.push_str(&format!("    delete_all=\"DELETE FROM {}\",\n", table.name));
        out.push_str("    ident={\n");
        for col in &table.columns {
            out.push_str(&format!("        \"{}\": \"{}\",\n", col.name, col.name));
        }
        out.push_str("    },\n");
        if !relations.is_empty() {
            out.push_str("    relations=(\n");
            for rel in &relations {
                let kind = match rel.kind {
                    RelKind::BelongsTo => "belongsTo",
                    RelKind::HasMany => "hasMany",
                };
                out.push_str(&format!(
                    "        RelationMeta(name=\"{}\", kind=\"{}\", local_columns={}, foreign_columns={}, target=lambda: {}_META),\n",
                    rel.name,
                    kind,
                    arr_lit(&rel.local_columns),
                    arr_lit(&rel.foreign_columns),
                    shout(&rel.target_table)
                ));
            }
            out.push_str("    ),\n");
        }
        out.push_str(")\n\n");
    }

    // Relation loading needs the row type of the *target* table.
    out.push_str("_ROW_TYPES = {\n");
    for table in &schema.tables {
        out.push_str(&format!(
            "    \"{}\": {},\n",
            table.name,
            pascal(&table.name)
        ));
    }
    out.push_str("}\n\n");

    // Named queries: one typed method per schema `queries` entry, AOT SQL.
    if !schema.queries.is_empty() {
        out.push_str("class PowderQueries:\n");
        out.push_str("    \"\"\"Custom named queries from powder.schema.json (SQL compiled at generation time).\"\"\"\n\n");
        out.push_str("    def __init__(self, client: Client):\n");
        out.push_str("        self._client = client\n\n");
        for q in &schema.queries {
            // Python surface is snake_case (matching the runtime API); the SQL
            // param dict keys keep the schema's original spelling.
            let kwargs: Vec<String> = q
                .params
                .iter()
                .map(|(n, t)| format!("{}: {}", snake(n), t.py_type()))
                .collect();
            let sig = if kwargs.is_empty() {
                String::new()
            } else {
                format!(", *, {}", kwargs.join(", "))
            };
            let ret = match &q.returns {
                Some(table) => format!("list[{}]", pascal(table)),
                None => "list".to_string(),
            };
            out.push_str(&format!(
                "    async def {}(self{sig}) -> {ret}:\n",
                snake(&q.name)
            ));
            out.push_str(&format!(
                "        \"\"\"``{}``\"\"\"\n",
                q.source_sql.replace('\n', " ")
            ));
            let args_dict = if q.params.is_empty() {
                "{}".to_string()
            } else {
                format!(
                    "{{{}}}",
                    q.params
                        .iter()
                        .map(|(n, _)| format!("\"{n}\": {}", snake(n)))
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            };
            let extra = match &q.returns {
                Some(table) => format!(", meta={}_META, row_type={}", shout(table), pascal(table)),
                None => String::new(),
            };
            out.push_str(&format!(
                "        return await run_named_query(self._client, {}, {}, {args_dict}{extra})\n\n",
                serde_json::to_string(&q.sql).expect("sql is a valid JSON string"),
                arr_lit(&q.param_order),
            ));
        }
    }

    out.push_str("class PowderClient:\n");
    out.push_str("    \"\"\"The unified Powder client: one typed handle per table.\"\"\"\n\n");
    out.push_str("    def __init__(self, client: Client):\n");
    out.push_str("        self._client = client\n");
    for table in &schema.tables {
        out.push_str(&format!(
            "        self.{} = PowderTable(client, {}_META, {}, row_types=_ROW_TYPES)\n",
            table.name,
            shout(&table.name),
            pascal(&table.name)
        ));
    }
    if !schema.queries.is_empty() {
        out.push_str("        #: Custom named queries: ``await db.queries.name(...)``.\n");
        out.push_str("        self.queries = PowderQueries(client)\n");
    }
    out.push('\n');
    out.push_str("    def transaction(self):\n");
    out.push_str("        \"\"\"``async with db.transaction(): ...`` — commit on exit, rollback on error.\"\"\"\n");
    out.push_str("        return self._client.transaction()\n\n");
    out.push_str("def powder(client: Client) -> PowderClient:\n");
    out.push_str("    \"\"\"Wrap a connected Powder client with the Powder model layer.\"\"\"\n");
    out.push_str("    return PowderClient(client)\n");
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::SAMPLE_SCHEMA;

    const COMPOSITE_SCHEMA: &str = r#"{"tables":{
        "orders":{"columns":{
            "id":{"type":"int","primaryKey":true},
            "year":{"type":"int","primaryKey":true}
        }},
        "line_items":{
            "columns":{
                "id":{"type":"int","primaryKey":true},
                "order_id":{"type":"int"},
                "order_year":{"type":"int"}
            },
            "foreignKeys":[
                {"columns":["order_id","order_year"],"references":{"table":"orders","columns":["id","year"]}}
            ]
        }
    }}"#;

    #[test]
    fn typescript_output_contains_aot_sql_and_types() {
        let schema = Schema::parse(SAMPLE_SCHEMA).unwrap();
        let ts = typescript(&schema, "@powder/node");
        assert!(ts.contains("export interface Users {"));
        assert!(ts.contains("score: number | null;"));
        assert!(ts.contains("selectAll: \"SELECT id, name, score, active FROM users\""));
        assert!(ts.contains("insert: \"INSERT INTO users (id, name, score, active) VALUES (?, ?, ?, ?)\""));
        assert!(ts.contains("users: new PowderTable<Users, UsersInclude>(client, USERS_META)"));
        assert!(ts.contains("export type UsersInclude"));
        // belongsTo: posts.user_id -> users.id surfaces as `user`.
        assert!(ts.contains("user?: Users | null;"));
        assert!(ts.contains(
            "{ name: \"user\", kind: \"belongsTo\", localColumns: [\"user_id\"], foreignColumns: [\"id\"], target: () => USERS_META }"
        ));
        // hasMany: users gets a `posts` array relation.
        assert!(ts.contains("posts?: Posts[];"), "{ts}");
        assert!(ts.contains(
            "{ name: \"posts\", kind: \"hasMany\", localColumns: [\"id\"], foreignColumns: [\"user_id\"], target: () => POSTS_META }"
        ), "{ts}");
        assert!(ts.contains("$transaction: (fn) => client.transaction(() => fn(api))"));
    }

    #[test]
    fn named_queries_generate_typed_methods() {
        let schema = Schema::parse(SAMPLE_SCHEMA).unwrap();

        let ts = typescript(&schema, "@powder/node");
        assert!(ts.contains("runNamedQuery"), "{ts}");
        assert!(ts.contains("topUsers(args: { active: boolean; minScore: number }): Promise<Users[]>;"), "{ts}");
        assert!(ts.contains("$queries: PowderQueries;"), "{ts}");
        // AOT: the emitted SQL is positional, not `:named`.
        assert!(ts.contains("active = ? AND score >= ?"), "{ts}");
        assert!(!ts.contains(":minScore\""), "{ts}");
        assert!(ts.contains("[\"active\", \"minScore\"]"), "{ts}");

        let py = python(&schema);
        assert!(py.contains("run_named_query"), "{py}");
        // Python surface is snake_case (docs: `db.queries.top_users(min_score=...)`);
        // the SQL param dict keys keep the schema's camelCase spelling.
        assert!(py.contains("async def top_users(self, *, active: bool, min_score: float) -> list[Users]:"), "{py}");
        assert!(py.contains("self.queries = PowderQueries(client)"), "{py}");
        assert!(py.contains("active = ? AND score >= ?"), "{py}");
        assert!(py.contains("{\"active\": active, \"minScore\": min_score}"), "{py}");
    }

    #[test]
    fn typescript_composite_fk_relation() {
        let schema = Schema::parse(COMPOSITE_SCHEMA).unwrap();
        let ts = typescript(&schema, "@powder/node");
        assert!(ts.contains(
            "localColumns: [\"order_id\", \"order_year\"], foreignColumns: [\"id\", \"year\"]"
        ), "{ts}");
    }

    #[test]
    fn python_output_contains_dataclass_and_meta() {
        let schema = Schema::parse(SAMPLE_SCHEMA).unwrap();
        let py = python(&schema);
        assert!(py.contains("@dataclass(kw_only=True)"));
        assert!(py.contains("class Users:"));
        assert!(py.contains("score: Optional[float] = None"));
        assert!(py.contains("select_all=\"SELECT id, name, score, active FROM users\""));
        assert!(py.contains("row_types=_ROW_TYPES"));
        assert!(py.contains("user: Optional[Users] = None"));
        assert!(py.contains("posts: List[Posts] = field(default_factory=list)"), "{py}");
        assert!(py.contains(
            "RelationMeta(name=\"user\", kind=\"belongsTo\", local_columns=[\"user_id\"], foreign_columns=[\"id\"], target=lambda: USERS_META)"
        ), "{py}");
        assert!(py.contains("def transaction(self):"));
    }
}
