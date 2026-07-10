//go:build !windows

package powder

/*
#cgo LDFLAGS: -ldl
#include <dlfcn.h>
#include <stdlib.h>
#include <stdint.h>

// Function pointers resolved from the shared library at Load() time.
typedef void* (*powder_connect_fn)(const char*);
typedef long long (*powder_execute_fn)(void*, const char*, const char*);
typedef unsigned char* (*powder_query_fn)(void*, const char*, const char*, size_t*);
typedef void (*powder_free_buffer_fn)(unsigned char*, size_t);
typedef void (*powder_close_fn)(void*);
typedef const char* (*powder_last_error_fn)(void);
typedef void* (*powder_orm_schema_new_fn)(const char*);
typedef void (*powder_orm_schema_free_fn)(void*);
typedef long long (*powder_orm_execute_fn)(void*, const void*, const char*);
typedef unsigned char* (*powder_orm_find_json_fn)(void*, const void*, const char*, size_t*);

static powder_connect_fn     p_connect;
static powder_execute_fn     p_execute;
static powder_query_fn       p_query;
static powder_free_buffer_fn p_free_buffer;
static powder_close_fn       p_close;
static powder_last_error_fn  p_last_error;
static powder_orm_schema_new_fn  p_orm_schema_new;
static powder_orm_schema_free_fn p_orm_schema_free;
static powder_orm_execute_fn     p_orm_execute;
static powder_orm_find_json_fn   p_orm_find_json;

// Returns NULL on success, else the dlerror() message.
static const char* powder_go_load(const char* path) {
    void* h = dlopen(path, RTLD_NOW | RTLD_LOCAL);
    if (!h) return dlerror();
    p_connect     = (powder_connect_fn)     dlsym(h, "powder_connect");
    p_execute     = (powder_execute_fn)     dlsym(h, "powder_execute");
    p_query       = (powder_query_fn)       dlsym(h, "powder_query");
    p_free_buffer = (powder_free_buffer_fn) dlsym(h, "powder_free_buffer");
    p_close       = (powder_close_fn)       dlsym(h, "powder_close");
    p_last_error  = (powder_last_error_fn)  dlsym(h, "powder_last_error");
    p_orm_schema_new  = (powder_orm_schema_new_fn)  dlsym(h, "powder_orm_schema_new");
    p_orm_schema_free = (powder_orm_schema_free_fn) dlsym(h, "powder_orm_schema_free");
    p_orm_execute     = (powder_orm_execute_fn)     dlsym(h, "powder_orm_execute");
    p_orm_find_json   = (powder_orm_find_json_fn)   dlsym(h, "powder_orm_find_json");
    if (!p_connect || !p_execute || !p_query || !p_free_buffer || !p_close || !p_last_error
        || !p_orm_schema_new || !p_orm_schema_free || !p_orm_execute || !p_orm_find_json) {
        return "powder_ffi symbols missing from shared library";
    }
    return NULL;
}

static void*       powder_go_connect(const char* url) { return p_connect(url); }
static long long   powder_go_execute(void* h, const char* sql, const char* params) { return p_execute(h, sql, params); }
static unsigned char* powder_go_query(void* h, const char* sql, const char* params, size_t* out_len) { return p_query(h, sql, params, out_len); }
static void        powder_go_free_buffer(unsigned char* p, size_t n) { p_free_buffer(p, n); }
static void        powder_go_close(void* h) { p_close(h); }
static const char* powder_go_last_error(void) { return p_last_error(); }
static void*       powder_go_orm_schema_new(const char* json) { return p_orm_schema_new(json); }
static void        powder_go_orm_schema_free(void* s) { p_orm_schema_free(s); }
static long long   powder_go_orm_execute(void* h, const void* s, const char* op) { return p_orm_execute(h, s, op); }
static unsigned char* powder_go_orm_find_json(void* h, const void* s, const char* op, size_t* out_len) { return p_orm_find_json(h, s, op, out_len); }
*/
import "C"

import (
	"errors"
	"fmt"
	"runtime"
	"sync"
	"unsafe"
)

// Unix loader: dlopen()s libpowder_ffi.so / .dylib via cgo.
var (
	loadOnce sync.Once
	loadErr  error
	loaded   bool
)

// Load binds the native Powder library from an absolute path. Call once before
// Connect; subsequent calls are no-ops.
func Load(path string) error {
	loadOnce.Do(func() {
		cpath := C.CString(path)
		defer C.free(unsafe.Pointer(cpath))
		if msg := C.powder_go_load(cpath); msg != nil {
			loadErr = fmt.Errorf("powder: cannot load %s: %s", path, C.GoString(msg))
			return
		}
		loaded = true
	})
	return loadErr
}

func ensureLoaded() error {
	if loadErr != nil {
		return loadErr
	}
	if !loaded {
		return errors.New("powder: call Load(pathToNativeLibrary) before Connect")
	}
	return nil
}

// lastError reads the thread-local message the native layer stored, with the
// goroutine still pinned to the OS thread that made the failing call.
func lastError(fallback string) error {
	if msg := C.powder_go_last_error(); msg != nil {
		return errors.New("powder: " + C.GoString(msg))
	}
	return errors.New("powder: " + fallback)
}

func nativeConnect(url string) (uintptr, error) {
	// The native error slot is thread-local; keep this goroutine pinned so the
	// failing call and powder_last_error() run on the same OS thread.
	runtime.LockOSThread()
	defer runtime.UnlockOSThread()

	curl := C.CString(url)
	defer C.free(unsafe.Pointer(curl))
	h := C.powder_go_connect(curl)
	if h == nil {
		return 0, lastError("connect failed")
	}
	return uintptr(h), nil
}

func nativeExecute(handle uintptr, sql, paramsJSON string) (int64, error) {
	runtime.LockOSThread()
	defer runtime.UnlockOSThread()

	csql := C.CString(sql)
	defer C.free(unsafe.Pointer(csql))
	cpar := C.CString(paramsJSON)
	defer C.free(unsafe.Pointer(cpar))

	n := int64(C.powder_go_execute(unsafe.Pointer(handle), csql, cpar))
	if n < 0 {
		return 0, lastError("execute failed")
	}
	return n, nil
}

func nativeQuery(handle uintptr, sql, paramsJSON string) ([]byte, error) {
	runtime.LockOSThread()
	defer runtime.UnlockOSThread()

	csql := C.CString(sql)
	defer C.free(unsafe.Pointer(csql))
	cpar := C.CString(paramsJSON)
	defer C.free(unsafe.Pointer(cpar))

	var outLen C.size_t
	ptr := C.powder_go_query(unsafe.Pointer(handle), csql, cpar, &outLen)
	if ptr == nil {
		return nil, lastError("query failed")
	}
	// Copy out of native memory, then hand the allocation back.
	out := C.GoBytes(unsafe.Pointer(ptr), C.int(outLen))
	C.powder_go_free_buffer(ptr, outLen)
	return out, nil
}

func nativeClose(handle uintptr) {
	C.powder_go_close(unsafe.Pointer(handle))
}

func nativeOrmSchemaNew(schemaJSON string) (uintptr, error) {
	runtime.LockOSThread()
	defer runtime.UnlockOSThread()

	cjson := C.CString(schemaJSON)
	defer C.free(unsafe.Pointer(cjson))
	h := C.powder_go_orm_schema_new(cjson)
	if h == nil {
		return 0, lastError("schema parse failed")
	}
	return uintptr(h), nil
}

func nativeOrmSchemaFree(schema uintptr) {
	C.powder_go_orm_schema_free(unsafe.Pointer(schema))
}

func nativeOrmExecute(handle, schema uintptr, opJSON string) (int64, error) {
	runtime.LockOSThread()
	defer runtime.UnlockOSThread()

	cop := C.CString(opJSON)
	defer C.free(unsafe.Pointer(cop))
	n := int64(C.powder_go_orm_execute(unsafe.Pointer(handle), unsafe.Pointer(schema), cop))
	if n < 0 {
		return 0, lastError("orm execute failed")
	}
	return n, nil
}

func nativeOrmFindJSON(handle, schema uintptr, opJSON string) (string, error) {
	runtime.LockOSThread()
	defer runtime.UnlockOSThread()

	cop := C.CString(opJSON)
	defer C.free(unsafe.Pointer(cop))
	var outLen C.size_t
	ptr := C.powder_go_orm_find_json(unsafe.Pointer(handle), unsafe.Pointer(schema), cop, &outLen)
	if ptr == nil {
		return "", lastError("orm find failed")
	}
	out := C.GoBytes(unsafe.Pointer(ptr), C.int(outLen))
	C.powder_go_free_buffer(ptr, outLen)
	return string(out), nil
}
