//! Python bindings for Powder via PyO3 + pyo3-async-runtimes.
//!
//! Mirrors the Node binding: a thin native layer that owns the async
//! connection and returns query results as the raw PCB `bytes`; the fluent
//! query builder and the zero-copy columnar reader live in the pure-Python
//! wrapper (`python/powder/`).
//!
//! Each Rust `async fn` is surfaced to Python as an `awaitable` — the
//! `future_into_py` bridge drives the core's `Future`s on a Tokio runtime and
//! resolves the result into the running `asyncio` event loop.

use pyo3::exceptions::PyRuntimeError;
use pyo3::prelude::*;
use pyo3::types::PyBytes;
use pyo3_async_runtimes::tokio::future_into_py;

use powder_core::{Client as CoreClient, Value};

// The columnar build path is allocation-heavy; the platform default heap
// (especially on Windows) measurably slows cold queries. Same setup as the
// Node binding.
#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

fn to_pyerr(e: powder_core::Error) -> PyErr {
    PyRuntimeError::new_err(e.to_string())
}

/// A single bound parameter arriving from Python.
///
/// Order matters: `bool` is a subclass of `int` in Python, so it must be tried
/// before `Int` to avoid `True`/`False` being coerced to `1`/`0`.
#[derive(FromPyObject)]
enum Param {
    #[pyo3(transparent)]
    Bool(bool),
    #[pyo3(transparent)]
    Int(i64),
    #[pyo3(transparent)]
    Float(f64),
    #[pyo3(transparent)]
    Text(String),
}

impl From<Param> for Value {
    fn from(p: Param) -> Self {
        match p {
            Param::Bool(b) => Value::Bool(b),
            Param::Int(i) => Value::Int(i),
            Param::Float(f) => Value::Float(f),
            Param::Text(s) => Value::Text(s),
        }
    }
}

/// `None` in the parameter list maps to SQL NULL; anything else to a [`Param`].
fn to_values(params: Option<Vec<Option<Param>>>) -> Vec<Value> {
    params
        .unwrap_or_default()
        .into_iter()
        .map(|p| p.map(Into::into).unwrap_or(Value::Null))
        .collect()
}

/// An async database client. Returned (awaitably) by [`connect`].
#[pyclass]
struct Client {
    inner: CoreClient,
}

/// Open a connection. Returns an awaitable resolving to a `Client`.
#[pyfunction]
fn connect(py: Python<'_>, url: String) -> PyResult<Bound<'_, PyAny>> {
    future_into_py(py, async move {
        let inner = CoreClient::connect(&url).await.map_err(to_pyerr)?;
        Python::attach(|py| Ok(Py::new(py, Client { inner })?.into_any()))
    })
}

#[pymethods]
impl Client {
    /// Run a non-row statement (INSERT/UPDATE/DDL). Resolves to rows affected.
    fn execute<'p>(
        &self,
        py: Python<'p>,
        sql: String,
        params: Option<Vec<Option<Param>>>,
    ) -> PyResult<Bound<'p, PyAny>> {
        let inner = self.inner.clone();
        let values = to_values(params);
        future_into_py(py, async move {
            let n = inner.execute(&sql, values).await.map_err(to_pyerr)?;
            Ok(n as u64)
        })
    }

    /// Run a query. Resolves to `bytes` holding the PCB columnar payload, which
    /// `decode_batch()` in the Python wrapper turns into zero-copy memoryviews.
    fn query<'p>(
        &self,
        py: Python<'p>,
        sql: String,
        params: Option<Vec<Option<Param>>>,
    ) -> PyResult<Bound<'p, PyAny>> {
        let inner = self.inner.clone();
        let values = to_values(params);
        future_into_py(py, async move {
            // shared(Arc) 경로: 캐시 히트 시 query_bytes()가 하던 전체 버퍼
            // 복제를 건너뛰고, PyBytes 생성의 1회 복사만 남긴다.
            let bytes = inner
                .query_bytes_shared(&sql, values)
                .await
                .map_err(to_pyerr)?;
            Python::attach(|py| Ok(PyBytes::new(py, bytes.as_ref()).unbind()))
        })
    }
}

/// The native extension module (`powder._powder`).
#[pymodule]
fn _powder(m: &Bound<'_, PyModule>) -> PyResult<()> {
    // Configure the Tokio runtime that backs the async bridge.
    let mut builder = tokio::runtime::Builder::new_multi_thread();
    builder.enable_all();
    pyo3_async_runtimes::tokio::init(builder);

    m.add_function(wrap_pyfunction!(connect, m)?)?;
    m.add_class::<Client>()?;
    Ok(())
}
