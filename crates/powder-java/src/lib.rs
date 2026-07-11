//! Java (JNI) bindings for the Powder engine.
//!
//! The native layer is deliberately thin — it owns the async connection and
//! returns query results as the raw PCB byte buffer (surfaced to the JVM as a
//! `byte[]`, which the pure-Java `PcbReader` turns into typed columns). The
//! fluent query builder and the columnar reader live in Java, mirroring the
//! Node (napi) and Python (PyO3) bindings.
//!
//! Bound parameters cross the boundary as a JSON array string (`[1,"a",true,
//! null]`), so the JNI surface stays four methods wide with no object-array
//! reflection.
//!
//! Native methods (class `com.powder.PowderNative`):
//! - `connect(String url) -> long`   handle to a boxed [`Client`]
//! - `execute(long, String sql, String paramsJson) -> long`   rows affected
//! - `query(long, String sql, String paramsJson) -> byte[]`   PCB payload
//! - `close(long)`   frees the connection

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use jni::objects::{JByteBuffer, JClass, JObject, JString};
use jni::sys::{jbyteArray, jlong, jobject};
use jni::JNIEnv;
use tokio::runtime::Runtime;

use powder_core::{Client, Value};

// The columnar build path is allocation-heavy; the platform default heap
// (especially on Windows) measurably slows cold queries. Same setup as the
// Node binding.
#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

/// Shared multi-thread runtime backing every blocking JNI call.
fn rt() -> &'static Runtime {
    static RT: OnceLock<Runtime> = OnceLock::new();
    RT.get_or_init(|| Runtime::new().expect("powder JNI: failed to start tokio runtime"))
}

/// PCB buffers handed to the JVM as `DirectByteBuffer`s, keyed by data
/// address. Holding the `Arc` here keeps the (possibly cache-shared) buffer
/// alive without copying it; `freeBuffer` drops the reference. The `Vec`
/// handles the same cached buffer being handed out more than once before the
/// first `freeBuffer` (identical address).
fn direct_bufs() -> &'static Mutex<HashMap<usize, Vec<Arc<Vec<u8>>>>> {
    type BufferMap = Mutex<HashMap<usize, Vec<Arc<Vec<u8>>>>>;
    static M: OnceLock<BufferMap> = OnceLock::new();
    M.get_or_init(|| Mutex::new(HashMap::new()))
}

fn jstr(env: &mut JNIEnv, s: &JString) -> String {
    match env.get_string(s) {
        Ok(js) => js.into(),
        Err(_) => String::new(),
    }
}

/// Parse a JSON array of bound parameters into core [`Value`]s. Integers map
/// to `Int`, other numbers to `Float`, matching the Node/Python heuristics.
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

fn throw(env: &mut JNIEnv, msg: impl AsRef<str>) {
    let _ = env.throw_new("java/lang/RuntimeException", msg.as_ref());
}

/// Reconstruct the boxed client from a handle without taking ownership.
///
/// # Safety
/// `handle` must be a live pointer returned by `connect` and not yet `close`d.
unsafe fn client<'a>(handle: jlong) -> &'a Client {
    &*(handle as *const Client)
}

#[no_mangle]
pub extern "system" fn Java_com_powder_PowderNative_connect<'l>(
    mut env: JNIEnv<'l>,
    _class: JClass<'l>,
    url: JString<'l>,
) -> jlong {
    let url = jstr(&mut env, &url);
    match rt().block_on(Client::connect(&url)) {
        Ok(c) => Box::into_raw(Box::new(c)) as jlong,
        Err(e) => {
            throw(&mut env, e.to_string());
            0
        }
    }
}

#[no_mangle]
pub extern "system" fn Java_com_powder_PowderNative_execute<'l>(
    mut env: JNIEnv<'l>,
    _class: JClass<'l>,
    handle: jlong,
    sql: JString<'l>,
    params: JString<'l>,
) -> jlong {
    let sql = jstr(&mut env, &sql);
    let pjson = jstr(&mut env, &params);
    let vals = match parse_params(&pjson) {
        Ok(v) => v,
        Err(e) => {
            throw(&mut env, e);
            return 0;
        }
    };
    let client = unsafe { client(handle) };
    match rt().block_on(client.execute(&sql, vals)) {
        Ok(n) => n as jlong,
        Err(e) => {
            throw(&mut env, e.to_string());
            0
        }
    }
}

#[no_mangle]
pub extern "system" fn Java_com_powder_PowderNative_query<'l>(
    mut env: JNIEnv<'l>,
    _class: JClass<'l>,
    handle: jlong,
    sql: JString<'l>,
    params: JString<'l>,
) -> jbyteArray {
    let sql = jstr(&mut env, &sql);
    let pjson = jstr(&mut env, &params);
    let vals = match parse_params(&pjson) {
        Ok(v) => v,
        Err(e) => {
            throw(&mut env, e);
            return std::ptr::null_mut();
        }
    };
    let client = unsafe { client(handle) };
    // shared(Arc) 경로: 캐시 히트 시 query_bytes()가 하던 전체 버퍼 복제를
    // 건너뛰고 byte[] 생성의 1회 복사만 남긴다.
    match rt().block_on(client.query_bytes_shared(&sql, vals)) {
        Ok(bytes) => match env.byte_array_from_slice(bytes.as_ref()) {
            Ok(arr) => arr.into_raw(),
            Err(e) => {
                throw(&mut env, e.to_string());
                std::ptr::null_mut()
            }
        },
        Err(e) => {
            throw(&mut env, e.to_string());
            std::ptr::null_mut()
        }
    }
}

/// Zero-copy variant of [`Java_com_powder_PowderNative_query`]: hands the JVM a
/// `DirectByteBuffer` aliasing the PCB bytes instead of copying them into a
/// `byte[]`. The Rust allocation is leaked here and reclaimed by
/// `freeBuffer(address, length)`, which the Java `Batch` calls on close.
#[no_mangle]
pub extern "system" fn Java_com_powder_PowderNative_queryDirect<'l>(
    mut env: JNIEnv<'l>,
    _class: JClass<'l>,
    handle: jlong,
    sql: JString<'l>,
    params: JString<'l>,
) -> jobject {
    let sql = jstr(&mut env, &sql);
    let pjson = jstr(&mut env, &params);
    let vals = match parse_params(&pjson) {
        Ok(v) => v,
        Err(e) => {
            throw(&mut env, e);
            return std::ptr::null_mut();
        }
    };
    let client = unsafe { client(handle) };
    let bytes = match rt().block_on(client.query_bytes_shared(&sql, vals)) {
        Ok(b) => b,
        Err(e) => {
            throw(&mut env, e.to_string());
            return std::ptr::null_mut();
        }
    };

    let ptr = bytes.as_ptr() as *mut u8;
    let len = bytes.len();
    if len == 0 {
        throw(&mut env, "empty PCB payload");
        return std::ptr::null_mut();
    }
    // 무복사 핸드오프: Arc를 레지스트리에 보관해 버퍼를 살려두고 그 데이터
    // 포인터를 그대로 JVM에 넘긴다. 캐시 히트 시 복사가 0회가 된다.
    // `freeBuffer(address, length)`가 레지스트리에서 참조를 내린다.
    direct_bufs()
        .lock()
        .unwrap()
        .entry(ptr as usize)
        .or_default()
        .push(bytes);
    // Safety: the registry keeps the allocation alive until the matching
    // `freeBuffer` call; the JVM only reads through the buffer.
    match unsafe { env.new_direct_byte_buffer(ptr, len) } {
        Ok(buf) => buf.into_raw(),
        Err(e) => {
            release_direct_buf(ptr as usize);
            throw(&mut env, e.to_string());
            std::ptr::null_mut()
        }
    }
}

/// Drop one registry reference for `addr`. Returns true when it was a
/// registered shared buffer.
fn release_direct_buf(addr: usize) -> bool {
    let mut map = direct_bufs().lock().unwrap();
    match map.get_mut(&addr) {
        Some(v) => {
            v.pop();
            if v.is_empty() {
                map.remove(&addr);
            }
            true
        }
        None => false,
    }
}

/// Native address of a direct `ByteBuffer`, so Java can pass it back to
/// [`Java_com_powder_PowderNative_freeBuffer`] without `sun.misc.Unsafe`.
#[no_mangle]
pub extern "system" fn Java_com_powder_PowderNative_bufferAddress<'l>(
    env: JNIEnv<'l>,
    _class: JClass<'l>,
    buf: JObject<'l>,
) -> jlong {
    match env.get_direct_buffer_address(&JByteBuffer::from(buf)) {
        Ok(p) => p as jlong,
        Err(_) => 0,
    }
}

/// Reclaim the allocation behind a buffer produced by `queryDirect`.
#[no_mangle]
pub extern "system" fn Java_com_powder_PowderNative_freeBuffer(
    _env: JNIEnv,
    _class: JClass,
    address: jlong,
    length: jlong,
) {
    if address == 0 || length <= 0 {
        return;
    }
    // Shared PCB buffers live in the registry; dropping the Arc reference is
    // the whole "free". The Box path remains as a fallback for any older
    // caller still holding a `Box::into_raw` buffer.
    if release_direct_buf(address as usize) {
        return;
    }
    // Safety: mirrors the historical `Box::into_raw(into_boxed_slice())` handoff.
    unsafe {
        drop(Box::from_raw(std::ptr::slice_from_raw_parts_mut(
            address as *mut u8,
            length as usize,
        )));
    }
}

#[no_mangle]
pub extern "system" fn Java_com_powder_PowderNative_close(
    _env: JNIEnv,
    _class: JClass,
    handle: jlong,
) {
    if handle != 0 {
        // Reclaim and drop the boxed client.
        unsafe { drop(Box::from_raw(handle as *mut Client)) };
    }
}

// ---------------------------------------------------------------------------
// ORM: the shared engine (powder_core::orm) over JNI.
//
// A schema handle is parsed once from `powder.schema.json` text; each op then
// crosses as one JSON string — the same operation spec in every language.
// ---------------------------------------------------------------------------

use powder_core::orm::{Orm, OrmSchema};

/// Reconstruct the boxed schema from a handle without taking ownership.
///
/// # Safety
/// `handle` must be live from `ormSchemaNew` and not yet `ormSchemaFree`d.
unsafe fn orm_schema<'a>(handle: jlong) -> &'a OrmSchema {
    &*(handle as *const OrmSchema)
}

#[no_mangle]
pub extern "system" fn Java_com_powder_PowderNative_ormSchemaNew<'l>(
    mut env: JNIEnv<'l>,
    _class: JClass<'l>,
    schema_json: JString<'l>,
) -> jlong {
    let json = jstr(&mut env, &schema_json);
    match OrmSchema::parse(&json) {
        Ok(s) => Box::into_raw(Box::new(s)) as jlong,
        Err(e) => {
            throw(&mut env, e.to_string());
            0
        }
    }
}

#[no_mangle]
pub extern "system" fn Java_com_powder_PowderNative_ormSchemaFree(
    _env: JNIEnv,
    _class: JClass,
    handle: jlong,
) {
    if handle != 0 {
        unsafe { drop(Box::from_raw(handle as *mut OrmSchema)) };
    }
}

#[no_mangle]
pub extern "system" fn Java_com_powder_PowderNative_ormExecute<'l>(
    mut env: JNIEnv<'l>,
    _class: JClass<'l>,
    handle: jlong,
    schema: jlong,
    op_json: JString<'l>,
) -> jlong {
    let op = jstr(&mut env, &op_json);
    let op: serde_json::Value = match serde_json::from_str(&op) {
        Ok(v) => v,
        Err(e) => {
            throw(&mut env, format!("op is not valid JSON: {e}"));
            return 0;
        }
    };
    let orm = Orm::new(
        unsafe { client(handle) }.clone(),
        unsafe { orm_schema(schema) }.clone(),
    );
    match rt().block_on(orm.execute(&op)) {
        Ok(n) => n,
        Err(e) => {
            throw(&mut env, e.to_string());
            0
        }
    }
}

#[no_mangle]
pub extern "system" fn Java_com_powder_PowderNative_ormFindJson<'l>(
    mut env: JNIEnv<'l>,
    _class: JClass<'l>,
    handle: jlong,
    schema: jlong,
    op_json: JString<'l>,
) -> jni::sys::jstring {
    let op = jstr(&mut env, &op_json);
    let op: serde_json::Value = match serde_json::from_str(&op) {
        Ok(v) => v,
        Err(e) => {
            throw(&mut env, format!("op is not valid JSON: {e}"));
            return std::ptr::null_mut();
        }
    };
    let orm = Orm::new(
        unsafe { client(handle) }.clone(),
        unsafe { orm_schema(schema) }.clone(),
    );
    match rt().block_on(orm.find_json(&op)) {
        Ok(rows) => match env.new_string(rows.to_string()) {
            Ok(s) => s.into_raw(),
            Err(e) => {
                throw(&mut env, e.to_string());
                std::ptr::null_mut()
            }
        },
        Err(e) => {
            throw(&mut env, e.to_string());
            std::ptr::null_mut()
        }
    }
}
