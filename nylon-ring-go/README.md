# Nylon Ring Go

Go implementation of nylon-ring plugins with a high-level SDK that makes plugin creation as easy as Rust's `define_plugin!` macro.

## Two Ways to Create Plugins

### 1. Using the SDK (Recommended)

The SDK provides a high-level API similar to Rust's `define_plugin!` macro:

```go
package main

import "github.com/nylon-ring/nylon-ring-go/sdk"

func main() {
	plugin := sdk.NewPlugin("my-plugin", "1.0.0")
	
	plugin.Handle("unary", func(req sdk.Request, payload []byte, callback func(sdk.Response)) {
		// SDK handles goroutines automatically - you can do blocking work
		callback(sdk.Response{Status: sdk.StatusOk, Data: []byte("OK")})
	})
	
	sdk.BuildPlugin(plugin)
}
```

See [`sdk/README.md`](sdk/README.md) for full documentation.

### 2. Low-Level CGO (Advanced)

For full control, you can use CGO directly. See `plugin-example/` for an example.

## Overview

This directory contains a Go plugin implementation for nylon-ring that demonstrates:

- **Entry-based routing**: Supports multiple entry points ("unary", "stream")
- **Non-blocking handlers**: All work is done in goroutines
- **C ABI compatibility**: Uses CGO to match the exact C struct layouts from Rust
- **State management**: Can use host extension for per-request/stream state

## Structure

```
nylon-ring-go/
├── sdk/                  # High-level SDK (recommended)
│   ├── plugin.go         # Plugin builder API
│   ├── go.mod
│   └── README.md         # SDK documentation
├── plugin-example-simple/ # Simple example using SDK
│   ├── main.go
│   ├── go.mod
│   └── build.sh
└── plugin-example/        # Low-level CGO example (advanced)
    ├── main.go
    ├── go.mod
    └── build.sh
```

## Building

### Prerequisites

- Go 1.21 or later
- C compiler (gcc or clang)
- CGO enabled (set `CGO_ENABLED=1`)

### Build with SDK (Recommended)

```bash
cd plugin-example-simple
./build.sh
```

### Build Low-Level Example

```bash
cd plugin-example
./build.sh
```

### Manual Build

```bash
# With SDK
cd plugin-example-simple
go build -buildmode=c-shared -o my_plugin.so .

# Low-level
cd plugin-example
go build -buildmode=c-shared -o my_plugin.so .
```

## Usage

The plugin can be loaded by the Rust host just like any other plugin:

```rust
use nylon_ring_host::{NylonRingHost, HighLevelRequest, Extensions};

let host = NylonRingHost::load("path/to/nylon_ring_go_plugin.so")?;

// Unary call
let req = HighLevelRequest {
    method: "GET".to_string(),
    path: "/hello".to_string(),
    query: "".to_string(),
    headers: vec![],
    body: vec![],
    extensions: Extensions::new(),
};

let (status, payload) = host.call("unary", req).await?;

// Streaming call
let mut stream = host.call_stream("stream", req).await?;
while let Some(frame) = stream.recv().await {
    // Process frames
}
```

## Implementation Details

### C ABI Compatibility

The Go plugin uses CGO to match the exact C struct layouts from Rust:

- `NrStr`: 16 bytes (ptr: 8 bytes, len: 4 bytes, padding: 4 bytes)
- `NrBytes`: 16 bytes (ptr: 8 bytes, len: 8 bytes)
- `NrRequest`: 72 bytes (matches Rust layout exactly)

### Non-Blocking Requirement

All handlers must return immediately. Background work is done in goroutines:

```go
//export plugin_handle
func plugin_handle(pluginCtx unsafe.Pointer, entry C.NrStr, sid uint64, req *C.NrRequest, payload C.NrBytes) C.NrStatus {
    // Copy data immediately
    path := nrStrToString(req.path)
    
    // Spawn goroutine for background work
    go func() {
        // Do actual work here
        time.Sleep(2 * time.Second)
        
        // Call host callback
        hostHandle.vtable.send_result(...)
    }()
    
    return C.NR_STATUS_OK
}
```

### Entry-Based Routing

The plugin supports multiple entry points:

- `"unary"`: Single request/response
- `"stream"`: Streaming/WebSocket-style (multiple frames)

The host routes requests based on the entry name:

```rust
host.call("unary", req)      // Routes to handle_unary
host.call_stream("stream", req) // Routes to handle_stream
```

### Memory Management

- Request data is copied immediately (host owns the original)
- Response data is allocated with `C.malloc` and freed after callback
- The host will copy the data, so the plugin can free it after the callback

## Limitations

1. **No host extension helper**: The Go plugin doesn't have access to `NylonRingHost::get_host_ext()` directly. State management would need to be implemented differently or the host would need to provide a C function.

2. **Static initialization**: The plugin info and vtable are initialized as package-level variables. This works but is less flexible than Rust's approach.

3. **Error handling**: Go's error handling doesn't map directly to C status codes. The plugin uses `C.NrStatus` values directly.

## Testing

To test the plugin with the Rust host:

```bash
# Build the Go plugin
cd nylon-ring-go/plugin-example
./build.sh

# Build the Rust host example
cd ../../nylon-ring-host
cargo build --example simple_host

# Run the example (modify the plugin path in the example)
cargo run --example simple_host
```

## Notes

- The plugin must be built as a C shared library (`-buildmode=c-shared`)
- Function names must be exported with `//export` comments
- The entry point `nylon_ring_get_plugin_v1` must be exported
- All C types must match the Rust `#[repr(C)]` structs exactly

