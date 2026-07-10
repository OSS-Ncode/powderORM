//go:build windows

package powder

import (
	"errors"
	"fmt"
	"runtime"
	"sync"
	"syscall"
	"unsafe"
)

// Windows loader: binds powder_ffi.dll through syscall, so the package builds
// with CGO_ENABLED=0 and needs no C toolchain.
var (
	loadOnce sync.Once
	loadErr  error
	loaded   bool

	procConnect       *syscall.Proc
	procExecute       *syscall.Proc
	procQuery         *syscall.Proc
	procCopyOut       *syscall.Proc
	procFreeBuffer    *syscall.Proc
	procClose         *syscall.Proc
	procLastErrorCopy *syscall.Proc
	procOrmSchemaNew  *syscall.Proc
	procOrmSchemaFree *syscall.Proc
	procOrmExecute    *syscall.Proc
	procOrmFindJSON   *syscall.Proc
)

// Load binds the native Powder library (powder_ffi.dll) from an absolute path.
// Call once before Connect; subsequent calls are no-ops.
func Load(path string) error {
	loadOnce.Do(func() {
		dll, err := syscall.LoadDLL(path)
		if err != nil {
			loadErr = fmt.Errorf("powder: cannot load %s: %w", path, err)
			return
		}
		find := func(name string) *syscall.Proc {
			if loadErr != nil {
				return nil
			}
			p, err := dll.FindProc(name)
			if err != nil {
				loadErr = fmt.Errorf("powder: %s missing from %s: %w", name, path, err)
				return nil
			}
			return p
		}
		procConnect = find("powder_connect")
		procExecute = find("powder_execute")
		procQuery = find("powder_query")
		procCopyOut = find("powder_copy_out")
		procFreeBuffer = find("powder_free_buffer")
		procClose = find("powder_close")
		procLastErrorCopy = find("powder_last_error_copy")
		procOrmSchemaNew = find("powder_orm_schema_new")
		procOrmSchemaFree = find("powder_orm_schema_free")
		procOrmExecute = find("powder_orm_execute")
		procOrmFindJSON = find("powder_orm_find_json")
		loaded = loadErr == nil
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

// cstring returns a NUL-terminated copy plus a pointer to its first byte. The
// caller must keep the slice alive for the duration of the native call.
func cstring(s string) ([]byte, uintptr) {
	b := make([]byte, len(s)+1)
	copy(b, s)
	return b, uintptr(unsafe.Pointer(&b[0]))
}

// lastError reads the thread-local message the native layer stored, copying it
// into Go memory (the runtime forbids dereferencing a foreign uintptr). Go may
// migrate a goroutine between OS threads at a call boundary, so we only read it
// immediately after a failing call, with the goroutine still locked.
func lastError(fallback string) error {
	buf := make([]byte, 256)
	n, _, _ := procLastErrorCopy.Call(uintptr(unsafe.Pointer(&buf[0])), uintptr(len(buf)))
	runtime.KeepAlive(buf)
	if n == 0 {
		return errors.New("powder: " + fallback)
	}
	if int(n) > len(buf) {
		// Truncated — retry with the exact size the native side reported.
		buf = make([]byte, int(n))
		procLastErrorCopy.Call(uintptr(unsafe.Pointer(&buf[0])), uintptr(len(buf)))
		runtime.KeepAlive(buf)
	}
	return errors.New("powder: " + string(buf[:min(int(n), len(buf))]))
}

func min(a, b int) int {
	if a < b {
		return a
	}
	return b
}

func nativeConnect(url string) (uintptr, error) {
	// The native error slot is thread-local; keep this goroutine pinned so the
	// failing call and powder_last_error() run on the same OS thread.
	runtime.LockOSThread()
	defer runtime.UnlockOSThread()

	burl, purl := cstring(url)
	h, _, _ := procConnect.Call(purl)
	runtime.KeepAlive(burl)
	if h == 0 {
		return 0, lastError("connect failed")
	}
	return h, nil
}

func nativeExecute(handle uintptr, sql, paramsJSON string) (int64, error) {
	runtime.LockOSThread()
	defer runtime.UnlockOSThread()

	bsql, psql := cstring(sql)
	bpar, ppar := cstring(paramsJSON)
	r, _, _ := procExecute.Call(handle, psql, ppar)
	runtime.KeepAlive(bsql)
	runtime.KeepAlive(bpar)
	n := int64(r)
	if n < 0 {
		return 0, lastError("execute failed")
	}
	return n, nil
}

func nativeQuery(handle uintptr, sql, paramsJSON string) ([]byte, error) {
	runtime.LockOSThread()
	defer runtime.UnlockOSThread()

	bsql, psql := cstring(sql)
	bpar, ppar := cstring(paramsJSON)
	var outLen uintptr
	ptr, _, _ := procQuery.Call(handle, psql, ppar, uintptr(unsafe.Pointer(&outLen)))
	runtime.KeepAlive(bsql)
	runtime.KeepAlive(bpar)
	if ptr == 0 {
		return nil, lastError("query failed")
	}
	// Copy into Go memory through the native helper (Go must not dereference a
	// foreign uintptr), then hand the allocation back.
	out := make([]byte, int(outLen))
	if outLen > 0 {
		procCopyOut.Call(ptr, outLen, uintptr(unsafe.Pointer(&out[0])))
		runtime.KeepAlive(out)
	}
	procFreeBuffer.Call(ptr, outLen)
	return out, nil
}

func nativeClose(handle uintptr) {
	procClose.Call(handle)
}

func nativeOrmSchemaNew(schemaJSON string) (uintptr, error) {
	runtime.LockOSThread()
	defer runtime.UnlockOSThread()

	bjson, pjson := cstring(schemaJSON)
	h, _, _ := procOrmSchemaNew.Call(pjson)
	runtime.KeepAlive(bjson)
	if h == 0 {
		return 0, lastError("schema parse failed")
	}
	return h, nil
}

func nativeOrmSchemaFree(schema uintptr) {
	procOrmSchemaFree.Call(schema)
}

func nativeOrmExecute(handle, schema uintptr, opJSON string) (int64, error) {
	runtime.LockOSThread()
	defer runtime.UnlockOSThread()

	bop, pop := cstring(opJSON)
	r, _, _ := procOrmExecute.Call(handle, schema, pop)
	runtime.KeepAlive(bop)
	n := int64(r)
	if n < 0 {
		return 0, lastError("orm execute failed")
	}
	return n, nil
}

func nativeOrmFindJSON(handle, schema uintptr, opJSON string) (string, error) {
	runtime.LockOSThread()
	defer runtime.UnlockOSThread()

	bop, pop := cstring(opJSON)
	var outLen uintptr
	ptr, _, _ := procOrmFindJSON.Call(handle, schema, pop, uintptr(unsafe.Pointer(&outLen)))
	runtime.KeepAlive(bop)
	if ptr == 0 {
		return "", lastError("orm find failed")
	}
	// Copy into Go memory through the native helper (Go must not dereference a
	// foreign uintptr), then hand the allocation back — same as nativeQuery.
	out := make([]byte, int(outLen))
	if outLen > 0 {
		procCopyOut.Call(ptr, outLen, uintptr(unsafe.Pointer(&out[0])))
		runtime.KeepAlive(out)
	}
	procFreeBuffer.Call(ptr, outLen)
	return string(out), nil
}
