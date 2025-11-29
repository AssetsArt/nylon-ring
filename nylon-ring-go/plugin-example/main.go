package main

/*
#include <stdint.h>
#include <stddef.h>
#include <stdlib.h>
#include <string.h>

// Status codes
typedef uint32_t NrStatus;
#define NR_STATUS_OK 0
#define NR_STATUS_ERR 1
#define NR_STATUS_INVALID 2
#define NR_STATUS_UNSUPPORTED 3
#define NR_STATUS_STREAM_END 4

// ABI types matching Rust #[repr(C)]
typedef struct {
	void* ptr;
	uint32_t len;
	uint32_t _padding;
} NrStr;

typedef struct {
	void* ptr;
	uint64_t len;
} NrBytes;

typedef struct {
	NrStr key;
	NrStr value;
} NrHeader;

typedef struct {
	NrStr path;
	NrStr method;
	NrStr query;
	NrHeader* headers;
	uint32_t headers_len;
	uint32_t _reserved0;
	uint64_t _reserved1;
} NrRequest;

typedef struct {
	void (*send_result)(void* host_ctx, uint64_t sid, NrStatus status, NrBytes payload);
} NrHostVTable;

typedef struct {
	NrBytes (*set_state)(void* host_ctx, uint64_t sid, NrStr key, NrBytes value);
	NrBytes (*get_state)(void* host_ctx, uint64_t sid, NrStr key);
} NrHostExt;

typedef struct {
	NrStatus (*init)(void* plugin_ctx, void* host_ctx, NrHostVTable* host_vtable);
	NrStatus (*handle)(void* plugin_ctx, NrStr entry, uint64_t sid, NrRequest* req, NrBytes payload);
	void (*shutdown)(void* plugin_ctx);
} NrPluginVTable;

typedef struct {
	uint32_t abi_version;
	uint32_t struct_size;
	NrStr name;
	NrStr version;
	void* plugin_ctx;
	NrPluginVTable* vtable;
} NrPluginInfo;

// Forward declarations for exported functions
extern NrStatus plugin_init(void* plugin_ctx, void* host_ctx, NrHostVTable* host_vtable);
extern NrStatus plugin_handle(void* plugin_ctx, NrStr entry, uint64_t sid, NrRequest* req, NrBytes payload);
extern void plugin_shutdown(void* plugin_ctx);

// Helper function to call send_result from Go
static void call_send_result(NrHostVTable* vtable, void* host_ctx, uint64_t sid, NrStatus status, NrBytes payload) {
	if (vtable && vtable->send_result) {
		vtable->send_result(host_ctx, sid, status, payload);
	}
}
*/
import "C"
import (
	"runtime"
	"time"
	"unsafe"
)

// Global host handle (set during init)
var hostHandle struct {
	ctx    unsafe.Pointer
	vtable *C.NrHostVTable
	ext    *C.NrHostExt
}

// Helper to convert NrStr to Go string
func nrStrToString(s C.NrStr) string {
	if s.ptr == nil || s.len == 0 {
		return ""
	}
	return C.GoStringN((*C.char)(s.ptr), C.int(s.len))
}

//export plugin_init
func plugin_init(pluginCtx, hostCtx unsafe.Pointer, hostVTable *C.NrHostVTable) C.NrStatus {
	hostHandle.ctx = hostCtx
	hostHandle.vtable = hostVTable
	// Note: host extension would be retrieved via helper function in real implementation
	hostHandle.ext = nil
	return C.NR_STATUS_OK
}

//export handle_unary
func handle_unary(pluginCtx unsafe.Pointer, entry C.NrStr, sid uint64, req *C.NrRequest, payload C.NrBytes) C.NrStatus {
	// Copy data immediately (non-blocking requirement)
	path := nrStrToString(req.path)
	method := nrStrToString(req.method)

	// Spawn goroutine for background work
	go func() {
		// Simulate work (DB call, network, etc.)
		time.Sleep(2 * time.Second)

		// Prepare response
		response := "OK: " + method + " " + path
		responseBytes := []byte(response)

		// Allocate C memory for response
		cResponse := C.malloc(C.size_t(len(responseBytes)))
		if cResponse == nil {
			return
		}
		C.memcpy(cResponse, unsafe.Pointer(&responseBytes[0]), C.size_t(len(responseBytes)))

		// Call host callback via C helper
		C.call_send_result(
			hostHandle.vtable,
			hostHandle.ctx,
			C.uint64_t(sid),
			C.NR_STATUS_OK,
			C.NrBytes{
				ptr: cResponse,
				len: C.uint64_t(len(responseBytes)),
			},
		)

		// Free the allocated memory after callback
		// Note: In practice, the host will copy the data, so we can free it
		C.free(cResponse)
	}()

	return C.NR_STATUS_OK
}

//export handle_stream
func handle_stream(pluginCtx unsafe.Pointer, entry C.NrStr, sid uint64, req *C.NrRequest, payload C.NrBytes) C.NrStatus {
	// Copy data immediately
	path := nrStrToString(req.path)

	// Spawn goroutine for background work
	go func() {
		// Send 5 frames, 1 per second
		for i := 1; i <= 5; i++ {
			time.Sleep(1 * time.Second)

			// Prepare frame
			frame := "Frame " + string(rune('0'+i)) + "/5 from " + path
			frameBytes := []byte(frame)

			// Allocate C memory for frame
			cFrame := C.malloc(C.size_t(len(frameBytes)))
			if cFrame == nil {
				continue
			}
			C.memcpy(cFrame, unsafe.Pointer(&frameBytes[0]), C.size_t(len(frameBytes)))

			// Send frame via C helper
			C.call_send_result(
				hostHandle.vtable,
				hostHandle.ctx,
				C.uint64_t(sid),
				C.NR_STATUS_OK,
				C.NrBytes{
					ptr: cFrame,
					len: C.uint64_t(len(frameBytes)),
				},
			)

			// Free the allocated memory
			C.free(cFrame)
		}

		// End stream via C helper
		C.call_send_result(
			hostHandle.vtable,
			hostHandle.ctx,
			C.uint64_t(sid),
			C.NR_STATUS_STREAM_END,
			C.NrBytes{ptr: nil, len: 0},
		)
	}()

	return C.NR_STATUS_OK
}

//export plugin_handle
func plugin_handle(pluginCtx unsafe.Pointer, entry C.NrStr, sid uint64, req *C.NrRequest, payload C.NrBytes) C.NrStatus {
	entryStr := nrStrToString(entry)

	switch entryStr {
	case "unary":
		return handle_unary(pluginCtx, entry, sid, req, payload)
	case "stream":
		return handle_stream(pluginCtx, entry, sid, req, payload)
	default:
		return C.NR_STATUS_INVALID
	}
}

//export plugin_shutdown
func plugin_shutdown(pluginCtx unsafe.Pointer) {
	// Cleanup if needed
}

// Static plugin info and vtable
var (
	pluginName    = C.CString("nylon-ring-go-plugin")
	pluginVersion = C.CString("1.0.0")

	pluginVTable = C.NrPluginVTable{
		init:     (*[0]byte)(C.plugin_init),
		handle:   (*[0]byte)(C.plugin_handle),
		shutdown: (*[0]byte)(C.plugin_shutdown),
	}

	pluginInfo = C.NrPluginInfo{
		abi_version: 1,
		struct_size: C.uint32_t(unsafe.Sizeof(C.NrPluginInfo{})),
		name: C.NrStr{
			ptr: unsafe.Pointer(pluginName),
			len: C.uint32_t(C.strlen(pluginName)),
		},
		version: C.NrStr{
			ptr: unsafe.Pointer(pluginVersion),
			len: C.uint32_t(C.strlen(pluginVersion)),
		},
		plugin_ctx: nil,
		vtable:     &pluginVTable,
	}
)

//export nylon_ring_get_plugin_v1
func nylon_ring_get_plugin_v1() *C.NrPluginInfo {
	return &pluginInfo
}

func main() {
	// This is a plugin library, not a standalone program
	// The main function is required but won't be called when loaded as a plugin
	runtime.LockOSThread()
}
