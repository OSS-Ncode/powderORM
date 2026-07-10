//! Command dispatch for the `powder` binary, factored out of `main.rs` so the
//! whole surface is unit-testable: `run` takes explicit args + a working
//! directory and returns the text it would print instead of writing to stdout.

use std::fmt::Write as _;
use std::path::Path;

use crate::schema::{Schema, SAMPLE_SCHEMA};
use crate::{codegen, db, dialect, jsonschema, scaffold};

pub const USAGE: &str = "\
powder — Powder ORM CLI

USAGE:
  powder new <dir>                              # scaffold a new Powder project
  powder init
  powder generate [--schema powder.schema.json] [--ts <out.ts>] [--py <out.py>] [--ts-import <module>]
  powder ddl      [--schema powder.schema.json] [--dialect sqlite|postgres|mysql]
  powder migrate  --db <url> [--schema powder.schema.json] [--rebuild]
  powder validate --db <url> [--schema powder.schema.json]
  powder seed     --db <url> --file <seed.json|seed.sql>

`migrate` is additive (CREATE TABLE / ADD COLUMN). With --rebuild, tables
whose live shape drifted destructively (dropped columns, type or key changes)
are rebuilt in place, preserving data in surviving columns.
`ddl` prints CREATE TABLE statements for the chosen dialect (default sqlite).

Database URLs accept the same forms as the Powder client:
  sqlite::memory: | sqlite://path/to.db | path/to.db
";

fn flag(args: &[String], name: &str) -> Option<String> {
    args.iter()
        .position(|a| a == name)
        .and_then(|i| args.get(i + 1))
        .cloned()
}

fn load_schema(args: &[String], cwd: &Path) -> Result<Schema, String> {
    let path = flag(args, "--schema").unwrap_or_else(|| "powder.schema.json".into());
    let resolved = cwd.join(&path);
    let json = std::fs::read_to_string(&resolved)
        .map_err(|e| format!("cannot read `{path}`: {e}"))?;
    Schema::parse(&json)
}

/// Execute one CLI invocation. `cwd` anchors every relative path (schema,
/// output files, `powder init`'s artifacts). Returns the stdout text.
pub fn run(args: &[String], cwd: &Path) -> Result<String, String> {
    let mut out = String::new();
    match args.first().map(String::as_str) {
        Some("new") => {
            let dir = args
                .get(1)
                .filter(|a| !a.starts_with("--"))
                .ok_or("new: a project directory is required")?;
            let written = scaffold::scaffold(&cwd.join(dir).to_string_lossy())?;
            for f in &written {
                let _ = writeln!(out, "wrote {f}");
            }
            let _ = writeln!(
                out,
                "\nnext steps:\n  cd {dir}\n  npm install\n  npm run migrate && npm run seed && npm run demo"
            );
            Ok(out)
        }
        Some("init") => {
            let path = cwd.join("powder.schema.json");
            if path.exists() {
                return Err("`powder.schema.json` already exists".into());
            }
            std::fs::write(&path, SAMPLE_SCHEMA).map_err(|e| e.to_string())?;
            let _ = writeln!(out, "wrote powder.schema.json");
            // Editor support: the schema-of-the-schema gives VS Code & co.
            // completion + validation while editing powder.schema.json.
            std::fs::write(
                cwd.join("powder.schema.schema.json"),
                jsonschema::SCHEMA_OF_SCHEMA,
            )
            .map_err(|e| e.to_string())?;
            let _ = writeln!(out, "wrote powder.schema.schema.json (editor autocompletion)");
            Ok(out)
        }
        Some("generate") => {
            let schema = load_schema(&args, cwd)?;
            let mut wrote = false;
            if let Some(ts_out) = flag(&args, "--ts") {
                let import = flag(&args, "--ts-import").unwrap_or_else(|| "@powder/node".into());
                std::fs::write(cwd.join(&ts_out), codegen::typescript(&schema, &import))
                    .map_err(|e| e.to_string())?;
                let _ = writeln!(out, "wrote {ts_out}");
                wrote = true;
            }
            if let Some(py_out) = flag(&args, "--py") {
                std::fs::write(cwd.join(&py_out), codegen::python(&schema))
                    .map_err(|e| e.to_string())?;
                let _ = writeln!(out, "wrote {py_out}");
                wrote = true;
            }
            if !wrote {
                return Err("generate: pass --ts <out.ts> and/or --py <out.py>".into());
            }
            Ok(out)
        }
        Some("ddl") => {
            let schema = load_schema(&args, cwd)?;
            let name = flag(&args, "--dialect").unwrap_or_else(|| "sqlite".into());
            let d = dialect::by_name(&name)?;
            for table in schema.tables_in_dependency_order() {
                let _ = writeln!(out, "{};", d.create_table(table));
            }
            Ok(out)
        }
        Some("migrate") => {
            let url = flag(&args, "--db").ok_or("migrate: --db <url> is required")?;
            let schema = load_schema(&args, cwd)?;
            let mut conn = db::open_at(&url, cwd)?;
            let rebuild = args.iter().any(|a| a == "--rebuild");
            let applied = if rebuild {
                db::migrate_rebuild(&mut conn, &schema)?
            } else {
                db::migrate(&mut conn, &schema)?
            };
            if applied.is_empty() {
                let _ = writeln!(out, "database already up to date");
            } else {
                for ddl in &applied {
                    let _ = writeln!(out, "applied: {ddl}");
                }
            }
            Ok(out)
        }
        Some("validate") => {
            let url = flag(&args, "--db").ok_or("validate: --db <url> is required")?;
            let schema = load_schema(&args, cwd)?;
            let mut conn = db::open_at(&url, cwd)?;
            let problems = db::validate(&mut conn, &schema)?;
            if problems.is_empty() {
                let _ = writeln!(out, "schema and database are in sync");
                Ok(out)
            } else {
                let mut msg = String::new();
                for p in &problems {
                    let _ = writeln!(msg, "mismatch: {p}");
                }
                let _ = write!(msg, "{} schema mismatch(es) found", problems.len());
                Err(msg)
            }
        }
        Some("seed") => {
            let url = flag(&args, "--db").ok_or("seed: --db <url> is required")?;
            let file =
                flag(&args, "--file").ok_or("seed: --file <seed.json|seed.sql> is required")?;
            let contents = std::fs::read_to_string(cwd.join(&file))
                .map_err(|e| format!("cannot read `{file}`: {e}"))?;
            let mut conn = db::open_at(&url, cwd)?;
            let n = db::seed(&mut conn, &file, &contents)?;
            let _ = writeln!(out, "seeded {n} row(s) from {file}");
            Ok(out)
        }
        Some("--help" | "-h" | "help") | None => {
            out.push_str(USAGE);
            Ok(out)
        }
        Some(other) => Err(format!("unknown command `{other}`\n\n{USAGE}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    fn tmpdir(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("powder-cli-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn help_and_unknown_command() {
        let cwd = tmpdir("help");
        assert!(run(&args(&["--help"]), &cwd).unwrap().contains("USAGE"));
        assert!(run(&[], &cwd).unwrap().contains("USAGE"));
        let err = run(&args(&["frobnicate"]), &cwd).unwrap_err();
        assert!(err.contains("unknown command `frobnicate`"));
        std::fs::remove_dir_all(&cwd).unwrap();
    }

    #[test]
    fn init_writes_schema_and_editor_schema_once() {
        let cwd = tmpdir("init");
        let out = run(&args(&["init"]), &cwd).unwrap();
        assert!(out.contains("wrote powder.schema.json"));
        assert!(cwd.join("powder.schema.schema.json").exists());
        // Second init refuses to overwrite.
        assert!(run(&args(&["init"]), &cwd).unwrap_err().contains("already exists"));
        std::fs::remove_dir_all(&cwd).unwrap();
    }

    #[test]
    fn full_lifecycle_generate_ddl_migrate_seed_validate() {
        let cwd = tmpdir("cycle");
        run(&args(&["init"]), &cwd).unwrap();

        // generate requires at least one output
        assert!(run(&args(&["generate"]), &cwd).unwrap_err().contains("--ts"));
        let out = run(&args(&["generate", "--ts", "m.ts", "--py", "m.py"]), &cwd).unwrap();
        assert!(out.contains("wrote m.ts") && out.contains("wrote m.py"));
        assert!(cwd.join("m.ts").exists() && cwd.join("m.py").exists());

        // ddl in two dialects
        assert!(run(&args(&["ddl"]), &cwd).unwrap().contains("CREATE TABLE"));
        assert!(run(&args(&["ddl", "--dialect", "postgres"]), &cwd)
            .unwrap()
            .contains("BIGINT"));

        // migrate + seed + validate against a real file DB
        let migrate = run(&args(&["migrate", "--db", "app.db"]), &cwd).unwrap();
        assert!(migrate.contains("applied: CREATE TABLE"));
        let again = run(&args(&["migrate", "--db", "app.db"]), &cwd).unwrap();
        assert!(again.contains("already up to date"));

        std::fs::write(
            cwd.join("seed.json"),
            r#"{"users": [{"id": 1, "name": "a", "score": 1.5, "active": true}]}"#,
        )
        .unwrap();
        let seeded = run(&args(&["seed", "--db", "app.db", "--file", "seed.json"]), &cwd).unwrap();
        assert!(seeded.contains("seeded 1 row(s)"));

        assert!(run(&args(&["validate", "--db", "app.db"]), &cwd)
            .unwrap()
            .contains("in sync"));

        // missing required flags surface as errors
        assert!(run(&args(&["migrate"]), &cwd).unwrap_err().contains("--db"));
        assert!(run(&args(&["seed", "--db", "app.db"]), &cwd)
            .unwrap_err()
            .contains("--file"));
        std::fs::remove_dir_all(&cwd).unwrap();
    }

    #[test]
    fn new_scaffolds_into_cwd_relative_dir() {
        let cwd = tmpdir("new");
        let out = run(&args(&["new", "proj"]), &cwd).unwrap();
        assert!(out.contains("next steps"));
        assert!(cwd.join("proj").join("powder.schema.json").exists());
        assert!(run(&args(&["new"]), &cwd).unwrap_err().contains("directory is required"));
        std::fs::remove_dir_all(&cwd).unwrap();
    }

    #[test]
    fn validate_reports_drift() {
        let cwd = tmpdir("drift");
        run(&args(&["init"]), &cwd).unwrap();
        run(&args(&["migrate", "--db", "app.db"]), &cwd).unwrap();
        // Drop a column behind the schema's back.
        let mut conn = db::open_at("app.db", &cwd).unwrap();
        conn.execute_batch("ALTER TABLE users DROP COLUMN score").unwrap();
        drop(conn);
        let err = run(&args(&["validate", "--db", "app.db"]), &cwd).unwrap_err();
        assert!(err.contains("mismatch"), "{err}");
        std::fs::remove_dir_all(&cwd).unwrap();
    }
}
