package sdk

import (
	"sync"
	"unsafe"
)

// Status represents the status codes for the Nylon Ring ABI.
type Status uint32

const (
	StatusOk          Status = 0
	StatusErr         Status = 1
	StatusInvalid     Status = 2
	StatusUnsupported Status = 3
	StatusStreamEnd   Status = 4
)

// Request represents a high-level request with Go types.
type Request struct {
	Method  string
	Path    string
	Query   string
	Headers map[string]string
	Body    []byte
}

var requestPool = sync.Pool{
	New: func() interface{} {
		return &Request{
			Headers: make(map[string]string),
		}
	},
}

func acquireRequest() *Request {
	return requestPool.Get().(*Request)
}

func releaseRequest(req *Request) {
	// Clear map
	for k := range req.Headers {
		delete(req.Headers, k)
	}
	req.Method = ""
	req.Path = ""
	req.Query = ""
	req.Body = nil
	requestPool.Put(req)
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

// RawHandler is a function that handles a raw request (no metadata).
type RawHandler func(payload []byte, callback func(Response))

// StreamHandler is a function that handles stream data.
type StreamHandler func(data []byte, callback func(Response))

// StreamCloseHandler is a function that handles stream close.
type StreamCloseHandler func(callback func(Response))

type streamHandlers struct {
	data  StreamHandler
	close StreamCloseHandler
}

// Plugin represents a nylon-ring plugin.
type Plugin struct {
	name           string
	version        string
	handlers       map[string]Handler
	syncHandlers   map[string]Handler
	rawHandlers    map[string]RawHandler
	streamHandlers *streamHandlers
	initFn         func() error
	shutdownFn     func()

	// Internal state
	hostCtx    unsafe.Pointer
	hostVTable unsafe.Pointer // *C.NrHostVTable
	hostExt    unsafe.Pointer // *C.NrHostExt
	mu         sync.RWMutex
}

// NewPlugin creates a new plugin with the given name and version.
func NewPlugin(name, version string) *Plugin {
	return &Plugin{
		name:         name,
		version:      version,
		handlers:     make(map[string]Handler),
		syncHandlers: make(map[string]Handler),
		rawHandlers:  make(map[string]RawHandler),
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
// The handler will be executed in a goroutine.
func (p *Plugin) Handle(entry string, handler Handler) {
	p.handlers[entry] = handler
}

// HandleSync registers a synchronous handler for the given entry name.
// The handler will be executed in the calling thread (blocking the host).
// Use this for very fast handlers to avoid goroutine overhead.
func (p *Plugin) HandleSync(entry string, handler Handler) {
	p.syncHandlers[entry] = handler
}

// HandleRaw registers a raw handler for the given entry name.
// The handler will be executed in a goroutine.
func (p *Plugin) HandleRaw(entry string, handler RawHandler) {
	p.rawHandlers[entry] = handler
}

// HandleRawStream registers a raw streaming handler for the given entry name.
// The handler will be executed in a goroutine and can call the callback multiple times.
func (p *Plugin) HandleRawStream(entry string, handler RawHandler) {
	p.rawHandlers[entry] = handler
}

// HandleStream registers handlers for bidirectional streaming.
func (p *Plugin) HandleStream(dataHandler StreamHandler, closeHandler StreamCloseHandler) {
	p.streamHandlers = &streamHandlers{
		data:  dataHandler,
		close: closeHandler,
	}
}

// SendResult sends a result back to the host.
// This should be called from a goroutine after the handler returns.
func (p *Plugin) SendResult(sid uint64, status Status, data []byte) {
	// Delegate to c_bindings.go
	sendResultToHost(sid, status, data)
}

// Internal methods called by c_bindings.go

func (p *Plugin) setHostContext(ctx, vtable unsafe.Pointer) {
	p.mu.Lock()
	defer p.mu.Unlock()
	p.hostCtx = ctx
	p.hostVTable = vtable
	p.hostExt = nil
}

func (p *Plugin) getHostContext() unsafe.Pointer {
	p.mu.RLock()
	defer p.mu.RUnlock()
	return p.hostCtx
}

func (p *Plugin) getHostVTable() unsafe.Pointer {
	p.mu.RLock()
	defer p.mu.RUnlock()
	return p.hostVTable
}

func (p *Plugin) callInit() error {
	if p.initFn != nil {
		return p.initFn()
	}
	return nil
}

func (p *Plugin) callShutdown() {
	if p.shutdownFn != nil {
		p.shutdownFn()
	}
}

func (p *Plugin) getInfo() (string, string) {
	return p.name, p.version
}

func (p *Plugin) handleRequest(entry string, req *Request, payload []byte, callback func(Status, []byte)) error {
	p.mu.RLock()
	handler, ok := p.handlers[entry]
	syncHandler, syncOk := p.syncHandlers[entry]
	p.mu.RUnlock()

	if syncOk {
		// Execute synchronously
		defer releaseRequest(req)

		defer func() {
			if r := recover(); r != nil {
				callback(StatusErr, []byte("plugin panic"))
			}
		}()

		syncHandler(*req, payload, func(resp Response) {
			callback(resp.Status, resp.Data)
		})
		return nil
	}

	if !ok {
		// If handler not found, we must release request here because we won't spawn goroutine
		releaseRequest(req)
		return &PluginError{msg: "handler not found"}
	}

	// Call handler in goroutine
	go func() {
		defer releaseRequest(req) // Release request when done

		defer func() {
			if r := recover(); r != nil {
				// Send error response on panic
				callback(StatusErr, []byte("plugin panic"))
			}
		}()

		// Pass by value to handler as per API
		handler(*req, payload, func(resp Response) {
			callback(resp.Status, resp.Data)
		})
	}()

	return nil
}

func (p *Plugin) handleRawRequest(entry string, payload []byte, callback func(Status, []byte)) error {
	p.mu.RLock()
	handler, ok := p.rawHandlers[entry]
	p.mu.RUnlock()

	if !ok {
		return &PluginError{msg: "handler not found"}
	}

	defer func() {
		if r := recover(); r != nil {
			callback(StatusErr, []byte("plugin panic"))
		}
	}()

	handler(payload, func(resp Response) {
		callback(resp.Status, resp.Data)
	})

	return nil
}

func (p *Plugin) handleStreamData(sid uint64, data []byte, callback func(Status, []byte)) error {
	p.mu.RLock()
	handlers := p.streamHandlers
	p.mu.RUnlock()

	if handlers == nil || handlers.data == nil {
		return &PluginError{msg: "stream data handler not found"}
	}

	defer func() {
		if r := recover(); r != nil {
			callback(StatusErr, []byte("plugin panic"))
		}
	}()

	handlers.data(data, func(resp Response) {
		callback(resp.Status, resp.Data)
	})

	return nil
}

func (p *Plugin) handleStreamClose(sid uint64, callback func(Status, []byte)) error {
	p.mu.RLock()
	handlers := p.streamHandlers
	p.mu.RUnlock()

	if handlers == nil || handlers.close == nil {
		return &PluginError{msg: "stream close handler not found"}
	}

	defer func() {
		if r := recover(); r != nil {
			callback(StatusErr, []byte("plugin panic"))
		}
	}()

	handlers.close(func(resp Response) {
		callback(resp.Status, resp.Data)
	})

	return nil
}

type PluginError struct {
	msg string
}

func (e *PluginError) Error() string {
	return e.msg
}

// Internal plugin instance (set during init)
var globalPlugin *Plugin

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
