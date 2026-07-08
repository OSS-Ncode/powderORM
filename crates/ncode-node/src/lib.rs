//! Node.js bindings for Ncode via napi-rs.
//!
//! The native layer is deliberately thin: it owns the async connection and
//! returns query results as the raw NCB byte buffer (handed to V8 as an
//! external `Buffer`, so no copy on the way out). The fluent query builder and
//! the zero-copy columnar reader live in the TypeScript wrapper (`ts/`), which
//! is the idiomatic place for them in the Node ecosystem.
//!
//! Every Rust `async fn` below is surfaced to JavaScript as a `Promise` — the
//! napi-rs `tokio_rt` runtime drives the core's `Future`s.

use napi::bindgen_prelude::*;
use napi_derive::napi;

use ncode_core::{Client as CoreClient, Value};

/// Convert a core error into a JS-friendly napi error.
fn map_err(e: ncode_core::Error) -> Error {
    Error::from_reason(e.to_string())
}

/// A single bound parameter arriving from JS: number | string | boolean | null.
type ParamValue = Either4<f64, String, bool, Null>;

fn to_value(p: ParamValue) -> Value {
    match p {
        // JS has only `number`; treat exact integers in the safe range as ints.
        Either4::A(n) => {
            if n.fract() == 0.0 && n.abs() < 9_007_199_254_740_992.0 {
                Value::Int(n as i64)
            } else {
                Value::Float(n)
            }
        }
        Either4::B(s) => Value::Text(s),
        Either4::C(b) => Value::Bool(b),
        Either4::D(_) => Value::Null,
    }
}

fn to_values(params: Option<Vec<ParamValue>>) -> Vec<Value> {
    params.unwrap_or_default().into_iter().map(to_value).collect()
}

/// An async database client. Returned by [`connect`].
#[napi]
pub struct Client {
    inner: CoreClient,
}

/// Open a connection. Returns a `Promise<Client>`.
#[napi]
pub async fn connect(url: String) -> Result<Client> {
    let inner = CoreClient::connect(&url).await.map_err(map_err)?;
    Ok(Client { inner })
}

#[napi]
impl Client {
    /// Run a non-row statement (INSERT/UPDATE/DDL). Resolves to rows affected.
    #[napi]
    pub async fn execute(&self, sql: String, params: Option<Vec<ParamValue>>) -> Result<u32> {
        let n = self
            .inner
            .execute(&sql, to_values(params))
            .await
            .map_err(map_err)?;
        Ok(n as u32)
    }

    /// Run a query. Resolves to a `Buffer` holding the NCB columnar payload,
    /// which `decodeBatch()` in the TS wrapper turns into zero-copy typed
    /// arrays. The `Vec<u8>` is transferred to V8 as an external buffer.
    #[napi]
    pub async fn query(&self, sql: String, params: Option<Vec<ParamValue>>) -> Result<Buffer> {
        let bytes = self
            .inner
            .query_bytes(&sql, to_values(params))
            .await
            .map_err(map_err)?;
        Ok(Buffer::from(bytes))
    }
}
