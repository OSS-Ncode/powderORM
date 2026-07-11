//! Command dispatch for the `powder` binary, factored out of `main.rs` so the
//! whole surface is unit-testable: `run` takes explicit args + a working
//! directory and returns the text it would print instead of writing to stdout.

use std::fmt::Write as _;
use std::path::Path;

use crate::schema::{Schema, SAMPLE_SCHEMA};
use crate::{codegen, db, dialect, jsonschema, scaffold, setup, tools};

pub const USAGE: &str = "\
powder — Powder ORM CLI

USAGE:

프로젝트:
  powder new <dir>                              # scaffold a new Powder project
  powder init
  powder setup    [--db <url>] [--langs ts,python,...] [--ai-endpoint <url>] [--add <lang>] [--show]
  powder generate [--schema powder.schema.json] [--ts [out.ts]] [--py [out.py]] [--ts-import <module>]
                  # 경로 생략 시 models.ts / powder_models.py

스키마 · 마이그레이션:
  powder ddl        [--schema powder.schema.json] [--dialect sqlite|postgres|mysql|mssql]
  powder migrate    --db <url> [--schema powder.schema.json] [--rebuild] [--dry-run]
  powder validate   --db <url> [--schema powder.schema.json]
  powder seed       --db <url> --file <seed.json|seed.sql>
  powder introspect --db <url> [--out powder.schema.json] [--loose]   # 살아있는 DB → 스키마 파일

라이브 데이터:
  powder query    --db <url> \"SELECT ...\" [--format table|json|csv]
  powder exec     --db <url> \"UPDATE ...\"
  powder tables   --db <url>
  powder describe --db <url> <table>
  powder dump     --db <url> [--tables a,b] [--out seed.json]         # powder seed로 재주입 가능

대시보드 · AI:
  powder studio   --db <url> [--port 5877] [--host 127.0.0.1] [--readonly]
  powder ai       \"하고 싶은 작업\" [--db <url>] [--run]                 # 자연어 → SQL (설정의 AI 엔드포인트 사용)

`migrate`는 추가만 합니다 (CREATE TABLE / ADD COLUMN). --rebuild는 파괴적
드리프트가 생긴 테이블을 데이터 보존 재빌드하고, --dry-run은 실행 없이
계획만 출력합니다. `studio`는 모바일에서도 동작하는 웹 대시보드를 띄우고
토큰 초대(읽기전용/읽기쓰기)로 공유할 수 있습니다.

Database URLs (모든 바인딩과 동일):
  sqlite::memory: | sqlite://path | path | postgres:// | mysql:// | mssql:// | libsql://
";

fn flag(args: &[String], name: &str) -> Option<String> {
    args.iter()
        .position(|a| a == name)
        .and_then(|i| args.get(i + 1))
        .cloned()
}

/// A value-taking flag whose value may be omitted: `--py out.py` uses the
/// explicit path, a bare `--py` (end of args, or followed by another flag)
/// falls back to `default`. Absent flag → None.
fn output_flag(args: &[String], name: &str, default: &str) -> Option<String> {
    let i = args.iter().position(|a| a == name)?;
    match args.get(i + 1) {
        Some(v) if !v.starts_with("--") => Some(v.clone()),
        _ => Some(default.to_string()),
    }
}

/// Positional arguments after the command word: everything that is neither a
/// `--flag` nor the value of a value-taking flag.
fn positionals(args: &[String]) -> Vec<String> {
    const VALUE_FLAGS: [&str; 16] = [
        "--db", "--schema", "--file", "--format", "--out", "--tables", "--dialect", "--port",
        "--host", "--ts", "--py", "--ts-import", "--add", "--langs", "--ai-endpoint", "--ai-model",
    ];
    let mut out = Vec::new();
    let mut skip = false;
    for a in &args[1..] {
        if skip {
            skip = false;
            continue;
        }
        if a.starts_with("--") {
            skip = VALUE_FLAGS.contains(&a.as_str());
            continue;
        }
        out.push(a.clone());
    }
    out
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
                "\nnext steps:\n  cd {dir}\n  npm install\n  npm run migrate && npm run seed && npm run start"
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
            let schema = load_schema(args, cwd)?;
            let mut wrote = false;
            if let Some(ts_out) = output_flag(args, "--ts", "models.ts") {
                let import = flag(args, "--ts-import").unwrap_or_else(|| "@powder/node".into());
                std::fs::write(cwd.join(&ts_out), codegen::typescript(&schema, &import))
                    .map_err(|e| e.to_string())?;
                let _ = writeln!(out, "wrote {ts_out}");
                wrote = true;
            }
            if let Some(py_out) = output_flag(args, "--py", "powder_models.py") {
                std::fs::write(cwd.join(&py_out), codegen::python(&schema))
                    .map_err(|e| e.to_string())?;
                let _ = writeln!(out, "wrote {py_out}");
                wrote = true;
            }
            if !wrote {
                return Err("generate: pass --ts [out.ts] and/or --py [out.py]".into());
            }
            Ok(out)
        }
        Some("ddl") => {
            let schema = load_schema(args, cwd)?;
            let name = flag(args, "--dialect").unwrap_or_else(|| "sqlite".into());
            let d = dialect::by_name(&name)?;
            for table in schema.tables_in_dependency_order() {
                let _ = writeln!(out, "{};", d.create_table(table));
            }
            Ok(out)
        }
        Some("migrate") => {
            let url = flag(args, "--db").ok_or("migrate: --db <url> is required")?;
            let schema = load_schema(args, cwd)?;
            let mut conn = db::open_at(&url, cwd)?;
            let rebuild = args.iter().any(|a| a == "--rebuild");
            let dry_run = args.iter().any(|a| a == "--dry-run");
            if dry_run && rebuild {
                return Err("migrate: --dry-run cannot be combined with --rebuild".into());
            }
            let applied = if dry_run {
                db::migrate_plan(&mut conn, &schema)?
            } else if rebuild {
                db::migrate_rebuild(&mut conn, &schema)?
            } else {
                db::migrate(&mut conn, &schema)?
            };
            let verb = if dry_run { "would apply" } else { "applied" };
            if applied.is_empty() {
                let _ = writeln!(out, "database already up to date");
            } else {
                for ddl in &applied {
                    let _ = writeln!(out, "{verb}: {ddl}");
                }
            }
            Ok(out)
        }
        Some("validate") => {
            let url = flag(args, "--db").ok_or("validate: --db <url> is required")?;
            let schema = load_schema(args, cwd)?;
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
            let url = flag(args, "--db").ok_or("seed: --db <url> is required")?;
            let file =
                flag(args, "--file").ok_or("seed: --file <seed.json|seed.sql> is required")?;
            let contents = std::fs::read_to_string(cwd.join(&file))
                .map_err(|e| format!("cannot read `{file}`: {e}"))?;
            let mut conn = db::open_at(&url, cwd)?;
            let n = db::seed(&mut conn, &file, &contents)?;
            let _ = writeln!(out, "seeded {n} row(s) from {file}");
            Ok(out)
        }
        Some("query") | Some("exec") => {
            let is_query = args[0] == "query";
            let url = flag(args, "--db").ok_or("query/exec: --db <url> is required")?;
            let sql = positionals(args)
                .into_iter()
                .next()
                .ok_or("query/exec: pass the SQL as an argument (quote it)")?;
            let engine = tools::Engine::connect(&url, cwd)?;
            if is_query {
                let batch = engine.query(&sql)?;
                let format = flag(args, "--format").unwrap_or_else(|| "table".into());
                out.push_str(&tools::render(&batch, &format)?);
            } else {
                let n = engine.execute(&sql)?;
                let _ = writeln!(out, "affected: {n}");
            }
            Ok(out)
        }
        Some("tables") => {
            let url = flag(args, "--db").ok_or("tables: --db <url> is required")?;
            let engine = tools::Engine::connect(&url, cwd)?;
            for t in tools::table_names(&engine)? {
                let _ = writeln!(out, "{t}");
            }
            Ok(out)
        }
        Some("describe") => {
            let url = flag(args, "--db").ok_or("describe: --db <url> is required")?;
            let table = positionals(args)
                .into_iter()
                .next()
                .ok_or("describe: pass a table name")?;
            let engine = tools::Engine::connect(&url, cwd)?;
            out.push_str(&tools::describe(&engine, &table)?);
            Ok(out)
        }
        Some("dump") => {
            let url = flag(args, "--db").ok_or("dump: --db <url> is required")?;
            let engine = tools::Engine::connect(&url, cwd)?;
            let only = flag(args, "--tables")
                .map(|t| t.split(',').map(|s| s.trim().to_string()).collect::<Vec<_>>());
            let doc = tools::dump(&engine, only)?;
            match flag(args, "--out") {
                Some(path) => {
                    std::fs::write(cwd.join(&path), doc).map_err(|e| e.to_string())?;
                    let _ = writeln!(out, "wrote {path}");
                }
                None => out.push_str(&doc),
            }
            Ok(out)
        }
        Some("introspect") => {
            let url = flag(args, "--db").ok_or("introspect: --db <url> is required")?;
            let engine = tools::Engine::connect(&url, cwd)?;
            let loose = args.iter().any(|a| a == "--loose");
            let doc = tools::introspect(&engine, loose)?;
            match flag(args, "--out") {
                Some(path) => {
                    std::fs::write(cwd.join(&path), doc).map_err(|e| e.to_string())?;
                    let _ = writeln!(out, "wrote {path}");
                }
                None => {
                    out.push_str(&doc);
                    out.push('\n');
                }
            }
            Ok(out)
        }
        Some("setup") => setup::run(&args[1..], cwd),
        #[cfg(feature = "studio")]
        Some("ai") => {
            use powder_studio::ai;
            let task = positionals(args)
                .into_iter()
                .next()
                .ok_or("ai: 하고 싶은 작업을 적어주세요 — powder ai \"활성 사용자 수를 세어줘\"")?;
            let doc = crate::config::load_config(cwd)?;
            let cfg = ai::ai_config(&doc).ok_or(
                "ai: 엔드포인트가 설정되지 않았습니다 — `powder setup --ai-endpoint <url>` 로 등록하세요",
            )?;
            // 스키마 컨텍스트: --db가 있으면 살아있는 DB에서, 없으면 스키마 파일에서.
            let (summary, flavor, engine) = match flag(args, "--db") {
                Some(url) => {
                    let engine = tools::Engine::connect(&url, cwd)?;
                    (tools::schema_summary(&engine)?, engine.flavor(), Some(engine))
                }
                None => {
                    let schema = load_schema(args, cwd)?;
                    let mut s = String::new();
                    for t in &schema.tables {
                        let cols: Vec<String> = t
                            .columns
                            .iter()
                            .map(|c| format!("{} {:?}", c.name, c.def.column_type))
                            .collect();
                        let _ = writeln!(s, "- {}({})", t.name, cols.join(", "));
                    }
                    (s, "sqlite", None)
                }
            };
            let (system, user) = ai::build_prompt(&summary, flavor, &task);
            let sql = ai::generate(&cfg, &system, &user)?;
            let _ = writeln!(out, "{sql}");
            if args.iter().any(|a| a == "--run") {
                let engine = engine.ok_or("ai --run: --db <url>이 필요합니다")?;
                if sql.trim_start().to_ascii_uppercase().starts_with("SELECT") {
                    let batch = engine.query(&sql)?;
                    out.push('\n');
                    out.push_str(&tools::render(&batch, "table")?);
                } else {
                    let n = engine.execute(&sql)?;
                    let _ = writeln!(out, "\naffected: {n}");
                }
            }
            Ok(out)
        }
        #[cfg(not(feature = "studio"))]
        Some("ai") => {
            Err("이 빌드에는 AI 기능이 포함되지 않았습니다 — `--features studio`로 빌드하세요".into())
        }
        Some("studio") => {
            #[cfg(feature = "studio")]
            {
                let url = flag(args, "--db").ok_or("studio: --db <url> is required")?;
                let port: u16 = flag(args, "--port")
                    .map(|p| p.parse().map_err(|_| format!("studio: bad port `{p}`")))
                    .transpose()?
                    .unwrap_or(5877);
                let host = flag(args, "--host").unwrap_or_else(|| "127.0.0.1".into());
                let readonly = args.iter().any(|a| a == "--readonly");
                let ai = powder_studio::ai::ai_config(&crate::config::load_config(cwd)?);
                powder_studio::studio::serve(&url, cwd, &host, port, readonly, ai)?;
                Ok(out)
            }
            #[cfg(not(feature = "studio"))]
            Err("이 빌드에는 studio가 포함되지 않았습니다 — `--features studio`로 빌드하세요".into())
        }
        Some("--version" | "-V" | "version") => {
            let _ = writeln!(out, "powder {}", env!("CARGO_PKG_VERSION"));
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

        // bare --ts/--py fall back to the documented default filenames,
        // including when the bare flag is followed by another flag
        let out = run(&args(&["generate", "--py"]), &cwd).unwrap();
        assert!(out.contains("wrote powder_models.py"), "{out}");
        assert!(cwd.join("powder_models.py").exists());
        let out = run(&args(&["generate", "--py", "--ts"]), &cwd).unwrap();
        assert!(out.contains("wrote powder_models.py") && out.contains("wrote models.ts"), "{out}");
        assert!(cwd.join("models.ts").exists());

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
