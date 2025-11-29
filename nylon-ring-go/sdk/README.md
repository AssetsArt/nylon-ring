# Nylon Ring Go SDK

High-level Go SDK for creating nylon-ring plugins easily, similar to Rust's `define_plugin!` macro.

## Features

- **Simple API**: Create plugins with just a few lines of code
- **Type-safe**: Go types instead of raw C types
- **Non-blocking**: SDK handles goroutines automatically
- **Entry-based routing**: Register multiple handlers easily
- **Panic-safe**: Automatic panic recovery

## Quick Start

```go
package main

import (
	"time"
	"github.com/AssetsArt/nylon-ring/nylon-ring-go/sdk"
)

func main() {
	// Create plugin
	plugin := sdk.NewPlugin("my-plugin", "1.0.0")
	
	// Register unary handler
	plugin.Handle("unary", func(req sdk.Request, payload []byte, callback func(sdk.Response)) {
		// SDK automatically calls this in a goroutine, so you can do blocking work
		time.Sleep(2 * time.Second)
		
		callback(sdk.Response{
			Status: sdk.StatusOk,
			Data:   []byte("OK: " + req.Method + " " + req.Path),
		})
	})
	
	// Register streaming handler
	plugin.Handle("stream", func(req sdk.Request, payload []byte, callback func(sdk.Response)) {
		for i := 1; i <= 5; i++ {
			time.Sleep(1 * time.Second)
			callback(sdk.Response{
				Status: sdk.StatusOk,
				Data:   []byte("Frame " + string(rune('0'+i)) + "/5"),
			})
		}
		callback(sdk.Response{
			Status: sdk.StatusStreamEnd,
			Data:   nil,
		})
	})
	
	// Build plugin
	sdk.BuildPlugin(plugin)
}
```

## API Reference

### Plugin

#### `NewPlugin(name, version string) *Plugin`

Creates a new plugin with the given name and version.

#### `Plugin.OnInit(fn func() error)`

Sets the initialization function. Called when the plugin is loaded.

#### `Plugin.OnShutdown(fn func())`

Sets the shutdown function. Called when the plugin is unloaded.

#### `Plugin.Handle(entry string, handler Handler)`

Registers a handler for the given entry name. The handler will be called when the host calls `host.call(entry, req)` or `host.call_stream(entry, req)`.

#### `Plugin.SendResult(sid uint64, status Status, data []byte)`

Sends a result back to the host. Usually called from within a handler via the callback function.

### Types

#### `Request`

High-level request type with Go types:

```go
type Request struct {
	Method  string
	Path    string
	Query   string
	Headers map[string]string
	Body    []byte
}
```

#### `Response`

Response type:

```go
type Response struct {
	Status Status
	Data   []byte
}
```

#### `Status`

Status codes:

- `StatusOk` - Success
- `StatusErr` - General error
- `StatusInvalid` - Invalid request/state
- `StatusUnsupported` - Unsupported operation
- `StatusStreamEnd` - Stream completed normally (streaming only)

#### `Handler`

Handler function signature:

```go
type Handler func(req Request, payload []byte, callback func(Response))
```

The SDK automatically calls handlers in goroutines, so you can do blocking work (DB calls, network requests, etc.) without worrying about blocking the host.

## Building

```bash
# In your plugin directory
go build -buildmode=c-shared -o my_plugin.so .
```

## Comparison with Rust

### Rust (using `define_plugin!` macro):

```rust
define_plugin! {
    init: plugin_init,
    shutdown: plugin_shutdown,
    entries: {
        "unary" => handle_unary,
        "stream" => handle_stream,
    }
}

unsafe fn handle_unary(
    _plugin_ctx: *mut c_void,
    sid: u64,
    req: *const NrRequest,
    payload: NrBytes,
) -> NrStatus {
    // Copy data, spawn thread, etc.
}
```

### Go (using SDK):

```go
plugin := sdk.NewPlugin("my-plugin", "1.0.0")
plugin.Handle("unary", func(req sdk.Request, payload []byte, callback func(sdk.Response)) {
    // SDK handles goroutines, type conversion, etc.
    callback(sdk.Response{Status: sdk.StatusOk, Data: []byte("OK")})
})
sdk.BuildPlugin(plugin)
```

The Go SDK provides the same simplicity as Rust's `define_plugin!` macro!

