//! `powder studio` — a Drizzle/Prisma-Studio-style database dashboard served
//! from the CLI binary. One embedded HTML page (no build step, no CDN) talks
//! to a small JSON API; every query goes through the engine `Client`, so all
//! six backends and the SQL-injection guard apply unchanged.
//!
//! ## Access model
//!
//! The server mints an unguessable admin token at startup and prints the
//! URL. Every request must carry a valid token (`?t=...`); the page keeps it
//! in JS and appends it to each API call. The admin can mint invite tokens —
//! read-only (browse tables) or read-write (edit rows, run SQL) — and share
//! the printed URL. Bind `--host 0.0.0.0` (plus a port forward or tunnel) to
//! work with people over the internet; `--readonly` locks the whole server.

use std::collections::HashMap;
use std::fmt::Write as _;
use std::path::Path;
use std::sync::Mutex;

use serde_json::{json, Map, Value as J};

use powder_core::inspect::{self as tools, Engine};

use crate::ai::{self, AiConfig, FifoGate};

const STUDIO_HTML: &str = include_str!("studio.html");
const PAGE_SIZE_MAX: usize = 500;

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Mode {
    Admin,
    ReadWrite,
    ReadOnly,
}

pub struct Studio {
    engine: Engine,
    tokens: Mutex<Vec<(String, Mode)>>,
    readonly: bool,
    db_label: String,
    /// AI query generation (optional — configured via `powder setup`).
    ai: Option<AiConfig>,
    /// Admission control for the shared model server: at most N concurrent
    /// generations, FIFO beyond that.
    gate: FifoGate,
}

/// Unguessable URL token: 128 bits of OS randomness, hex-encoded.
fn new_token() -> String {
    let mut buf = [0u8; 16];
    getrandom::fill(&mut buf).expect("OS randomness unavailable");
    buf.iter().fold(String::with_capacity(32), |mut s, b| {
        let _ = write!(s, "{b:02x}");
        s
    })
}

fn ident_ok(s: &str) -> bool {
    !s.is_empty()
        && s.chars().next().is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// Strip credentials out of the connection URL for display.
fn redact(url: &str) -> String {
    match (url.find("://"), url.rfind('@')) {
        (Some(s), Some(a)) if a > s => format!("{}://***@{}", &url[..s], &url[a + 1..]),
        _ => url.to_string(),
    }
}

impl Studio {
    pub fn new(
        url: &str,
        cwd: &Path,
        readonly: bool,
        admin_token: Option<String>,
        ai: Option<AiConfig>,
    ) -> Result<Self, String> {
        let engine = Engine::connect(url, cwd)?;
        let token = admin_token.unwrap_or_else(new_token);
        let max = ai.as_ref().map(|c| c.max_concurrent).unwrap_or(50);
        Ok(Self {
            engine,
            tokens: Mutex::new(vec![(token, Mode::Admin)]),
            readonly,
            db_label: redact(url),
            ai,
            gate: FifoGate::new(max),
        })
    }

    pub fn admin_token(&self) -> String {
        self.tokens.lock().unwrap()[0].0.clone()
    }

    fn mode_of(&self, token: &str) -> Option<Mode> {
        self.tokens
            .lock()
            .unwrap()
            .iter()
            .find(|(t, _)| t == token)
            .map(|(_, m)| *m)
    }

    fn can_write(&self, mode: Mode) -> bool {
        !self.readonly && mode != Mode::ReadOnly
    }

    /// Handle one API request; returns (HTTP status, JSON body).
    pub fn api(&self, method: &str, path: &str, query: &HashMap<String, String>, body: &str) -> (u16, String) {
        let Some(mode) = query.get("t").and_then(|t| self.mode_of(t)) else {
            return (401, json!({"error": "invalid or missing token"}).to_string());
        };
        let r = match (method, path) {
            ("GET", "/api/meta") => self.meta(mode),
            ("GET", "/api/rows") => self.rows(query),
            ("POST", "/api/query") => self.run_sql(mode, body, true),
            ("POST", "/api/exec") => self.run_sql(mode, body, false),
            ("POST", "/api/mutate") => self.mutate(mode, body),
            ("POST", "/api/ai") => self.ai_generate(mode, body),
            ("POST", "/api/invite") => self.invite(mode, body),
            _ => Err((404, "not found".into())),
        };
        match r {
            Ok(v) => (200, v.to_string()),
            Err((code, msg)) => (code, json!({ "error": msg }).to_string()),
        }
    }

    fn meta(&self, mode: Mode) -> Result<J, (u16, String)> {
        let names = tools::table_names(&self.engine).map_err(internal)?;
        let mut tables = Vec::new();
        for t in &names {
            let cols = tools::table_columns(&self.engine, t).map_err(internal)?;
            let fks = tools::table_fks(&self.engine, t).map_err(internal)?;
            tables.push(json!({
                "name": t,
                "columns": cols.iter().map(|c| json!({
                    "name": c.name, "type": c.sql_type,
                    "notnull": c.notnull, "pk": c.pk,
                })).collect::<Vec<_>>(),
                "fks": fks.iter().map(|f| json!({
                    "from": f.from, "table": f.table, "to": f.to,
                })).collect::<Vec<_>>(),
            }));
        }
        Ok(json!({
            "db": self.db_label,
            "flavor": self.engine.flavor(),
            "mode": match mode { Mode::Admin => "admin", Mode::ReadWrite => "rw", Mode::ReadOnly => "ro" },
            "writable": self.can_write(mode),
            "tables": tables,
            "ai": {
                "configured": self.ai.is_some(),
                "model": self.ai.as_ref().map(|c| c.model.clone()).unwrap_or_default(),
                "waiting": self.gate.waiting(),
                "max": self.ai.as_ref().map(|c| c.max_concurrent).unwrap_or(0),
            },
        }))
    }

    /// 자연어 작업 설명 → SQL. 스키마 요약을 먼저 뽑고(짧은 락), 모델 호출은
    /// FIFO 게이트 안에서 락 없이 수행한다 — 동시 N명 초과분은 도착 순 대기.
    fn ai_generate(&self, mode: Mode, body: &str) -> Result<J, (u16, String)> {
        if !self.can_write(mode) {
            return Err((403, "read-only token: AI 생성은 비활성화되어 있습니다".into()));
        }
        let Some(cfg) = &self.ai else {
            return Err((400, "AI 엔드포인트 미설정 — `powder setup --ai-endpoint <url>`".into()));
        };
        let doc: J = serde_json::from_str(body).map_err(|e| (400, e.to_string()))?;
        let task = doc.get("task").and_then(J::as_str).unwrap_or_default();
        if task.trim().is_empty() {
            return Err((400, "missing `task`".into()));
        }
        let summary = tools::schema_summary(&self.engine).map_err(internal)?;
        let (system, user) = ai::build_prompt(&summary, self.engine.flavor(), task);
        let queued_behind = self.gate.waiting();
        let _pass = self.gate.enter();
        let sql = ai::generate(cfg, &system, &user).map_err(|e| (502, e))?;
        Ok(json!({ "sql": sql, "queuedBehind": queued_behind }))
    }

    fn rows(&self, q: &HashMap<String, String>) -> Result<J, (u16, String)> {
        let table = q.get("table").cloned().unwrap_or_default();
        let columns = self.check_table(&table)?;
        let limit: usize = q
            .get("limit")
            .and_then(|v| v.parse().ok())
            .unwrap_or(50)
            .clamp(1, PAGE_SIZE_MAX);
        let offset: usize = q.get("offset").and_then(|v| v.parse().ok()).unwrap_or(0);
        let (order, dir) = self.order_clause(&table, q)?;

        let sql = if self.engine.flavor() == "mssql" {
            // T-SQL has no LIMIT/OFFSET. TOP covers page one; deeper pages
            // use ROW_NUMBER() (works on 2005+, unlike OFFSET/FETCH's 2012+).
            if offset == 0 {
                format!("SELECT TOP ({limit}) * FROM {table}{order}{dir}")
            } else {
                let ob = if order.is_empty() {
                    format!("ORDER BY {}", self.first_column(&table)?)
                } else {
                    format!("{}{dir}", order.trim_start())
                };
                let cols = columns.join(", ");
                format!(
                    "SELECT {cols} FROM (SELECT {cols}, ROW_NUMBER() OVER ({ob}) AS __rn FROM {table}) q \
                     WHERE __rn > {offset} AND __rn <= {end}",
                    end = offset + limit
                )
            }
        } else {
            format!("SELECT * FROM {table}{order}{dir} LIMIT {limit} OFFSET {offset}")
        };
        let batch = self.engine.query(&sql).map_err(bad)?;
        let total_batch = self
            .engine
            .query(&format!("SELECT COUNT(*) AS n FROM {table}"))
            .map_err(bad)?;
        let total = total_batch
            .column("n")
            .and_then(|c| c.i64(0))
            .unwrap_or(0);
        Ok(json!({
            "columns": batch.columns.iter().map(|c| c.field.name.clone()).collect::<Vec<_>>(),
            "rows": tools::batch_rows(&batch),
            "total": total,
        }))
    }

    fn run_sql(&self, mode: Mode, body: &str, is_query: bool) -> Result<J, (u16, String)> {
        if !self.can_write(mode) {
            return Err((403, "read-only token: SQL runner is disabled".into()));
        }
        let doc: J = serde_json::from_str(body).map_err(|e| (400, e.to_string()))?;
        let sql = doc.get("sql").and_then(J::as_str).unwrap_or_default();
        if sql.trim().is_empty() {
            return Err((400, "missing `sql`".into()));
        }
        if is_query {
            let batch = self.engine.query(sql).map_err(bad)?;
            Ok(json!({
                "columns": batch.columns.iter().map(|c| c.field.name.clone()).collect::<Vec<_>>(),
                "rows": tools::batch_rows(&batch),
            }))
        } else {
            let n = self.engine.execute(sql).map_err(bad)?;
            Ok(json!({ "affected": n }))
        }
    }

    /// Row edits: `{op: insert|update|delete, table, values?, where?}` —
    /// identifiers validated against the live catalog, values bound as
    /// parameters (never inlined).
    fn mutate(&self, mode: Mode, body: &str) -> Result<J, (u16, String)> {
        if !self.can_write(mode) {
            return Err((403, "read-only token: edits are disabled".into()));
        }
        let doc: J = serde_json::from_str(body).map_err(|e| (400, e.to_string()))?;
        let op = doc.get("op").and_then(J::as_str).unwrap_or_default();
        let table = doc.get("table").and_then(J::as_str).unwrap_or_default().to_string();
        let cols = self.check_table(&table)?;
        let col_ok = |name: &str| cols.iter().any(|c| c == name);

        let obj = |key: &str| -> Result<Map<String, J>, (u16, String)> {
            doc.get(key)
                .and_then(J::as_object)
                .cloned()
                .ok_or((400, format!("missing `{key}` object")))
        };
        let mut params: Vec<powder_core::Value> = Vec::new();
        let mut bind = |v: &J| -> Result<(), (u16, String)> {
            params.push(tools::json_to_value(v).map_err(|e| (400, e))?);
            Ok(())
        };

        let sql = match op {
            "insert" => {
                let values = obj("values")?;
                let mut names = Vec::new();
                for (k, v) in &values {
                    if !col_ok(k) {
                        return Err((400, format!("unknown column `{k}`")));
                    }
                    names.push(k.clone());
                    bind(v)?;
                }
                if names.is_empty() {
                    return Err((400, "insert needs at least one column".into()));
                }
                format!(
                    "INSERT INTO {table} ({}) VALUES ({})",
                    names.join(", "),
                    vec!["?"; names.len()].join(", ")
                )
            }
            "update" | "delete" => {
                let mut set_frag = String::new();
                if op == "update" {
                    let values = obj("values")?;
                    let mut frags = Vec::new();
                    for (k, v) in &values {
                        if !col_ok(k) {
                            return Err((400, format!("unknown column `{k}`")));
                        }
                        frags.push(format!("{k} = ?"));
                        bind(v)?;
                    }
                    if frags.is_empty() {
                        return Err((400, "update needs at least one column".into()));
                    }
                    set_frag = format!(" SET {}", frags.join(", "));
                }
                let where_ = obj("where")?;
                if where_.is_empty() {
                    return Err((400, "refusing an empty `where` (would hit every row)".into()));
                }
                let mut conds = Vec::new();
                for (k, v) in &where_ {
                    if !col_ok(k) {
                        return Err((400, format!("unknown column `{k}`")));
                    }
                    if v.is_null() {
                        conds.push(format!("{k} IS NULL"));
                    } else {
                        conds.push(format!("{k} = ?"));
                        bind(v)?;
                    }
                }
                let verb = if op == "update" { format!("UPDATE {table}") } else { format!("DELETE FROM {table}") };
                format!("{verb}{set_frag} WHERE {}", conds.join(" AND "))
            }
            other => return Err((400, format!("unknown op `{other}`"))),
        };
        let n = self.engine.execute_params(&sql, params).map_err(bad)?;
        Ok(json!({ "affected": n }))
    }

    fn invite(&self, mode: Mode, body: &str) -> Result<J, (u16, String)> {
        if mode != Mode::Admin {
            return Err((403, "only the admin token can mint invites".into()));
        }
        let doc: J = serde_json::from_str(body).unwrap_or(J::Null);
        let ro = doc.get("mode").and_then(J::as_str) != Some("rw");
        let token = new_token();
        self.tokens
            .lock()
            .unwrap()
            .push((token.clone(), if ro { Mode::ReadOnly } else { Mode::ReadWrite }));
        Ok(json!({ "token": token, "mode": if ro { "ro" } else { "rw" } }))
    }

    /// Validate the table against the live catalog; return its column names.
    fn check_table(&self, table: &str) -> Result<Vec<String>, (u16, String)> {
        if !ident_ok(table) {
            return Err((400, format!("invalid table name `{table}`")));
        }
        let names = tools::table_names(&self.engine).map_err(internal)?;
        if !names.iter().any(|n| n == table) {
            return Err((404, format!("table `{table}` not found")));
        }
        let cols = tools::table_columns(&self.engine, table).map_err(internal)?;
        Ok(cols.into_iter().map(|c| c.name).collect())
    }

    fn order_clause(
        &self,
        table: &str,
        q: &HashMap<String, String>,
    ) -> Result<(String, String), (u16, String)> {
        let Some(col) = q.get("order").filter(|c| !c.is_empty()) else {
            return Ok((String::new(), String::new()));
        };
        let cols = self.check_table(table)?;
        if !cols.iter().any(|c| c == col) {
            return Err((400, format!("unknown order column `{col}`")));
        }
        let dir = if q.get("dir").map(String::as_str) == Some("desc") { " DESC" } else { " ASC" };
        Ok((format!(" ORDER BY {col}"), dir.into()))
    }

    fn first_column(&self, table: &str) -> Result<String, (u16, String)> {
        let cols = tools::table_columns(&self.engine, table).map_err(internal)?;
        cols.iter()
            .find(|c| c.pk == 1)
            .or_else(|| cols.first())
            .map(|c| c.name.clone())
            .ok_or((404, format!("table `{table}` has no columns")))
    }
}

fn internal(e: String) -> (u16, String) {
    (500, e)
}
fn bad(e: String) -> (u16, String) {
    (400, e)
}

fn parse_query(raw: &str) -> HashMap<String, String> {
    let mut map = HashMap::new();
    for pair in raw.split('&').filter(|s| !s.is_empty()) {
        let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
        map.insert(k.to_string(), percent_decode(v));
    }
    map
}

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' if i + 2 < bytes.len() + 1 && i + 2 < bytes.len() + 1 => {
                if let (Some(h), Some(l)) = (
                    bytes.get(i + 1).and_then(|b| (*b as char).to_digit(16)),
                    bytes.get(i + 2).and_then(|b| (*b as char).to_digit(16)),
                ) {
                    out.push((h * 16 + l) as u8);
                    i += 3;
                } else {
                    out.push(b'%');
                    i += 1;
                }
            }
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn handle(studio: &Studio, req: &mut tiny_http::Request) -> (u16, String, &'static str) {
    let raw_url = req.url().to_string();
    let (path, qraw) = raw_url.split_once('?').unwrap_or((raw_url.as_str(), ""));
    let query = parse_query(qraw);
    let method = req.method().as_str().to_uppercase();

    if path == "/" || path == "/index.html" {
        return match query.get("t").and_then(|t| studio.mode_of(t)) {
            Some(_) => (200, STUDIO_HTML.to_string(), "text/html; charset=utf-8"),
            None => (
                401,
                "<h3>powder studio</h3><p>유효한 토큰이 필요합니다 — 서버를 시작한 사람에게 초대 URL을 요청하세요.</p>".into(),
                "text/html; charset=utf-8",
            ),
        };
    }
    if path.starts_with("/api/") {
        let mut body_in = String::new();
        let _ = req.as_reader().read_to_string(&mut body_in);
        let (status, body) = studio.api(&method, path, &query, &body_in);
        return (status, body, "application/json; charset=utf-8");
    }
    (404, json!({"error": "not found"}).to_string(), "application/json; charset=utf-8")
}

/// Run the studio server (blocks until the process is stopped). Requests are
/// handled on a worker pool so long AI generations don't block browsing.
pub fn serve(
    url: &str,
    cwd: &Path,
    host: &str,
    port: u16,
    readonly: bool,
    ai: Option<AiConfig>,
) -> Result<(), String> {
    let studio = std::sync::Arc::new(Studio::new(url, cwd, readonly, None, ai)?);
    let addr = format!("{host}:{port}");
    let server =
        std::sync::Arc::new(tiny_http::Server::http(&addr).map_err(|e| format!("cannot bind {addr}: {e}"))?);
    let token = studio.admin_token();
    let shown_host = if host == "0.0.0.0" { "localhost" } else { host };
    println!("powder studio — {}", studio.db_label);
    println!("  URL   : http://{shown_host}:{port}/?t={token}");
    println!("  mode  : {}", if readonly { "read-only" } else { "admin (초대 발급 가능)" });
    match &studio.ai {
        Some(cfg) => println!("  AI    : {} ({}) — 동시 {}명, 초과분 선착순 대기", cfg.endpoint, cfg.model, cfg.max_concurrent),
        None => println!("  AI    : 미설정 (powder setup --ai-endpoint <url>)"),
    }
    if host != "0.0.0.0" {
        println!("  공유  : --host 0.0.0.0 으로 실행하면 휴대폰 등 다른 기기에서 접속할 수 있습니다");
    }
    println!("  종료  : Ctrl+C");

    let workers = 64;
    let mut handles = Vec::new();
    for _ in 0..workers {
        let server = server.clone();
        let studio = studio.clone();
        handles.push(std::thread::spawn(move || {
            for mut req in server.incoming_requests() {
                let (status, body, ctype) = handle(&studio, &mut req);
                let header =
                    tiny_http::Header::from_bytes(&b"Content-Type"[..], ctype.as_bytes()).unwrap();
                let resp = tiny_http::Response::from_string(body)
                    .with_status_code(status)
                    .with_header(header);
                let _ = req.respond(resp);
            }
        }));
    }
    for h in handles {
        let _ = h.join();
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn studio() -> Studio {
        let cwd = std::env::temp_dir();
        let s = Studio::new("sqlite::memory:", &cwd, false, Some("tok_admin".into()), None).unwrap();
        s.engine
            .execute("CREATE TABLE users (id INTEGER PRIMARY KEY, name TEXT, score REAL)")
            .unwrap();
        s.engine
            .execute_params(
                "INSERT INTO users VALUES (?, ?, ?)",
                vec![
                    powder_core::Value::Int(1),
                    powder_core::Value::Text("alice".into()),
                    powder_core::Value::Float(9.5),
                ],
            )
            .unwrap();
        s
    }

    fn q(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
    }

    #[test]
    fn auth_gates_everything() {
        let s = studio();
        let (code, _) = s.api("GET", "/api/meta", &q(&[]), "");
        assert_eq!(code, 401);
        let (code, _) = s.api("GET", "/api/meta", &q(&[("t", "wrong")]), "");
        assert_eq!(code, 401);
        let (code, body) = s.api("GET", "/api/meta", &q(&[("t", "tok_admin")]), "");
        assert_eq!(code, 200, "{body}");
        assert!(body.contains("\"users\""));
    }

    #[test]
    fn rows_and_mutate_roundtrip() {
        let s = studio();
        let t = q(&[("t", "tok_admin"), ("table", "users")]);
        let (code, body) = s.api("GET", "/api/rows", &t, "");
        assert_eq!(code, 200, "{body}");
        assert!(body.contains("alice") && body.contains("\"total\":1"));

        let (code, body) = s.api(
            "POST",
            "/api/mutate",
            &q(&[("t", "tok_admin")]),
            r#"{"op":"update","table":"users","values":{"score":10.0},"where":{"id":1}}"#,
        );
        assert_eq!(code, 200, "{body}");
        let (_, body) = s.api("GET", "/api/rows", &t, "");
        assert!(body.contains("10.0") || body.contains("\"score\":10"), "{body}");

        // Empty where must be refused.
        let (code, _) = s.api(
            "POST",
            "/api/mutate",
            &q(&[("t", "tok_admin")]),
            r#"{"op":"delete","table":"users","where":{}}"#,
        );
        assert_eq!(code, 400);
        // Unknown identifiers are rejected before touching SQL.
        let (code, _) = s.api(
            "POST",
            "/api/mutate",
            &q(&[("t", "tok_admin")]),
            r#"{"op":"update","table":"users","values":{"nope":1},"where":{"id":1}}"#,
        );
        assert_eq!(code, 400);
    }

    #[test]
    fn invites_and_readonly_scope() {
        let s = studio();
        let (code, body) = s.api("POST", "/api/invite", &q(&[("t", "tok_admin")]), r#"{"mode":"ro"}"#);
        assert_eq!(code, 200, "{body}");
        let doc: J = serde_json::from_str(&body).unwrap();
        let ro = doc["token"].as_str().unwrap().to_string();

        // RO token browses but cannot edit, run SQL, or mint invites.
        let (code, _) = s.api("GET", "/api/rows", &q(&[("t", &ro), ("table", "users")]), "");
        assert_eq!(code, 200);
        let (code, _) = s.api("POST", "/api/exec", &q(&[("t", &ro)]), r#"{"sql":"DELETE FROM users"}"#);
        assert_eq!(code, 403);
        let (code, _) = s.api("POST", "/api/invite", &q(&[("t", &ro)]), r#"{"mode":"rw"}"#);
        assert_eq!(code, 403);

        // RW invite can edit but not invite.
        let (_, body) = s.api("POST", "/api/invite", &q(&[("t", "tok_admin")]), r#"{"mode":"rw"}"#);
        let doc: J = serde_json::from_str(&body).unwrap();
        let rw = doc["token"].as_str().unwrap().to_string();
        let (code, _) = s.api(
            "POST",
            "/api/query",
            &q(&[("t", &rw)]),
            r#"{"sql":"SELECT COUNT(*) AS n FROM users"}"#,
        );
        assert_eq!(code, 200);
        let (code, _) = s.api("POST", "/api/invite", &q(&[("t", &rw)]), r#"{"mode":"ro"}"#);
        assert_eq!(code, 403);
    }

    #[test]
    fn injection_guard_reaches_the_sql_runner() {
        let s = studio();
        let (code, body) = s.api(
            "POST",
            "/api/query",
            &q(&[("t", "tok_admin")]),
            r#"{"sql":"SELECT * FROM users; DROP TABLE users"}"#,
        );
        assert_eq!(code, 400, "{body}");
        assert!(body.contains("SQL-injection guard"), "{body}");
    }

    #[test]
    fn table_and_order_validation() {
        let s = studio();
        let (code, _) = s.api("GET", "/api/rows", &q(&[("t", "tok_admin"), ("table", "users; --")]), "");
        assert_eq!(code, 400);
        let (code, _) = s.api("GET", "/api/rows", &q(&[("t", "tok_admin"), ("table", "ghosts")]), "");
        assert_eq!(code, 404);
        let (code, _) = s.api(
            "GET",
            "/api/rows",
            &q(&[("t", "tok_admin"), ("table", "users"), ("order", "evil")]),
            "",
        );
        assert_eq!(code, 400);
        let (code, _) = s.api(
            "GET",
            "/api/rows",
            &q(&[("t", "tok_admin"), ("table", "users"), ("order", "score"), ("dir", "desc")]),
            "",
        );
        assert_eq!(code, 200);
    }

    #[test]
    fn url_redaction_hides_credentials() {
        assert_eq!(redact("postgres://u:pw@h:5432/db"), "postgres://***@h:5432/db");
        assert_eq!(redact("app.db"), "app.db");
    }
}
