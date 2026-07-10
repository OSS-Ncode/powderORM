//! C ABI for the Powder engine.
//!
//! A stable, dependency-free surface any C-compatible host can call — used by
//! the Go binding (`bindings/go`) and available to C/C++/Zig/etc. The design
//! mirrors the other bindings: the native layer owns the async connection and
//! hands back the raw PCB byte buffer; the host decodes it.
//!
//! Bound parameters cross as a JSON array string (`[1,"a",true,null]`), which
//! keeps the ABI to plain pointers and integers.
//!
//! ## Error handling
//!
//! Fallible functions signal failure with a sentinel return (`NULL` for
//! pointers, `-1` for counts) and store a message retrievable via
//! [`powder_last_error`], which is thread-local and valid until the next
//! failing call **on the same thread**.
//!
//! ## Memory ownership
//!
//! - [`powder_connect`] returns a handle freed by [`powder_close`].
//! - [`powder_query`] returns a buffer freed by [`powder_free_buffer`] with the
//!   exact `len` it reported.
//! - [`powder_last_error`] returns a borrowed pointer; copy it before the next
//!   call on that thread.

use std::cell::RefCell;
use std::ffi::{c_char, CStr, CString};
use std::sync::OnceLock;

use tokio::runtime::Runtime;

use powder_core::orm::{Orm, OrmSchema};
use powder_core::{Client, Value};

thread_local! {
    static LAST_ERROR: RefCell<Option<CString>> = const { RefCell::new(None) };
}

fn set_error(msg: impl Into<Vec<u8>>) {
    let cs = CString::new(msg).unwrap_or_else(|_| CString::new("error contained a NUL byte").unwrap());
    LAST_ERROR.with(|e| *e.borrow_mut() = Some(cs));
}

fn clear_error() {
    LAST_ERROR.with(|e| *e.borrow_mut() = None);
}

/// Shared multi-thread runtime backing every blocking FFI call.
fn rt() -> &'static Runtime {
    static RT: OnceLock<Runtime> = OnceLock::new();
    RT.get_or_init(|| Runtime::new().expect("powder FFI: failed to start tokio runtime"))
}

/// Read a borrowed C string. Returns `None` for NULL or invalid UTF-8.
///
/// # Safety
/// `p` must be NULL or point to a NUL-terminated string.
unsafe fn cstr(p: *const c_char) -> Option<&'static str> {
    if p.is_null() {
        return None;
    }
    CStr::from_ptr(p).to_str().ok()
}

/// Parse a JSON array of bound parameters into core [`Value`]s. Integers map
/// to `Int`, other numbers to `Float` — same heuristic as the other bindings.
fn parse_params(json: &str) -> Result<Vec<Value>, String> {
    let json = json.trim();
    if json.is_empty() || json == "[]" {
        return Ok(Vec::new());
    }
    let parsed: serde_json::Value = serde_json::from_str(json).map_err(|e| e.to_string())?;
    let arr = parsed
        .as_array()
        .ok_or_else(|| "params must be a JSON array".to_string())?;
    arr.iter()
        .map(|e| match e {
            serde_json::Value::Null => Ok(Value::Null),
            serde_json::Value::Bool(b) => Ok(Value::Bool(*b)),
            serde_json::Value::Number(n) => {
                if let Some(i) = n.as_i64() {
                    Ok(Value::Int(i))
                } else {
                    Ok(Value::Float(n.as_f64().unwrap_or(0.0)))
                }
            }
            serde_json::Value::String(s) => Ok(Value::Text(s.clone())),
            other => Err(format!("unsupported parameter: {other}")),
        })
        .collect()
}

/// Message for the last failing call on this thread, or NULL if none.
///
/// The pointer is owned by Powder and stays valid until the next failing call
/// on the same thread.
#[no_mangle]
pub extern "C" fn powder_last_error() -> *const c_char {
    LAST_ERROR.with(|e| match &*e.borrow() {
        Some(cs) => cs.as_ptr(),
        None => std::ptr::null(),
    })
}

/// Open a connection. Returns a handle, or NULL on failure.
///
/// # Safety
/// `url` must be a NUL-terminated UTF-8 string.
#[no_mangle]
pub unsafe extern "C" fn powder_connect(url: *const c_char) -> *mut Client {
    clear_error();
    let Some(url) = cstr(url) else {
        set_error("url must be a valid UTF-8 C string");
        return std::ptr::null_mut();
    };
    match rt().block_on(Client::connect(url)) {
        Ok(c) => Box::into_raw(Box::new(c)),
        Err(e) => {
            set_error(e.to_string());
            std::ptr::null_mut()
        }
    }
}

/// Run a non-row statement. Returns rows affected, or -1 on failure.
///
/// # Safety
/// `handle` must come from [`powder_connect`] and not yet be closed; `sql` and
/// `params_json` must be NUL-terminated UTF-8 (`params_json` may be NULL).
#[no_mangle]
pub unsafe extern "C" fn powder_execute(
    handle: *mut Client,
    sql: *const c_char,
    params_json: *const c_char,
) -> i64 {
    clear_error();
    if handle.is_null() {
        set_error("null client handle");
        return -1;
    }
    let Some(sql) = cstr(sql) else {
        set_error("sql must be a valid UTF-8 C string");
        return -1;
    };
    let pjson = cstr(params_json).unwrap_or("[]");
    let vals = match parse_params(pjson) {
        Ok(v) => v,
        Err(e) => {
            set_error(e);
            return -1;
        }
    };
    match rt().block_on((*handle).execute(sql, vals)) {
        Ok(n) => n as i64,
        Err(e) => {
            set_error(e.to_string());
            -1
        }
    }
}

/// Run a query. On success returns a pointer to `*out_len` PCB bytes (free it
/// with [`powder_free_buffer`]); on failure returns NULL.
///
/// # Safety
/// As [`powder_execute`], plus `out_len` must be a valid `usize` pointer.
#[no_mangle]
pub unsafe extern "C" fn powder_query(
    handle: *mut Client,
    sql: *const c_char,
    params_json: *const c_char,
    out_len: *mut usize,
) -> *mut u8 {
    clear_error();
    if handle.is_null() || out_len.is_null() {
        set_error("null client handle or out_len");
        return std::ptr::null_mut();
    }
    *out_len = 0;
    let Some(sql) = cstr(sql) else {
        set_error("sql must be a valid UTF-8 C string");
        return std::ptr::null_mut();
    };
    let pjson = cstr(params_json).unwrap_or("[]");
    let vals = match parse_params(pjson) {
        Ok(v) => v,
        Err(e) => {
            set_error(e);
            return std::ptr::null_mut();
        }
    };
    match rt().block_on((*handle).query_bytes(sql, vals)) {
        Ok(bytes) => {
            // `into_boxed_slice` makes capacity == length, so `free_buffer` can
            // rebuild the exact allocation from (pointer, len) alone.
            let boxed: Box<[u8]> = bytes.into_boxed_slice();
            let len = boxed.len();
            *out_len = len;
            Box::into_raw(boxed) as *mut u8
        }
        Err(e) => {
            set_error(e.to_string());
            std::ptr::null_mut()
        }
    }
}

/// Copy `len` bytes from a [`powder_query`] buffer into caller-owned memory.
///
/// Hosts that cannot safely dereference a foreign pointer (Go, for one — the
/// runtime forbids turning an arbitrary `uintptr` into a pointer) call this
/// with a pointer into their own allocation instead.
///
/// # Safety
/// `src` must be a live [`powder_query`] buffer of at least `len` bytes and
/// `dst` must be writable for `len` bytes; the ranges must not overlap.
#[no_mangle]
pub unsafe extern "C" fn powder_copy_out(src: *const u8, len: usize, dst: *mut u8) {
    if src.is_null() || dst.is_null() || len == 0 {
        return;
    }
    std::ptr::copy_nonoverlapping(src, dst, len);
}

/// Copy the last error message for this thread into `dst` (no NUL terminator).
///
/// Returns the message's full length in bytes. When that exceeds `cap` the
/// message was truncated — call again with a larger buffer. Returns 0 when
/// there is no error.
///
/// # Safety
/// `dst` must be writable for `cap` bytes (it may be NULL when `cap` is 0,
/// which is the way to query the required length).
#[no_mangle]
pub unsafe extern "C" fn powder_last_error_copy(dst: *mut u8, cap: usize) -> usize {
    LAST_ERROR.with(|e| match &*e.borrow() {
        None => 0,
        Some(cs) => {
            let bytes = cs.as_bytes();
            let n = bytes.len().min(cap);
            if n > 0 && !dst.is_null() {
                std::ptr::copy_nonoverlapping(bytes.as_ptr(), dst, n);
            }
            bytes.len()
        }
    })
}

/// Free a buffer returned by [`powder_query`].
///
/// # Safety
/// `ptr`/`len` must be exactly what [`powder_query`] reported, freed once.
#[no_mangle]
pub unsafe extern "C" fn powder_free_buffer(ptr: *mut u8, len: usize) {
    if ptr.is_null() || len == 0 {
        return;
    }
    drop(Box::from_raw(std::ptr::slice_from_raw_parts_mut(ptr, len)));
}

/// Close a connection opened by [`powder_connect`].
///
/// # Safety
/// `handle` must come from [`powder_connect`] and be closed at most once.
#[no_mangle]
pub unsafe extern "C" fn powder_close(handle: *mut Client) {
    if !handle.is_null() {
        drop(Box::from_raw(handle));
    }
}

// ---------------------------------------------------------------------------
// ORM: the shared engine (powder_core::orm) over the C ABI.
//
// A schema handle is parsed once from `powder.schema.json` text; each op then
// crosses as one JSON object. Mutations return an affected count, row-returning
// ops return a JSON string — the same operation spec in every language.
// ---------------------------------------------------------------------------

/// Parse `powder.schema.json` text into a schema handle for the ORM calls.
/// Returns NULL on failure (see [`powder_last_error`]); free with
/// [`powder_orm_schema_free`].
///
/// # Safety
/// `schema_json` must be a NUL-terminated UTF-8 string.
#[no_mangle]
pub unsafe extern "C" fn powder_orm_schema_new(schema_json: *const c_char) -> *mut OrmSchema {
    clear_error();
    let Some(json) = cstr(schema_json) else {
        set_error("schema_json must be a valid UTF-8 C string");
        return std::ptr::null_mut();
    };
    match OrmSchema::parse(json) {
        Ok(s) => Box::into_raw(Box::new(s)),
        Err(e) => {
            set_error(e.to_string());
            std::ptr::null_mut()
        }
    }
}

/// Free a schema handle from [`powder_orm_schema_new`].
///
/// # Safety
/// `schema` must come from [`powder_orm_schema_new`] and be freed at most once.
#[no_mangle]
pub unsafe extern "C" fn powder_orm_schema_free(schema: *mut OrmSchema) {
    if !schema.is_null() {
        drop(Box::from_raw(schema));
    }
}

unsafe fn orm_of(handle: *mut Client, schema: *const OrmSchema) -> Option<Orm> {
    if handle.is_null() || schema.is_null() {
        set_error("null client or schema handle");
        return None;
    }
    Some(Orm::new((*handle).clone(), (*schema).clone()))
}

/// Run a mutation (or `count`) ORM op: `create`, `createMany`, `update`,
/// `delete`, `deleteAll`, `count`. Returns the affected/row count, or -1 on
/// failure.
///
/// # Safety
/// `handle`/`schema` must be live handles from this library; `op_json` must be
/// a NUL-terminated UTF-8 string.
#[no_mangle]
pub unsafe extern "C" fn powder_orm_execute(
    handle: *mut Client,
    schema: *const OrmSchema,
    op_json: *const c_char,
) -> i64 {
    clear_error();
    let Some(orm) = orm_of(handle, schema) else { return -1 };
    let Some(op) = cstr(op_json) else {
        set_error("op_json must be a valid UTF-8 C string");
        return -1;
    };
    let op: serde_json::Value = match serde_json::from_str(op) {
        Ok(v) => v,
        Err(e) => {
            set_error(format!("op is not valid JSON: {e}"));
            return -1;
        }
    };
    match rt().block_on(orm.execute(&op)) {
        Ok(n) => n,
        Err(e) => {
            set_error(e.to_string());
            -1
        }
    }
}

/// Run a row-returning ORM op: `findMany`, `findFirst`, `groupBy`,
/// `aggregate`. On success returns a pointer to `*out_len` UTF-8 JSON bytes
/// (not NUL-terminated — free with [`powder_free_buffer`], same convention as
/// [`powder_query`]); on failure returns NULL.
///
/// # Safety
/// As [`powder_orm_execute`], plus `out_len` must be a valid `usize` pointer.
#[no_mangle]
pub unsafe extern "C" fn powder_orm_find_json(
    handle: *mut Client,
    schema: *const OrmSchema,
    op_json: *const c_char,
    out_len: *mut usize,
) -> *mut u8 {
    clear_error();
    if out_len.is_null() {
        set_error("null out_len");
        return std::ptr::null_mut();
    }
    *out_len = 0;
    let Some(orm) = orm_of(handle, schema) else {
        return std::ptr::null_mut();
    };
    let Some(op) = cstr(op_json) else {
        set_error("op_json must be a valid UTF-8 C string");
        return std::ptr::null_mut();
    };
    let op: serde_json::Value = match serde_json::from_str(op) {
        Ok(v) => v,
        Err(e) => {
            set_error(format!("op is not valid JSON: {e}"));
            return std::ptr::null_mut();
        }
    };
    match rt().block_on(orm.find_json(&op)) {
        Ok(rows) => {
            let boxed: Box<[u8]> = rows.to_string().into_bytes().into_boxed_slice();
            *out_len = boxed.len();
            Box::into_raw(boxed) as *mut u8
        }
        Err(e) => {
            set_error(e.to_string());
            std::ptr::null_mut()
        }
    }
}
