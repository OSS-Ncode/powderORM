//! SQL dialect layer: everything DDL-shaped goes through here so adding a
//! second backend means implementing one trait, not editing call sites.

use crate::schema::{Column, ColumnType, Table};

pub trait SqlDialect {
    /// Physical column type for this dialect.
    fn type_sql(&self, ct: ColumnType) -> &'static str;

    /// `ALTER TABLE ... ADD COLUMN ...`, or an error when the dialect cannot
    /// add this column in place (e.g. a primary-key member).
    fn add_column(&self, table: &Table, col: &Column) -> Result<String, String>;

    /// `CREATE TABLE IF NOT EXISTS ...` for the whole table, including
    /// composite primary keys and foreign-key constraints.
    ///
    /// Shared across dialects — only the per-column type text (`type_sql`)
    /// varies. A single primary key is rendered inline; a composite one
    /// becomes a table-level `PRIMARY KEY (...)` constraint.
    fn create_table(&self, table: &Table) -> String {
        let pk_cols: Vec<&Column> = table.columns.iter().filter(|c| c.def.primary_key).collect();
        let inline_pk = pk_cols.len() == 1;

        let mut parts: Vec<String> = table
            .columns
            .iter()
            .map(|c| {
                let mut s = format!("{} {}", c.name, self.type_sql(c.def.column_type));
                if inline_pk && c.def.primary_key {
                    s.push_str(" PRIMARY KEY");
                } else if !c.def.nullable {
                    s.push_str(" NOT NULL");
                }
                s
            })
            .collect();

        if pk_cols.len() > 1 {
            parts.push(format!(
                "PRIMARY KEY ({})",
                pk_cols.iter().map(|c| c.name.as_str()).collect::<Vec<_>>().join(", ")
            ));
        }
        for fk in &table.foreign_keys {
            parts.push(format!(
                "FOREIGN KEY ({}) REFERENCES {}({})",
                fk.columns.join(", "),
                fk.ref_table,
                fk.ref_columns.join(", ")
            ));
        }

        format!(
            "CREATE TABLE IF NOT EXISTS {} ({})",
            table.name,
            parts.join(", ")
        )
    }
}

/// Parse a dialect name (`--dialect <name>`) into an implementation.
pub fn by_name(name: &str) -> Result<Box<dyn SqlDialect>, String> {
    match name.to_ascii_lowercase().as_str() {
        "sqlite" => Ok(Box::new(Sqlite)),
        "postgres" | "postgresql" | "pg" => Ok(Box::new(Postgres)),
        "mysql" | "mariadb" => Ok(Box::new(MySql)),
        other => Err(format!(
            "unknown dialect `{other}` (expected sqlite, postgres, or mysql)"
        )),
    }
}

/// SQLite: the default backend Powder runs against today.
pub struct Sqlite;

impl SqlDialect for Sqlite {
    fn type_sql(&self, ct: ColumnType) -> &'static str {
        // Matches `ColumnType::sql_type`, which `validate` compares against.
        ct.sql_type()
    }

    fn add_column(&self, table: &Table, col: &Column) -> Result<String, String> {
        if col.def.primary_key {
            return Err(format!(
                "table `{}`: cannot add primary key column `{}` to an existing table (use --rebuild)",
                table.name, col.name
            ));
        }
        let mut ddl = format!(
            "ALTER TABLE {} ADD COLUMN {} {}",
            table.name,
            col.name,
            self.type_sql(col.def.column_type)
        );
        // SQLite requires a default when adding NOT NULL columns to a
        // populated table.
        if !col.def.nullable {
            ddl.push_str(if self.type_sql(col.def.column_type) == "TEXT" {
                " NOT NULL DEFAULT ''"
            } else {
                " NOT NULL DEFAULT 0"
            });
        }
        if let Some(r) = &col.def.references {
            ddl.push_str(&format!(" REFERENCES {}({})", r.table, r.column));
        }
        Ok(ddl)
    }
}

/// PostgreSQL: DDL generation only. A live Postgres runtime needs a driver +
/// server; the dialect here produces standards-compliant `CREATE TABLE` /
/// `ALTER TABLE` so `powder ddl --dialect postgres` and a future Postgres
/// backend share one source of truth.
pub struct Postgres;

impl SqlDialect for Postgres {
    fn type_sql(&self, ct: ColumnType) -> &'static str {
        match ct {
            ColumnType::Int => "BIGINT",
            ColumnType::Float => "DOUBLE PRECISION",
            ColumnType::Text => "TEXT",
            ColumnType::Bool => "BOOLEAN",
        }
    }

    fn add_column(&self, table: &Table, col: &Column) -> Result<String, String> {
        if col.def.primary_key {
            return Err(format!(
                "table `{}`: cannot add primary key column `{}` to an existing table",
                table.name, col.name
            ));
        }
        let mut ddl = format!(
            "ALTER TABLE {} ADD COLUMN {} {}",
            table.name,
            col.name,
            self.type_sql(col.def.column_type)
        );
        if !col.def.nullable {
            ddl.push_str(if self.type_sql(col.def.column_type) == "TEXT" {
                " NOT NULL DEFAULT ''"
            } else if self.type_sql(col.def.column_type) == "BOOLEAN" {
                " NOT NULL DEFAULT false"
            } else {
                " NOT NULL DEFAULT 0"
            });
        }
        if let Some(r) = &col.def.references {
            ddl.push_str(&format!(" REFERENCES {}({})", r.table, r.column));
        }
        Ok(ddl)
    }
}

/// MySQL / MariaDB: DDL generation. Text primary-key members get a length
/// prefix requirement, so schema-level TEXT is rendered as `VARCHAR(255)`
/// when it participates in a key.
pub struct MySql;

impl SqlDialect for MySql {
    fn type_sql(&self, ct: ColumnType) -> &'static str {
        match ct {
            ColumnType::Int => "BIGINT",
            ColumnType::Float => "DOUBLE",
            ColumnType::Text => "TEXT",
            ColumnType::Bool => "TINYINT(1)",
        }
    }

    fn create_table(&self, table: &Table) -> String {
        let pk_cols: Vec<&Column> = table.columns.iter().filter(|c| c.def.primary_key).collect();
        let inline_pk = pk_cols.len() == 1;

        let mut parts: Vec<String> = table
            .columns
            .iter()
            .map(|c| {
                // MySQL cannot index bare TEXT — key members become VARCHAR.
                let is_key = c.def.primary_key
                    || table.foreign_keys.iter().any(|fk| fk.columns.contains(&c.name));
                let ty = if c.def.column_type == ColumnType::Text && is_key {
                    "VARCHAR(255)"
                } else {
                    self.type_sql(c.def.column_type)
                };
                let mut s = format!("{} {}", c.name, ty);
                if inline_pk && c.def.primary_key {
                    s.push_str(" PRIMARY KEY");
                } else if !c.def.nullable {
                    s.push_str(" NOT NULL");
                }
                s
            })
            .collect();

        if pk_cols.len() > 1 {
            parts.push(format!(
                "PRIMARY KEY ({})",
                pk_cols.iter().map(|c| c.name.as_str()).collect::<Vec<_>>().join(", ")
            ));
        }
        for fk in &table.foreign_keys {
            parts.push(format!(
                "FOREIGN KEY ({}) REFERENCES {}({})",
                fk.columns.join(", "),
                fk.ref_table,
                fk.ref_columns.join(", ")
            ));
        }

        format!(
            "CREATE TABLE IF NOT EXISTS {} ({})",
            table.name,
            parts.join(", ")
        )
    }

    fn add_column(&self, table: &Table, col: &Column) -> Result<String, String> {
        if col.def.primary_key {
            return Err(format!(
                "table `{}`: cannot add primary key column `{}` to an existing table",
                table.name, col.name
            ));
        }
        let mut ddl = format!(
            "ALTER TABLE {} ADD COLUMN {} {}",
            table.name,
            col.name,
            self.type_sql(col.def.column_type)
        );
        if !col.def.nullable {
            ddl.push_str(match self.type_sql(col.def.column_type) {
                "TEXT" => " NOT NULL",
                _ => " NOT NULL DEFAULT 0",
            });
        }
        if let Some(r) = &col.def.references {
            ddl.push_str(&format!(", ADD FOREIGN KEY ({}) REFERENCES {}({})", col.name, r.table, r.column));
        }
        Ok(ddl)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::Schema;

    #[test]
    fn composite_pk_becomes_table_constraint() {
        let schema = Schema::parse(
            r#"{"tables":{"m":{"columns":{
                "a":{"type":"int","primaryKey":true},
                "b":{"type":"text","primaryKey":true},
                "v":{"type":"float","nullable":true}
            }}}}"#,
        )
        .unwrap();
        let ddl = Sqlite.create_table(&schema.tables[0]);
        assert_eq!(
            ddl,
            "CREATE TABLE IF NOT EXISTS m (a INTEGER NOT NULL, b TEXT NOT NULL, v REAL, PRIMARY KEY (a, b))"
        );
    }

    #[test]
    fn foreign_keys_render_as_constraints() {
        let schema = Schema::parse(
            r#"{"tables":{
                "users":{"columns":{"id":{"type":"int","primaryKey":true}}},
                "posts":{"columns":{
                    "id":{"type":"int","primaryKey":true},
                    "user_id":{"type":"int","references":{"table":"users","column":"id"}}
                }}
            }}"#,
        )
        .unwrap();
        let posts = schema.tables.iter().find(|t| t.name == "posts").unwrap();
        let ddl = Sqlite.create_table(posts);
        assert_eq!(
            ddl,
            "CREATE TABLE IF NOT EXISTS posts (id INTEGER PRIMARY KEY, user_id INTEGER NOT NULL, FOREIGN KEY (user_id) REFERENCES users(id))"
        );
    }

    #[test]
    fn postgres_maps_types_and_keeps_constraints() {
        let schema = Schema::parse(
            r#"{"tables":{
                "users":{"columns":{
                    "id":{"type":"int","primaryKey":true},
                    "name":{"type":"text"},
                    "score":{"type":"float","nullable":true},
                    "active":{"type":"bool"}
                }},
                "posts":{"columns":{
                    "id":{"type":"int","primaryKey":true},
                    "user_id":{"type":"int","references":{"table":"users","column":"id"}}
                }}
            }}"#,
        )
        .unwrap();
        let users = schema.tables.iter().find(|t| t.name == "users").unwrap();
        assert_eq!(
            Postgres.create_table(users),
            "CREATE TABLE IF NOT EXISTS users (id BIGINT PRIMARY KEY, name TEXT NOT NULL, score DOUBLE PRECISION, active BOOLEAN NOT NULL)"
        );
        let posts = schema.tables.iter().find(|t| t.name == "posts").unwrap();
        assert!(Postgres
            .create_table(posts)
            .contains("FOREIGN KEY (user_id) REFERENCES users(id)"));
    }

    #[test]
    fn postgres_composite_pk() {
        let schema = Schema::parse(
            r#"{"tables":{"m":{"columns":{
                "a":{"type":"int","primaryKey":true},
                "b":{"type":"text","primaryKey":true}
            }}}}"#,
        )
        .unwrap();
        assert_eq!(
            Postgres.create_table(&schema.tables[0]),
            "CREATE TABLE IF NOT EXISTS m (a BIGINT NOT NULL, b TEXT NOT NULL, PRIMARY KEY (a, b))"
        );
    }

    #[test]
    fn dialect_by_name() {
        assert!(by_name("sqlite").is_ok());
        assert!(by_name("postgres").is_ok());
        assert!(by_name("pg").is_ok());
        assert!(by_name("mysql").is_ok());
        assert!(by_name("mariadb").is_ok());
        assert!(by_name("oracle").is_err());
        assert!(by_name("mssql").is_err());
    }

    #[test]
    fn mysql_maps_types_and_varchars_key_text() {
        let schema = Schema::parse(
            r#"{"tables":{
                "users":{"columns":{
                    "code":{"type":"text","primaryKey":true},
                    "name":{"type":"text"},
                    "score":{"type":"float","nullable":true},
                    "active":{"type":"bool"}
                }}
            }}"#,
        )
        .unwrap();
        assert_eq!(
            MySql.create_table(&schema.tables[0]),
            "CREATE TABLE IF NOT EXISTS users (code VARCHAR(255) PRIMARY KEY, name TEXT NOT NULL, score DOUBLE, active TINYINT(1) NOT NULL)"
        );
    }

    #[test]
    fn mysql_composite_pk_and_fk() {
        let schema = Schema::parse(
            r#"{"tables":{
                "users":{"columns":{"id":{"type":"int","primaryKey":true}}},
                "posts":{"columns":{
                    "id":{"type":"int","primaryKey":true},
                    "user_id":{"type":"int","references":{"table":"users","column":"id"}}
                }}
            }}"#,
        )
        .unwrap();
        let posts = schema.tables.iter().find(|t| t.name == "posts").unwrap();
        let ddl = MySql.create_table(posts);
        assert!(ddl.contains("user_id BIGINT NOT NULL"), "{ddl}");
        assert!(ddl.contains("FOREIGN KEY (user_id) REFERENCES users(id)"), "{ddl}");
    }
}
