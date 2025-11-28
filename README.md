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

## Performance

We benchmark both the ABI types and the full host–plugin round trip to ensure minimal overhead.

> **Note**: All performance numbers below are measured on **Apple M1 Pro (10-core)** with release builds.

### ABI Types (`nylon-ring`)

The ABI layer itself is extremely lightweight:

* `NrStr::from_str` ≈ **0.99 ns** (M1 Pro 10-core)
* `NrStr::as_str` ≈ **1.00 ns** (M1 Pro 10-core)
* `NrBytes::from_slice` ≈ **0.52 ns** (M1 Pro 10-core)
* `NrBytes::as_slice` ≈ **0.84 ns** (M1 Pro 10-core)
* `NrHeader::new` ≈ **1.91 ns** (M1 Pro 10-core)
* `NrRequest::build` ≈ **2.83 ns** (M1 Pro 10-core)

**Conclusion**: Creating ABI views is essentially free (0.5–3 ns) compared to real-world network or I/O costs. The bottleneck will never be in the ABI struct layer.

### Host Overhead (`nylon-ring-host`)

Full round-trip performance (host → plugin → host callback):

* **Unary call**: ~14.8 µs per call → **~67k calls/sec** on a single core (M1 Pro 10-core)
* **Unary call with 1KB body**: ~14.9 µs per call → **~67k calls/sec** (M1 Pro 10-core, body size has negligible impact)
* **Streaming call** (consume all frames): ~16.0 µs per call → **~62k calls/sec** (M1 Pro 10-core)
* **Build `HighLevelRequest`**: ~216 ns (M1 Pro 10-core)

The overhead is dominated by:
* FFI crossing (`extern "C"` calls)
* Async scheduling (Tokio runtime)
* Locking the pending-request map (`Mutex<HashMap>`)
* Plugin's own work

**Scaling**: With multiple cores handling requests, ideal throughput scales linearly. On M1 Pro 10-core, theoretical maximum can reach **~670k req/s** in a scale-out scenario, which is well within the range of high-performance reverse proxy systems.

### Benchmarking

Run benchmarks with:

```bash
make benchmark              # All benchmarks
make benchmark-abi         # ABI type benchmarks only
make benchmark-host        # Host overhead benchmarks (requires plugin)
```

> **Note**: Benchmark results are hardware-dependent. The numbers above are from **Apple M1 Pro (10-core)**. Your results may vary based on CPU architecture, clock speed, and system load.

## State Management

nylon-ring supports **per-request and per-stream state** without changing the ABI layout.

### Per-SID State

Host maintains state per request/stream:

```rust
state_per_sid: Mutex<HashMap<u64, HashMap<String, Vec<u8>>>>
```

### Host Extension API

Plugins can access state through the `NrHostExt` extension:

```rust
#[repr(C)]
pub struct NrHostExt {
    pub set_state: unsafe extern "C" fn(host_ctx, sid, key: NrStr, value: NrBytes) -> NrBytes,
    pub get_state: unsafe extern "C" fn(host_ctx, sid, key: NrStr) -> NrBytes,
}
```

### Using State in Plugins

```rust
// Set state
host_ext.set_state(host_ctx, sid, NrStr::from_str("key"), NrBytes::from_slice(value));

// Get state
let value = host_ext.get_state(host_ctx, sid, NrStr::from_str("key"));
```

### State Lifecycle

* Created automatically on first `set_state()` call
* Persists for the lifetime of the request/stream
* Automatically cleared when:
  * Unary call completes
  * Streaming call ends (via `StreamEnd` or error status)

This enables:
* WebSocket session management
* Per-request metadata storage
* Plugin-local agent state
* Frame-to-frame data persistence

## Key Constraints

* **Plugin `handle()` must return immediately** - no blocking operations
* **All ABI types are `#[repr(C)]`** - do not modify their layout
* **Host owns request data** - plugin must copy if needed
* **Thread-safe callbacks** - `send_result` can be called from any thread
* **Panic-safe FFI** - all `extern "C"` functions catch panics
* **No `unwrap()` in production** - proper error handling required

## Error Handling

The host adapter uses `NylonRingHostError` (defined with `thiserror`):

* All functions return `Result<T, NylonRingHostError>`
* Clear, descriptive error messages
* No `anyhow` dependency
* Panic-safe callbacks

## Rust Coding Rules

The nylon-ring ecosystem follows strict Rust coding rules for production safety:

1. **No `unwrap()` or `expect()`** in production code (only in tests/benchmarks)
2. **No `anyhow`** - use `thiserror` for error types
3. **All fallible functions return `Result`** - no panic as control flow
4. **Panic-safe `extern "C"` functions** - all FFI boundaries catch panics
5. **Error consolidation** - single error enum per crate with `thiserror::Error`
6. **Clear error messages** - descriptive error variants
7. **Avoid `panic!` and `assert!`** - only in benchmarks/tests

See `nylon-ring-host/src/error.rs` for an example error type implementation.

## Platform Support

* Linux (`.so`)
* macOS (`.dylib`)
* Windows (`.dll`)
