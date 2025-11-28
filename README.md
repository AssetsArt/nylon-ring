# Nylon Ring

**Nylon Ring** is an ABI-stable, non-blocking host–plugin interface designed for high-performance systems. It allows plugins written in Rust (and potentially other languages like C, C++, Zig, Go) to communicate with a host application without blocking the host's execution threads.

## Features

* **ABI-Stable**: All data structures use C ABI (`#[repr(C)]`), ensuring compatibility across language boundaries
* **Non-Blocking**: Plugins must return immediately; actual work happens in background tasks
* **Cross-Language**: Works with Rust, Go, C, Zig, and more
* **High Performance**: Designed for high-throughput, low-latency workloads
* **Dual Mode**: Supports both unary (request/response) and streaming (WebSocket-style) communication
* **Zero-Copy**: Efficient data passing using borrowed slices

## Core Design

The system relies on a few key concepts:

1. **ABI Stability**: All data structures exchanged between host and plugin are `#[repr(C)]`.
2. **Non-Blocking**: The plugin's `handle` function must return immediately. Actual work is done in the background.
3. **Callback Mechanism**: The plugin reports results back to the host via a `send_result` callback, using a request ID (`sid`).
4. **Streaming Support**: Plugins can send multiple frames for a single request, enabling WebSocket-style communication.

### Core Types (`nylon-ring` crate)

* `NrStr` / `NrBytes`: ABI-stable string and byte slices
* `NrRequest`: Request metadata (method, path, headers)
* `NrStatus`: Status codes including `Ok`, `Err`, `Invalid`, `Unsupported`, and `StreamEnd`
* `NrHostVTable`: Function pointers provided by the host (e.g., `send_result`)
* `NrPluginVTable`: Function pointers provided by the plugin (`init`, `handle`, `shutdown`)

## Project Structure

This workspace contains:

* `nylon-ring`: The core ABI library with helper functions
* `nylon-ring-host`: A Rust host adapter using `tokio` and `libloading`
* `nylon-ring-plugin-example`: An example Rust plugin demonstrating both unary and streaming modes

## Quick Start

### Building

```bash
# Build everything
make all

# Or build individually
cargo build
```

### Running Examples

```bash
# Run all examples
make examples

# Run unary example
make example-simple

# Run streaming example
make example-streaming
```

### Running Tests

```bash
# Run all tests
make test

# Run tests with verbose output
make test-all
```

## Usage

### Implementing a Plugin

Create a `cdylib` crate and implement the required VTable functions:

```rust
use nylon_ring::{NrPluginInfo, NrPluginVTable, NrStatus, NrRequest, NrBytes};
use std::ffi::c_void;

extern "C" fn plugin_handle(
    _plugin_ctx: *mut c_void,
    sid: u64,
    req: *const NrRequest,
    _payload: NrBytes,
) -> NrStatus {
    // 1. Read request (copy data if needed)
    let req_ref = unsafe { &*req };
    let path = req_ref.path.as_str().to_string();
    
    // 2. Spawn background task
    std::thread::spawn(move || {
        // 3. Do actual work (DB, network, etc.)
        std::thread::sleep(std::time::Duration::from_secs(1));
        
        // 4. Send result back
        let response = format!("OK: {}", path);
        // ... get host_vtable and host_ctx from stored state ...
        // unsafe {
        //     host_vtable.send_result(
        //         host_ctx,
        //         sid,
        //         NrStatus::Ok,
        //         NrBytes::from_slice(response.as_bytes()),
        //     );
        // }
    });
    
    // 5. Return immediately (non-blocking)
    NrStatus::Ok
}

// Export the plugin info
#[no_mangle]
pub extern "C" fn nylon_ring_get_plugin_v1() -> *const NrPluginInfo {
    &PLUGIN_INFO
}
```

### Loading a Plugin (Host)

Use `nylon-ring-host` to load and call the plugin:

**Unary Call:**
```rust
use nylon_ring_host::{NylonRingHost, HighLevelRequest};

let host = NylonRingHost::load("path/to/plugin.so")?;

let req = HighLevelRequest {
    method: "GET".to_string(),
    path: "/api/data".to_string(),
    query: "".to_string(),
    headers: vec![("User-Agent".to_string(), "MyApp/1.0".to_string())],
    body: vec![],
};

// Async call - does not block the thread
let (status, payload) = host.call(req).await?;
println!("Status: {:?}, Response: {}", status, String::from_utf8_lossy(&payload));
```

**Streaming Call:**
```rust
use nylon_ring::{NrStatus};
use nylon_ring_host::{NylonRingHost, HighLevelRequest};

let host = NylonRingHost::load("path/to/plugin.so")?;

let req = HighLevelRequest {
    method: "GET".to_string(),
    path: "/stream".to_string(),
    query: "".to_string(),
    headers: vec![],
    body: vec![],
};

// Get stream receiver
let mut stream = host.call_stream(req).await?;

// Receive frames
while let Some(frame) = stream.recv().await {
    println!("Frame - Status: {:?}, Data: {}", 
        frame.status, 
        String::from_utf8_lossy(&frame.data)
    );
    
    // Stream ends when we receive StreamEnd, Err, Invalid, or Unsupported
    if matches!(
        frame.status,
        NrStatus::StreamEnd | NrStatus::Err | NrStatus::Invalid | NrStatus::Unsupported
    ) {
        break;
    }
}
```

## Architecture

### Unary Flow

```
Host                    Plugin
  |                       |
  |-- handle(sid, req) -->|
  |<-- return Ok -------- |
  |                       | [spawn background task]
  |                       | [do work...]
  |                       |
  |<-- send_result(sid) --|
  |                       |
```

### Streaming Flow

```
Host                    Plugin
  |                       |
  |-- handle(sid, req) -->|
  |<-- return Ok -------- |
  |                       | [spawn background task]
  |                       |
  |<-- send_result(sid) --| [frame 1]
  |<-- send_result(sid) --| [frame 2]
  |<-- send_result(sid) --| [frame 3]
  |<-- send_result(sid) --| [StreamEnd]
  |                       |
```

## Key Constraints

* **Plugin `handle()` must return immediately** - no blocking operations
* **All ABI types are `#[repr(C)]`** - do not modify their layout
* **Host owns request data** - plugin must copy if needed
* **Thread-safe callbacks** - `send_result` can be called from any thread

## Platform Support

* Linux (`.so`)
* macOS (`.dylib`)
* Windows (`.dll`)
