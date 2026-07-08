//! # ncode-core
//!
//! The Rust core of **Ncode** — an async database engine that returns query
//! results in a zero-copy, Apache-Arrow-style columnar binary format ("NCB").
//!
//! The crate is backend-agnostic in shape (SQLite is the bundled backend) and
//! is consumed both directly from Rust and through the `ncode-node` (napi-rs)
//! and `ncode-python` (PyO3) language bindings.
//!
//! ## Pieces
//! - [`Client`] — async connection + query execution.
//! - [`query::Query`] — a fluent, injection-safe SQL builder.
//! - [`RecordBatch`] / [`array::Column`] — the in-memory columnar model.
//! - [`codec`] — the NCB wire format ([`RecordBatch::encode`] /
//!   [`RecordBatch::decode`]).
//!
//! ```no_run
//! # async fn demo() -> ncode_core::Result<()> {
//! use ncode_core::{Client, query::Query};
//!
//! let client = Client::connect("sqlite::memory:").await?;
//! client.execute("CREATE TABLE users (id INTEGER, name TEXT)", vec![]).await?;
//!
//! let (sql, params) = Query::table("users").select(["id", "name"]).build();
//! let batch = client.query(&sql, params).await?;
//! println!("{} rows x {} cols", batch.num_rows, batch.num_columns());
//! # Ok(())
//! # }
//! ```

pub mod array;
pub mod batch;
pub mod client;
pub mod codec;
pub mod error;
pub mod query;
pub mod schema;

pub use array::{Column, ColumnData};
pub use batch::RecordBatch;
pub use client::Client;
pub use error::{Error, Result};
pub use query::{Order, Query, Value};
pub use schema::{DataType, Field, Schema};
