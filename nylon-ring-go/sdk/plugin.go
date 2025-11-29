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

// Helper function to call send_result from Go
static void call_send_result(NrHostVTable* vtable, void* host_ctx, uint64_t sid, NrStatus status, NrBytes payload) {
	if (vtable && vtable->send_result) {
		vtable->send_result(host_ctx, sid, status, payload);
	}
}

// Forward declarations for Go exported functions
extern NrStatus go_plugin_init(void* plugin_ctx, void* host_ctx, NrHostVTable* host_vtable);
extern NrStatus go_plugin_handle(void* plugin_ctx, NrStr entry, uint64_t sid, NrRequest* req, NrBytes payload);
extern void go_plugin_shutdown(void* plugin_ctx);
*/
import "C"
import (
	"runtime"
	"sync"
	"unsafe"
)

// Status represents the status codes for the Nylon Ring ABI.
type Status uint32

const (
	StatusOk          Status = C.NR_STATUS_OK
	StatusErr         Status = C.NR_STATUS_ERR
	StatusInvalid     Status = C.NR_STATUS_INVALID
	StatusUnsupported Status = C.NR_STATUS_UNSUPPORTED
	StatusStreamEnd   Status = C.NR_STATUS_STREAM_END
)

// Request represents a high-level request with Go types.
type Request struct {
	Method  string
	Path    string
	Query   string
	Headers map[string]string
	Body    []byte
}

// Response represents a response to send back to the host.
type Response struct {
	Status Status
	Data   []byte
}

// Handler is a function that handles a request.
// The SDK automatically calls this in a goroutine, so you can do blocking work.
// Results should be sent via the callback function.
type Handler func(req Request, payload []byte, callback func(Response))

// Plugin represents a nylon-ring plugin.
type Plugin struct {
	name       string
	version    string
	handlers   map[string]Handler
	initFn     func() error
	shutdownFn func()

	// Internal state
	hostCtx    unsafe.Pointer
	hostVTable *C.NrHostVTable
	hostExt    *C.NrHostExt
	mu         sync.RWMutex
}

// NewPlugin creates a new plugin with the given name and version.
func NewPlugin(name, version string) *Plugin {
	return &Plugin{
		name:     name,
		version:  version,
		handlers: make(map[string]Handler),
	}
}

// OnInit sets the initialization function.
func (p *Plugin) OnInit(fn func() error) {
	p.initFn = fn
}

// OnShutdown sets the shutdown function.
func (p *Plugin) OnShutdown(fn func()) {
	p.shutdownFn = fn
}

// Handle registers a handler for the given entry name.
func (p *Plugin) Handle(entry string, handler Handler) {
	p.handlers[entry] = handler
}

// SendResult sends a result back to the host.
// This should be called from a goroutine after the handler returns.
func (p *Plugin) SendResult(sid uint64, status Status, data []byte) {
	p.mu.RLock()
	defer p.mu.RUnlock()

	if p.hostVTable == nil {
		return
	}

	// Allocate C memory for data
	// Always allocate at least 1 byte to avoid null pointer issues
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
		// Allocate 1 byte for empty data to avoid null pointer
		cDataPtr := C.malloc(1)
		if cDataPtr == nil {
			return
		}
		cData = (*C.char)(cDataPtr)
		dataLen = 0
	}

	// Call host callback
	C.call_send_result(
		p.hostVTable,
		p.hostCtx,
		C.uint64_t(sid),
		C.NrStatus(status),
		C.NrBytes{
			ptr: unsafe.Pointer(cData),
			len: dataLen,
		},
	)

	// Free the allocated memory after callback
	// Note: We always allocate memory, so always free it
	C.free(unsafe.Pointer(cData))
}

// convertCRequest converts a C request to a Go Request.
func convertCRequest(cReq *C.NrRequest) Request {
	req := Request{
		Method:  cStrToString(cReq.method),
		Path:    cStrToString(cReq.path),
		Query:   cStrToString(cReq.query),
		Headers: make(map[string]string),
		Body:    nil, // Will be set from payload
	}

	// Convert headers
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

// cStrToString converts C.NrStr to Go string.
func cStrToString(s C.NrStr) string {
	if s.ptr == nil || s.len == 0 {
		return ""
	}
	return C.GoStringN((*C.char)(s.ptr), C.int(s.len))
}

// Internal plugin instance (set during init)
var globalPlugin *Plugin
var pluginOnce sync.Once

// Static vtable - initialized in init()
var staticVTable C.NrPluginVTable

//export go_plugin_init
func go_plugin_init(pluginCtx, hostCtx unsafe.Pointer, hostVTable *C.NrHostVTable) C.NrStatus {
	pluginOnce.Do(func() {
		if globalPlugin == nil {
			return
		}

		globalPlugin.mu.Lock()
		globalPlugin.hostCtx = hostCtx
		globalPlugin.hostVTable = hostVTable
		globalPlugin.hostExt = nil // Would be retrieved via helper function
		globalPlugin.mu.Unlock()

		// Call user's init function
		if globalPlugin.initFn != nil {
			if err := globalPlugin.initFn(); err != nil {
				return
			}
		}
	})

	return C.NR_STATUS_OK
}

//export go_plugin_handle
func go_plugin_handle(pluginCtx unsafe.Pointer, entry C.NrStr, sid uint64, req *C.NrRequest, payload C.NrBytes) C.NrStatus {
	if globalPlugin == nil {
		return C.NR_STATUS_ERR
	}

	entryStr := cStrToString(entry)

	// Get handler
	globalPlugin.mu.RLock()
	handler, ok := globalPlugin.handlers[entryStr]
	globalPlugin.mu.RUnlock()

	if !ok {
		return C.NR_STATUS_INVALID
	}

	// Convert request
	goReq := convertCRequest(req)

	// Convert payload
	var payloadBytes []byte
	if payload.ptr != nil && payload.len > 0 {
		payloadBytes = C.GoBytes(payload.ptr, C.int(payload.len))
	}
	goReq.Body = payloadBytes

	// Create callback function
	callback := func(resp Response) {
		globalPlugin.SendResult(sid, resp.Status, resp.Data)
	}

	// Call handler in goroutine (non-blocking)
	// The handler can do blocking work since it's already in a goroutine
	go func() {
		defer func() {
			if r := recover(); r != nil {
				// Send error response on panic
				callback(Response{
					Status: StatusErr,
					Data:   []byte("plugin panic"),
				})
			}
		}()
		handler(goReq, payloadBytes, callback)
	}()

	return C.NR_STATUS_OK
}

//export go_plugin_shutdown
func go_plugin_shutdown(pluginCtx unsafe.Pointer) {
	if globalPlugin != nil && globalPlugin.shutdownFn != nil {
		globalPlugin.shutdownFn()
	}
}

// RegisterPlugin registers the plugin for use.
// This must be called before the plugin is loaded.
func RegisterPlugin(p *Plugin) {
	globalPlugin = p
}

// BuildPlugin builds the plugin and registers it.
// This must be called in main() before the plugin is loaded.
func BuildPlugin(p *Plugin) {
	RegisterPlugin(p)
}

//export nylon_ring_get_plugin_v1
func nylon_ring_get_plugin_v1() *C.NrPluginInfo {
	if globalPlugin == nil {
		return nil
	}

	pluginName := C.CString(globalPlugin.name)
	pluginVersion := C.CString(globalPlugin.version)

	// Allocate plugin info in C memory (required by CGO pinning rules)
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

	// Copy vtable from static (which has function pointers)
	*vtable = staticVTable

	// Fill plugin info
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

func init() {
	runtime.LockOSThread()

	// Initialize static vtable with function pointers to Go exported functions
	// Go compiler will handle the function pointer conversion
	staticVTable = C.NrPluginVTable{
		init:     (*[0]byte)(C.go_plugin_init),
		handle:   (*[0]byte)(C.go_plugin_handle),
		shutdown: (*[0]byte)(C.go_plugin_shutdown),
	}
}
