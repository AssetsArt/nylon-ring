package sdk

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
	NrStatus (*handle_raw)(void* plugin_ctx, NrStr entry, uint64_t sid, NrBytes payload);
	void (*shutdown)(void* plugin_ctx);
	NrStatus (*stream_data)(void* plugin_ctx, uint64_t sid, NrBytes data);
	NrStatus (*stream_close)(void* plugin_ctx, uint64_t sid);
} NrPluginVTable;

typedef struct {
	uint32_t abi_version;
	uint32_t struct_size;
	NrStr name;
	NrStr version;
	void* plugin_ctx;
	NrPluginVTable* vtable;
} NrPluginInfo;

// Helper function to call send_result from Go
static void call_send_result(NrHostVTable* vtable, void* host_ctx, uint64_t sid, NrStatus status, NrBytes payload) {
	if (vtable && vtable->send_result) {
		vtable->send_result(host_ctx, sid, status, payload);
	}
}

// Forward declarations for Go exported functions
extern NrStatus go_plugin_init(void* plugin_ctx, void* host_ctx, NrHostVTable* host_vtable);
extern NrStatus go_plugin_handle(void* plugin_ctx, NrStr entry, uint64_t sid, NrRequest* req, NrBytes payload);
extern NrStatus go_plugin_handle_raw(void* plugin_ctx, NrStr entry, uint64_t sid, NrBytes payload);
extern void go_plugin_shutdown(void* plugin_ctx);
extern NrStatus go_plugin_stream_data(void* plugin_ctx, uint64_t sid, NrBytes data);
extern NrStatus go_plugin_stream_close(void* plugin_ctx, uint64_t sid);
*/
import "C"
import (
	"unsafe"
)

// Static vtable - initialized in init()
var staticVTable C.NrPluginVTable

func init() {
	// Initialize static vtable with function pointers to Go exported functions
	staticVTable = C.NrPluginVTable{
		init:         (*[0]byte)(C.go_plugin_init),
		handle:       (*[0]byte)(C.go_plugin_handle),
		handle_raw:   (*[0]byte)(C.go_plugin_handle_raw),
		shutdown:     (*[0]byte)(C.go_plugin_shutdown),
		stream_data:  (*[0]byte)(C.go_plugin_stream_data),
		stream_close: (*[0]byte)(C.go_plugin_stream_close),
	}
}

//export go_plugin_init
func go_plugin_init(pluginCtx, hostCtx unsafe.Pointer, hostVTable *C.NrHostVTable) C.NrStatus {
	if globalPlugin == nil {
		return C.NR_STATUS_OK
	}

	// We need to store the hostVTable pointer.
	// Since plugin.go doesn't import C, we store it as unsafe.Pointer in globalPlugin
	// but here we know it's *C.NrHostVTable.
	// Actually, globalPlugin.hostVTable in plugin.go should be unsafe.Pointer.

	globalPlugin.setHostContext(hostCtx, unsafe.Pointer(hostVTable))

	// Call user's init function
	if err := globalPlugin.callInit(); err != nil {
		return C.NR_STATUS_ERR
	}

	return C.NR_STATUS_OK
}

//export go_plugin_handle
func go_plugin_handle(pluginCtx unsafe.Pointer, entry C.NrStr, sid uint64, req *C.NrRequest, payload C.NrBytes) C.NrStatus {
	if globalPlugin == nil {
		return C.NR_STATUS_ERR
	}

	entryStr := cStrToString(entry)

	// Convert request
	goReq := convertCRequest(req)

	// Convert payload
	var payloadBytes []byte
	if payload.ptr != nil && payload.len > 0 {
		payloadBytes = C.GoBytes(payload.ptr, C.int(payload.len))
	}
	goReq.Body = payloadBytes

	// Call plugin handler
	// We pass a callback that calls sendResultToHost
	// handleRequest takes ownership of goReq and will release it
	err := globalPlugin.handleRequest(entryStr, goReq, payloadBytes, func(status Status, data []byte) {
		sendResultToHost(sid, status, data)
	})

	if err != nil {
		return C.NR_STATUS_INVALID // Or appropriate error
	}

	return C.NR_STATUS_OK
}

//export go_plugin_handle_raw
func go_plugin_handle_raw(pluginCtx unsafe.Pointer, entry C.NrStr, sid uint64, payload C.NrBytes) C.NrStatus {
	if globalPlugin == nil {
		return C.NR_STATUS_ERR
	}

	entryStr := cStrToString(entry)

	// Convert payload
	var payloadBytes []byte
	if payload.ptr != nil && payload.len > 0 {
		payloadBytes = C.GoBytes(payload.ptr, C.int(payload.len))
	}

	// Call plugin handler
	err := globalPlugin.handleRawRequest(entryStr, payloadBytes, func(status Status, data []byte) {
		sendResultToHost(sid, status, data)
	})

	if err != nil {
		return C.NR_STATUS_INVALID
	}

	return C.NR_STATUS_OK
}

//export go_plugin_shutdown
func go_plugin_shutdown(pluginCtx unsafe.Pointer) {
	if globalPlugin != nil {
		globalPlugin.callShutdown()
	}
}

//export go_plugin_stream_data
func go_plugin_stream_data(pluginCtx unsafe.Pointer, sid uint64, data C.NrBytes) C.NrStatus {
	if globalPlugin == nil {
		return C.NR_STATUS_ERR
	}

	// Convert payload
	var dataBytes []byte
	if data.ptr != nil && data.len > 0 {
		dataBytes = C.GoBytes(data.ptr, C.int(data.len))
	}

	err := globalPlugin.handleStreamData(sid, dataBytes, func(status Status, respData []byte) {
		sendResultToHost(sid, status, respData)
	})

	if err != nil {
		return C.NR_STATUS_UNSUPPORTED
	}

	return C.NR_STATUS_OK
}

//export go_plugin_stream_close
func go_plugin_stream_close(pluginCtx unsafe.Pointer, sid uint64) C.NrStatus {
	if globalPlugin == nil {
		return C.NR_STATUS_ERR
	}

	err := globalPlugin.handleStreamClose(sid, func(status Status, respData []byte) {
		sendResultToHost(sid, status, respData)
	})

	if err != nil {
		return C.NR_STATUS_UNSUPPORTED
	}

	return C.NR_STATUS_OK
}

//export nylon_ring_get_plugin_v1
func nylon_ring_get_plugin_v1() *C.NrPluginInfo {
	if globalPlugin == nil {
		return nil
	}

	name, version := globalPlugin.getInfo()
	pluginName := C.CString(name)
	pluginVersion := C.CString(version)

	// Allocate plugin info in C memory
	pluginInfo := (*C.NrPluginInfo)(C.malloc(C.size_t(unsafe.Sizeof(C.NrPluginInfo{}))))
	if pluginInfo == nil {
		return nil
	}

	// Allocate vtable in C memory
	vtable := (*C.NrPluginVTable)(C.malloc(C.size_t(unsafe.Sizeof(C.NrPluginVTable{}))))
	if vtable == nil {
		C.free(unsafe.Pointer(pluginInfo))
		return nil
	}

	*vtable = staticVTable

	*pluginInfo = C.NrPluginInfo{
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
		vtable:     vtable,
	}

	return pluginInfo
}

// sendResultToHost sends the result back to the host using the stored vtable
func sendResultToHost(sid uint64, status Status, data []byte) {
	if globalPlugin == nil {
		return
	}

	vtablePtr := globalPlugin.getHostVTable()
	if vtablePtr == nil {
		return
	}
	vtable := (*C.NrHostVTable)(vtablePtr)
	hostCtx := globalPlugin.getHostContext()

	// Allocate C memory for data
	var cData *C.char
	var dataLen C.uint64_t

	if len(data) > 0 {
		cDataPtr := C.malloc(C.size_t(len(data)))
		if cDataPtr == nil {
			return
		}
		C.memcpy(cDataPtr, unsafe.Pointer(&data[0]), C.size_t(len(data)))
		cData = (*C.char)(cDataPtr)
		dataLen = C.uint64_t(len(data))
	} else {
		cDataPtr := C.malloc(1)
		if cDataPtr == nil {
			return
		}
		cData = (*C.char)(cDataPtr)
		dataLen = 0
	}

	C.call_send_result(
		vtable,
		hostCtx,
		C.uint64_t(sid),
		C.NrStatus(status),
		C.NrBytes{
			ptr: unsafe.Pointer(cData),
			len: dataLen,
		},
	)

	C.free(unsafe.Pointer(cData))
}

// convertCRequest converts a C request to a Go Request.
func convertCRequest(cReq *C.NrRequest) *Request {
	req := acquireRequest()

	req.Method = cStrToString(cReq.method)
	req.Path = cStrToString(cReq.path)
	req.Query = cStrToString(cReq.query)
	// Body is set later

	if cReq.headers != nil && cReq.headers_len > 0 {
		headers := (*[1 << 28]C.NrHeader)(unsafe.Pointer(cReq.headers))[:cReq.headers_len:cReq.headers_len]
		for _, h := range headers {
			key := cStrToString(h.key)
			value := cStrToString(h.value)
			req.Headers[key] = value
		}
	}

	return req
}

func cStrToString(s C.NrStr) string {
	if s.ptr == nil || s.len == 0 {
		return ""
	}
	return C.GoStringN((*C.char)(s.ptr), C.int(s.len))
}
