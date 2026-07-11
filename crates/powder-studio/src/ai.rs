//! AI query generation — "하고 싶은 작업"을 적으면 스키마를 컨텍스트로 넣어
//! OpenAI 호환 엔드포인트(vLLM/sglang 등, 예: DGX Spark의 Qwen)에 SQL 생성을
//! 요청한다. 생성 결과는 항상 사람이 확인한 뒤 실행하는 흐름이 기본이고,
//! 실행 경로는 엔진을 그대로 타므로 SQL 인젝션 가드가 최종 방어선이 된다.
//!
//! 설정은 `powder.config.json`의 `ai` 섹션 (엔드포인트는 나중에 붙여도 됨):
//!
//! ```json
//! {
//!   "ai": {
//!     "endpoint": "http://dgx-spark:8000/v1",
//!     "model": "qwen3.5-35b-a3b",
//!     "apiKey": "",
//!     "maxConcurrent": 50
//!   }
//! }
//! ```

use serde_json::{json, Value as J};

#[derive(Debug, Clone, PartialEq)]
pub struct AiConfig {
    pub endpoint: String,
    pub model: String,
    pub api_key: String,
    pub max_concurrent: usize,
}

/// Parse the `ai` section of a `powder.config.json` document (the CLI owns
/// reading/writing the file itself).
pub fn ai_config(doc: &J) -> Option<AiConfig> {
    let ai = doc.get("ai")?;
    let endpoint = ai.get("endpoint")?.as_str()?.trim_end_matches('/').to_string();
    if endpoint.is_empty() {
        return None;
    }
    Some(AiConfig {
        endpoint,
        model: ai.get("model").and_then(J::as_str).unwrap_or("default").into(),
        api_key: ai.get("apiKey").and_then(J::as_str).unwrap_or("").into(),
        max_concurrent: ai
            .get("maxConcurrent")
            .and_then(J::as_u64)
            .map(|n| n as usize)
            .unwrap_or(50)
            .max(1),
    })
}

/// System+user prompt for the model: schema first, task second, and a strict
/// "one SQL statement, no prose" contract so the answer drops into the
/// engine (whose stacked-statement guard enforces the same contract).
pub fn build_prompt(schema_summary: &str, flavor: &str, task: &str) -> (String, String) {
    let system = format!(
        "You are a SQL generator for the Powder engine.\n\
         Target dialect: {flavor}.\n\
         Rules:\n\
         - Reply with EXACTLY ONE SQL statement and nothing else — no prose, \
           no markdown fences, no trailing semicolon-separated extras.\n\
         - Use only tables/columns from the schema below.\n\
         - Prefer explicit column lists over SELECT *.\n\
         - Never invent DROP/TRUNCATE unless the task explicitly asks.\n\n\
         Schema:\n{schema_summary}"
    );
    (system, format!("Task: {task}\nSQL:"))
}

/// Strip the model's cosmetic wrapping (markdown fences, `SQL:` echoes,
/// trailing `;`) down to the bare statement.
pub fn extract_sql(reply: &str) -> String {
    let mut s = reply.trim();
    if let Some(start) = s.find("```") {
        let after = &s[start + 3..];
        let after = after.strip_prefix("sql").or_else(|| after.strip_prefix("SQL")).unwrap_or(after);
        s = match after.find("```") {
            Some(end) => &after[..end],
            None => after,
        };
    }
    let s = s.trim().trim_start_matches("SQL:").trim();
    s.trim_end_matches(';').trim().to_string()
}

/// Call the OpenAI-compatible `/chat/completions` endpoint (blocking).
pub fn generate(cfg: &AiConfig, system: &str, user: &str) -> Result<String, String> {
    let url = format!("{}/chat/completions", cfg.endpoint);
    let body = json!({
        "model": cfg.model,
        "messages": [
            { "role": "system", "content": system },
            { "role": "user", "content": user }
        ],
        "temperature": 0.1,
        "max_tokens": 512,
        // Thinking models (Qwen3/3.5) emit reasoning prose before the
        // answer; vLLM's chat template turns it off with this kwarg.
        // Servers without the kwarg simply ignore the extra field.
        "chat_template_kwargs": { "enable_thinking": false },
    });
    let mut req = ureq::post(&url).timeout(std::time::Duration::from_secs(120));
    if !cfg.api_key.is_empty() {
        req = req.set("Authorization", &format!("Bearer {}", cfg.api_key));
    }
    let resp = req
        .send_json(body)
        .map_err(|e| format!("AI endpoint `{url}` unreachable: {e}"))?;
    let doc: J = resp.into_json().map_err(|e| e.to_string())?;
    let content = doc
        .pointer("/choices/0/message/content")
        .and_then(J::as_str)
        .ok_or_else(|| format!("unexpected AI response shape: {doc}"))?;
    Ok(extract_sql(content))
}

// ---------------------------------------------------------------------------
// FIFO admission queue — 최대 N명 동시, 초과분은 도착 순서대로 대기.
// ---------------------------------------------------------------------------

use std::collections::VecDeque;
use std::sync::{Condvar, Mutex};

pub struct FifoGate {
    max: usize,
    state: Mutex<GateState>,
    cv: Condvar,
}

struct GateState {
    running: usize,
    next_ticket: u64,
    waiting: VecDeque<u64>,
}

pub struct GatePass<'a>(&'a FifoGate);

impl FifoGate {
    pub fn new(max: usize) -> Self {
        Self {
            max: max.max(1),
            state: Mutex::new(GateState {
                running: 0,
                next_ticket: 0,
                waiting: VecDeque::new(),
            }),
            cv: Condvar::new(),
        }
    }

    /// Current queue depth (people waiting behind the running set).
    pub fn waiting(&self) -> usize {
        self.state.lock().unwrap().waiting.len()
    }

    /// Block until admitted; strictly first-come-first-served.
    pub fn enter(&self) -> GatePass<'_> {
        let mut st = self.state.lock().unwrap();
        let ticket = st.next_ticket;
        st.next_ticket += 1;
        st.waiting.push_back(ticket);
        while !(st.running < self.max && st.waiting.front() == Some(&ticket)) {
            st = self.cv.wait(st).unwrap();
        }
        st.waiting.pop_front();
        st.running += 1;
        GatePass(self)
    }
}

impl Drop for GatePass<'_> {
    fn drop(&mut self) {
        let mut st = self.0.state.lock().unwrap();
        st.running -= 1;
        drop(st);
        self.0.cv.notify_all();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_roundtrip_and_defaults() {
        let doc = json!({
            "ai": { "endpoint": "http://dgx-spark:8000/v1/", "model": "qwen3.5-35b-a3b" }
        });
        let cfg = ai_config(&doc).unwrap();
        assert_eq!(cfg.endpoint, "http://dgx-spark:8000/v1"); // trailing / trimmed
        assert_eq!(cfg.max_concurrent, 50);
        assert!(ai_config(&json!({})).is_none());
        assert!(ai_config(&json!({"ai": {"endpoint": ""}})).is_none());
    }

    #[test]
    fn sql_extraction_unwraps_fences_and_prose() {
        assert_eq!(extract_sql("SELECT 1"), "SELECT 1");
        assert_eq!(extract_sql("```sql\nSELECT 1;\n```"), "SELECT 1");
        assert_eq!(extract_sql("SQL: SELECT name FROM users;"), "SELECT name FROM users");
    }

    #[test]
    fn gate_admits_in_fifo_order_with_cap() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::sync::Arc;
        let gate = Arc::new(FifoGate::new(2));
        let peak = Arc::new(AtomicUsize::new(0));
        let cur = Arc::new(AtomicUsize::new(0));
        let order = Arc::new(Mutex::new(Vec::new()));
        let mut handles = Vec::new();
        for i in 0..6 {
            let (g, p, c, o) = (gate.clone(), peak.clone(), cur.clone(), order.clone());
            handles.push(std::thread::spawn(move || {
                // Stagger arrivals so ticket order is deterministic.
                std::thread::sleep(std::time::Duration::from_millis(i as u64 * 30));
                let _pass = g.enter();
                o.lock().unwrap().push(i);
                let now = c.fetch_add(1, Ordering::SeqCst) + 1;
                p.fetch_max(now, Ordering::SeqCst);
                std::thread::sleep(std::time::Duration::from_millis(120));
                c.fetch_sub(1, Ordering::SeqCst);
            }));
        }
        for h in handles {
            h.join().unwrap();
        }
        assert!(peak.load(Ordering::SeqCst) <= 2, "cap of 2 exceeded");
        let got = order.lock().unwrap().clone();
        assert_eq!(got, vec![0, 1, 2, 3, 4, 5], "admission must be FIFO");
    }

    #[test]
    fn prompt_carries_schema_and_dialect() {
        let (sys, user) = build_prompt("- users(id INTEGER PK)", "postgres", "활성 사용자 수");
        assert!(sys.contains("postgres") && sys.contains("users(id INTEGER PK)"));
        assert!(user.contains("활성 사용자 수"));
    }
}
