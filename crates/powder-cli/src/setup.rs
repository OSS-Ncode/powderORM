//! `powder setup` — 프로젝트 구성 선택기. 사용할 언어 바인딩·데이터베이스·
//! AI 쿼리 생성 플러그인을 골라 `powder.config.json`에 기록하고, 코드젠이
//! 있는 언어(ts/python)는 즉시 모델을 생성한다. 나중에 `--add <lang>`으로
//! 처음에 고르지 않은 언어를 바로 추가할 수 있다.

use std::fmt::Write as _;
use std::io::Write as _;
use std::path::Path;

use serde_json::{json, Value as J};

use crate::config::{load_config, save_config, CONFIG_FILE};
use crate::codegen;
use crate::schema::Schema;

pub const LANGUAGES: [&str; 9] = [
    "ts", "python", "java", "kotlin", "go", "c", "cpp", "csharp", "rust",
];

fn lang_note(lang: &str) -> &'static str {
    match lang {
        "ts" => "powder_models.ts 생성됨 — `@powder/node`와 함께 사용",
        "python" => "powder_models.py 생성됨 — `pip install powder`와 함께 사용",
        "java" => "crates/powder-java (JNI) — README의 빌드 절차 참고",
        "kotlin" => "bindings/kotlin — Java 클래스 + Powder.kt, README 참고",
        "go" => "bindings/go — powder_ffi 동적 라이브러리 필요",
        "c" => "bindings/c — powder.h + powder_ffi",
        "cpp" => "bindings/cpp — 헤더 하나(powder.hpp) + powder_ffi",
        "csharp" => "bindings/csharp — Powder 프로젝트 참조 + POWDER_LIB",
        "rust" => "powder-core 크레이트를 직접 사용",
        _ => "",
    }
}

fn ask(prompt: &str) -> String {
    print!("{prompt}");
    let _ = std::io::stdout().flush();
    let mut line = String::new();
    let _ = std::io::stdin().read_line(&mut line);
    line.trim().to_string()
}

fn parse_langs(raw: &str) -> Result<Vec<String>, String> {
    let mut out = Vec::new();
    for l in raw.split(',').map(str::trim).filter(|s| !s.is_empty()) {
        let norm = match l.to_ascii_lowercase().as_str() {
            "typescript" | "node" | "js" => "ts".to_string(),
            "py" => "python".to_string(),
            "c++" => "cpp".to_string(),
            "c#" | "cs" | "dotnet" => "csharp".to_string(),
            other => other.to_string(),
        };
        if !LANGUAGES.contains(&norm.as_str()) {
            return Err(format!(
                "unknown language `{l}` (expected one of: {})",
                LANGUAGES.join(", ")
            ));
        }
        if !out.contains(&norm) {
            out.push(norm);
        }
    }
    Ok(out)
}

/// Run codegen for languages that have it; report what to do for the rest.
fn install_langs(out: &mut String, langs: &[String], cwd: &Path) -> Result<(), String> {
    let schema_path = cwd.join("powder.schema.json");
    let schema = if schema_path.exists() {
        let text = std::fs::read_to_string(&schema_path).map_err(|e| e.to_string())?;
        Some(Schema::parse(&text)?)
    } else {
        None
    };
    for lang in langs {
        match (lang.as_str(), &schema) {
            ("ts", Some(s)) => {
                std::fs::write(cwd.join("powder_models.ts"), codegen::typescript(s, "@powder/node"))
                    .map_err(|e| e.to_string())?;
            }
            ("python", Some(s)) => {
                std::fs::write(cwd.join("powder_models.py"), codegen::python(s))
                    .map_err(|e| e.to_string())?;
            }
            _ => {}
        }
        let _ = writeln!(out, "  [{lang}] {}", lang_note(lang));
    }
    if schema.is_none() {
        let _ = writeln!(out, "  (powder.schema.json이 없어 코드젠은 건너뜀 — `powder init` 후 다시 실행)");
    }
    Ok(())
}

pub fn run(args: &[String], cwd: &Path) -> Result<String, String> {
    let mut out = String::new();
    let flag = |name: &str| -> Option<String> {
        args.iter()
            .position(|a| a == name)
            .and_then(|i| args.get(i + 1))
            .cloned()
    };
    let mut doc = load_config(cwd)?;

    // --show: 현재 설정 출력
    if args.iter().any(|a| a == "--show") {
        let _ = writeln!(out, "{}", serde_json::to_string_pretty(&doc).unwrap());
        return Ok(out);
    }

    // --add <lang>: 언어만 추가 (나중에 설치)
    if let Some(add) = flag("--add") {
        let new_langs = parse_langs(&add)?;
        let mut langs: Vec<String> = doc
            .get("languages")
            .and_then(J::as_array)
            .map(|a| a.iter().filter_map(J::as_str).map(String::from).collect())
            .unwrap_or_default();
        for l in &new_langs {
            if !langs.contains(l) {
                langs.push(l.clone());
            }
        }
        doc["languages"] = json!(langs);
        save_config(cwd, &doc)?;
        let _ = writeln!(out, "added: {}", new_langs.join(", "));
        install_langs(&mut out, &new_langs, cwd)?;
        let _ = writeln!(out, "wrote {CONFIG_FILE}");
        return Ok(out);
    }

    // 전체 setup: 플래그가 없으면 대화형으로 묻는다.
    let interactive = args.iter().all(|a| !a.starts_with("--"));
    let db = flag("--db").unwrap_or_else(|| {
        if interactive {
            let v = ask("데이터베이스 URL [sqlite://app.db]: ");
            if v.is_empty() { "sqlite://app.db".into() } else { v }
        } else {
            doc.pointer("/database/url")
                .and_then(J::as_str)
                .unwrap_or("sqlite://app.db")
                .to_string()
        }
    });
    let langs_raw = flag("--langs").unwrap_or_else(|| {
        if interactive {
            let v = ask(&format!("사용할 언어 (콤마 구분, 가능: {}) [ts]: ", LANGUAGES.join(", ")));
            if v.is_empty() { "ts".into() } else { v }
        } else {
            "ts".into()
        }
    });
    let langs = parse_langs(&langs_raw)?;

    let ai_endpoint = flag("--ai-endpoint").unwrap_or_else(|| {
        if interactive {
            ask("AI 쿼리 생성 엔드포인트 (OpenAI 호환, 비우면 나중에 설정): ")
        } else {
            doc.pointer("/ai/endpoint").and_then(J::as_str).unwrap_or("").to_string()
        }
    });

    doc["database"] = json!({ "url": db });
    doc["languages"] = json!(langs);
    if !ai_endpoint.is_empty() {
        let model = flag("--ai-model").unwrap_or_else(|| {
            if interactive {
                let v = ask("AI 모델 이름 [qwen3.5-35b-a3b]: ");
                if v.is_empty() { "qwen3.5-35b-a3b".into() } else { v }
            } else {
                "qwen3.5-35b-a3b".into()
            }
        });
        let max = flag("--ai-max")
            .and_then(|v| v.parse::<usize>().ok())
            .unwrap_or(50);
        doc["ai"] = json!({
            "endpoint": ai_endpoint,
            "model": model,
            "apiKey": flag("--ai-key").unwrap_or_default(),
            "maxConcurrent": max,
        });
    }
    save_config(cwd, &doc)?;

    let _ = writeln!(out, "database : {db}");
    let _ = writeln!(out, "languages: {}", langs.join(", "));
    install_langs(&mut out, &langs, cwd)?;
    if doc.get("ai").is_some() {
        let _ = writeln!(out, "ai       : {} ({})",
            doc.pointer("/ai/endpoint").and_then(J::as_str).unwrap_or(""),
            doc.pointer("/ai/model").and_then(J::as_str).unwrap_or(""));
    } else {
        let _ = writeln!(out, "ai       : (미설정 — `powder setup --ai-endpoint <url>` 로 나중에 추가)");
    }
    let _ = writeln!(out, "wrote {CONFIG_FILE}");
    let _ = writeln!(out, "\n언어는 언제든 추가: powder setup --add <lang>");
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmpdir(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("powder-setup-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn language_aliases_normalize_and_reject_unknown() {
        assert_eq!(parse_langs("TypeScript, py, C++, c#").unwrap(), ["ts", "python", "cpp", "csharp"]);
        assert!(parse_langs("cobol").is_err());
    }

    #[test]
    fn setup_writes_config_and_codegen_then_add_appends() {
        let cwd = tmpdir("cfg");
        crate::cli::run(&args(&["init"]), &cwd).unwrap();

        let out = run(
            &args(&["--db", "sqlite://app.db", "--langs", "ts,python",
                    "--ai-endpoint", "http://dgx-spark:8000/v1", "--ai-model", "qwen3.5-35b-a3b"]),
            &cwd,
        )
        .unwrap();
        assert!(out.contains("wrote powder.config.json"), "{out}");
        assert!(cwd.join("powder_models.ts").exists());
        assert!(cwd.join("powder_models.py").exists());

        let doc = load_config(&cwd).unwrap();
        assert_eq!(doc["languages"], json!(["ts", "python"]));
        assert_eq!(doc["ai"]["maxConcurrent"], json!(50));

        // 나중에 언어 추가
        let out = run(&args(&["--add", "kotlin,go"]), &cwd).unwrap();
        assert!(out.contains("added: kotlin, go"), "{out}");
        let doc = load_config(&cwd).unwrap();
        assert_eq!(doc["languages"], json!(["ts", "python", "kotlin", "go"]));

        // --show round-trips
        let show = run(&args(&["--show"]), &cwd).unwrap();
        assert!(show.contains("dgx-spark"));
        std::fs::remove_dir_all(&cwd).unwrap();
    }
}
