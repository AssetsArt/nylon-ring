# Nylon Ring: ABI-Stable, Non-Blocking Host–Plugin Interface

## Introduction

`nylon-ring` is a **host–plugin interface** standard designed for:

* **ABI-stable** (uses C ABI → works with Rust, Go, C, Zig)
* **Flexible** (supports both blocking and non-blocking plugins)
* **Cross-language** (connects Rust host ↔ Rust/Go plugin seamlessly)
* **Zero-serialization enforcement** (payload is bytes; you choose JSON, rkyv, FlatBuffers, Cap'nProto)
* **Safe for high-QPS workloads** (designed for Nylon/Pingora)

This document explains everything an Agent needs to know to:

* Create plugins
* Create host adapters
* Design non-blocking integrations
* Send/receive data with sid-based async callbacks

---

## 1. Overview

### 1.1 What is nylon-ring?

A **middle ring** connecting:

```
Host (Nylon/Pingora) <--ABI--> Plugin (Rust/Go/…)
```

Uses C ABI:

* All structs are `#[repr(C)]`
* All functions are `extern "C"`

Goal: Make plugins "external modules" that can:

* Read request metadata
* Read request metadata
* Work async/background (optional)
* Send results via callback
* Can block if needed (but discouraged for high-throughput)

---

## 2. Architecture Summary

### 2.1 High-Level Flow

**Unary Call (Request/Response):**
```
[Host]
  1) Build NrRequest + NrBytes
  2) sid = next_id()
  3) vtable.handle(plugin_ctx, entry, sid, req, payload)
  4) Wait sid via tokio::oneshot (async)
  
[Plugin]
  A) handle(entry, sid, req, payload) → MUST return immediately
  B) spawn background task
  C) run heavy logic (DB, network…)
  D) call host_vtable.send_result(host_ctx, sid, status, bytes)
```

**Streaming Call (WebSocket-style):**
```
[Host]
  1) Build NrRequest + NrBytes
  2) sid = next_id()
  3) vtable.handle(plugin_ctx, entry, sid, req, payload)
  4) Return StreamReceiver immediately
  
[Plugin]
  A) handle(entry, sid, req, payload) → MUST return immediately
  B) spawn background task
  C) loop: send multiple frames via send_result(host_ctx, sid, Ok, frame_bytes)
  D) call send_result(host_ctx, sid, StreamEnd, empty) to close
```

**Note**: The `entry` parameter allows plugins to support multiple entry points. The host calls `host.call("entry_name", req)` or `host.call_stream("entry_name", req)` to route to specific handlers.

### Blocking vs Non-blocking:
* Host never blocks worker thread by default.
* Plugin CAN block if it needs to, but for high performance, it should spawn background tasks.
* Go SDK provides `HandleSync` for blocking handlers and `Handle` for non-blocking (goroutine) handlers.

---

## 3. ABI Specification (nylon-ring)

### 3.1 Status Codes

```rust
#[repr(u32)]
pub enum NrStatus {
    Ok = 0,
    Err = 1,
    Invalid = 2,
    Unsupported = 3,
    StreamEnd = 4,  // For streaming: indicates stream completion
}
```

### 3.2 String (UTF-8)

```rust
#[repr(C)]
pub struct NrStr {
    pub ptr: *const u8,
    pub len: u32,
}
```

* Borrowed only
* Must remain valid during the call
* Host owns the actual storage

### 3.3 Bytes

```rust
#[repr(C)]
pub struct NrBytes {
    pub ptr: *const u8,
    pub len: u64,
}
```

* Borrowed bytes
* Used for request body or serialized payload

### 3.4 Header Pair

```rust
#[repr(C)]
pub struct NrHeader {
    pub key: NrStr,
    pub value: NrStr,
}
```

### 3.5 Request Structure

```rust
#[repr(C)]
pub struct NrRequest {
    pub path: NrStr,
    pub method: NrStr,
    pub query: NrStr,

    pub headers: *const NrHeader,
    pub headers_len: u32,

    // ABI forward-compatibility storage
    pub _reserved0: u32,
    pub _reserved1: u64,
}
```

Note:

* Slightly HTTP-centric but can represent generic request metadata
* Reserved fields allow adding future fields without breaking ABI

---

## 4. Host Callback Table (Non-Blocking)

Host must expose a vtable:

```rust
#[repr(C)]
pub struct NrHostVTable {
    pub send_result: unsafe extern "C" fn(
        host_ctx: *mut c_void,
        sid: u64,
        status: NrStatus,
        payload: NrBytes,
    ),
}
```

Rules:

* Plugin may call from any thread.
* Host must map `sid → future/oneshot` (unary) or `sid → mpsc::UnboundedSender` (streaming).
* Host must wake waiting future on callback.
* For streaming: `send_result` can be called multiple times per `sid`.

---

## 5. Plugin VTable

Plugin exports functions via:

```rust
#[repr(C)]
pub struct NrPluginVTable {
    pub init: Option<
        unsafe extern "C" fn(plugin_ctx, host_ctx, host_vtable) -> NrStatus
    >,

    pub handle: Option<
        unsafe extern "C" fn(plugin_ctx, entry: NrStr, sid, req, payload) -> NrStatus
    >,

    pub shutdown: Option<
        unsafe extern "C" fn(plugin_ctx)
    >,
}
```

**Note**: The `handle` function takes an `entry: NrStr` parameter for entry-based routing. Plugins can support multiple entry points (e.g., "unary", "stream", "state").

### Contract (CRITICAL):

### **`handle()`**
Plugin must:
1. Copy all required data out of `req` & `payload`.
2. Do work (sync or async).
3. Call `host_vtable.send_result(...)`.
   - For unary: call once with final status.
   - For streaming: call multiple times with `Ok`, then once with `StreamEnd`.

### Plugin Creation with `define_plugin!` Macro

The `nylon-ring` crate provides a `define_plugin!` macro that simplifies plugin creation:

```rust
use nylon_ring::define_plugin;

unsafe fn plugin_init(
    _plugin_ctx: *mut c_void,
    host_ctx: *mut c_void,
    host_vtable: *const NrHostVTable,
) -> NrStatus {
    // Initialize plugin state
    NrStatus::Ok
}

unsafe fn handle_unary(
    _plugin_ctx: *mut c_void,
    sid: u64,
    req: *const NrRequest,
    payload: NrBytes,
) -> NrStatus {
    // Non-blocking handler for "unary" entry
    NrStatus::Ok
}

unsafe fn plugin_shutdown(_plugin_ctx: *mut c_void) {
    // Cleanup
}

define_plugin! {
    init: plugin_init,
    shutdown: plugin_shutdown,
    entries: {
        "unary" => handle_unary,
        "stream" => handle_stream,
        "state" => handle_state,
    }
}
```

The macro automatically:
- Creates the `NrPluginVTable` with panic-safe wrappers
- Exports `nylon_ring_get_plugin_v1()` entry point
- Routes requests to the correct handler based on entry name
- Handles panics safely across FFI boundaries

---

## 6. Plugin Info

Plugins export:

```rust
#[repr(C)]
pub struct NrPluginInfo {
    pub abi_version: u32,
    pub struct_size: u32,

    pub name: NrStr,
    pub version: NrStr,

    pub plugin_ctx: *mut c_void,
    pub vtable: *const NrPluginVTable,
}
```

### Required exported symbol:

```rust
extern "C" fn nylon_ring_get_plugin_v1() -> *const NrPluginInfo;
```

Host loads via `libloading` or C's `dlopen`.

---

## 7. Host Responsibilities

Host must:

### 7.1 Validate ABI

```rust
plugin_info.compatible(expected_abi_version)
```

### 7.2 Manage `sid` lifecycle

* Generate unique `sid`
* For unary: Insert into `DashMap<sid, oneshot::Sender<(NrStatus, Vec<u8>)>>`
* For streaming: Insert into `DashMap<sid, mpsc::UnboundedSender<StreamFrame>>`
* Erase when callback returns (unary) or stream ends (streaming)

**Note**: The host uses `DashMap` (not `Mutex<HashMap>`) for better concurrency performance with fine-grained locking.

### 7.3 Build request & payload

Host owns underlying storage.

### 7.4 Maintain `host_ctx`

Pointer to any structure host uses (e.g., `Arc<HostContext>` with `DashMap`)

### 7.5 Support both unary and streaming

* `call(entry, req)` → unary RPC (single response)
* `call_stream(entry, req)` → streaming RPC (multiple frames)

The `entry` parameter is a string that routes to the correct handler in the plugin.

### 7.6 High-Level Request API

The host provides `HighLevelRequest` for convenience:

```rust
pub struct HighLevelRequest {
    pub method: String,
    pub path: String,
    pub query: String,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
    pub extensions: Extensions,  // Type-safe metadata storage
}
```

The `extensions` field uses a type-safe `Extensions` struct (similar to `http::Extensions`) that allows storing arbitrary types without serialization.

---

## 8. Plugin Responsibilities

Plugin must:

### 8.1 Implement `nylon_ring_get_plugin_v1()`

This is automatically handled by the `define_plugin!` macro.

### 8.2 Implement fast-returning `handle()`

The `handle()` function receives an `entry: NrStr` parameter that identifies which handler to use. Plugins can support multiple entry points (e.g., "unary", "stream", "state").

Must NOT:

* block thread
* do sleep in handle
* do DB call inside handle
* await anything

### 8.3 Entry-Based Routing

Plugins use entry-based routing to support multiple handlers:

```rust
define_plugin! {
    init: plugin_init,
    shutdown: plugin_shutdown,
    entries: {
        "unary" => handle_unary,      // Handles unary RPC calls
        "stream" => handle_stream,    // Handles streaming calls
        "state" => handle_state,      // Handles state management demo
    }
}
```

The host calls `host.call("unary", req)` or `host.call_stream("stream", req)` to route to specific handlers. If an entry doesn't exist, the plugin returns `NrStatus::Invalid`.

### 8.4 Background task callback

**Unary:**
```rust
host_vtable.send_result(host_ctx, sid, NrStatus::Ok, result_bytes)
```

**Streaming:**
```rust
// Send multiple frames
host_vtable.send_result(host_ctx, sid, NrStatus::Ok, frame1_bytes);
host_vtable.send_result(host_ctx, sid, NrStatus::Ok, frame2_bytes);
// ...
// Close stream
host_vtable.send_result(host_ctx, sid, NrStatus::StreamEnd, empty_bytes);
```

---

## 9. Concurrency Model

### Host

* Multi-threaded
* Async (Tokio)
* Never blocked by plugin
* Uses `DashMap` for concurrent access to pending requests and state

### Plugin

* Free to create own thread pools, tokio runtimes, rayon, etc.
* Can be written in Rust, Go, Zig, C

### Synchronization

Plugin should log from plugin, not host.

---

## 10. Error Model

* `NrStatus::Ok` - Success
* `NrStatus::Err` - General error
* `NrStatus::Invalid` - Invalid request/state
* `NrStatus::Unsupported` - Unsupported operation
* `NrStatus::StreamEnd` - Stream completed normally (streaming only)

Plugins must return **synchronous errors** only if:

* request struct is invalid
* plugin_ctx/host_ctx is null
* ABI contract violated

Runtime errors always delivered by callback.

---

## 11. Payload Strategy

You may use any serialization:

* JSON
* FlatBuffers
* Cap'n Proto
* rkyv
* MessagePack
* Protobuf

nylon-ring does not enforce any format.

---

## 12. Multi-Language Design

nylon-ring supports:

### Rust Plugin

* easiest, zero overhead
* full ABI control

### Go Plugin

nylon-ring provides a **high-level Go SDK** that makes plugin creation as easy as Rust's `define_plugin!` macro:

**Using SDK (Recommended):**
```go
package main

import "github.com/AssetsArt/nylon-ring/nylon-ring-go/sdk"

func init() {
	plugin := sdk.NewPlugin("my-plugin", "1.0.0")
	
	plugin.Handle("unary", func(req sdk.Request, payload []byte, callback func(sdk.Response)) {
		// SDK automatically calls this in a goroutine - you can do blocking work
		callback(sdk.Response{Status: sdk.StatusOk, Data: []byte("OK")})
	})

    // Use HandleSync for very fast, non-blocking operations (runs on host thread)
    plugin.HandleSync("fast", func(req sdk.Request, payload []byte, callback func(sdk.Response)) {
        callback(sdk.Response{Status: sdk.StatusOk, Data: []byte("FAST")})
    })
	
	sdk.BuildPlugin(plugin)
}
```

**Low-Level CGO (Advanced):**
* Using `cgo` directly
* Plugin exports `extern "C"` functions
* Uses same ABI structs
* See `nylon-ring-go/plugin-example/` for full example

### Zig Plugin

* perfect C ABI support

### C / C++ Plugin

* direct match

---

## 13. Performance & Benchmarks

### ABI Types Performance

The ABI layer itself is extremely lightweight (measured on **Apple M1 Pro 10-core**):

* `NrStr::from_str` ≈ **0.99 ns**
* `NrStr::as_str` ≈ **1.00 ns**
* `NrBytes::from_slice` ≈ **0.52 ns**
* `NrBytes::as_slice` ≈ **0.84 ns**
* `NrHeader::new` ≈ **1.91 ns**
* `NrRequest::build` ≈ **2.83 ns**

**Conclusion**: Creating ABI views is essentially free (0.5–3 ns). The bottleneck will never be in the ABI struct layer.

### Host Overhead

Full round-trip performance (host → plugin → host callback, measured on **Apple M1 Pro 10-core**):

* **Unary call**: ~0.57 µs per call → **~1.76M calls/sec** on a single core
* **Unary call with 1KB body**: ~0.60 µs per call → **~1.68M calls/sec** (body size has negligible impact)
* **Streaming call** (consume all frames): ~1.36 µs per call → **~736k calls/sec**
* **Build `HighLevelRequest`**: ~216 ns

The overhead is dominated by:
* FFI crossing (`extern "C"` calls)
* Async scheduling (Tokio runtime)
* Concurrent map operations (`DashMap` - fine-grained locking)
* Plugin's own work

**Scaling**: With multiple cores handling requests, ideal throughput scales linearly. On M1 Pro 10-core, measured throughput reaches **~10M req/s** in a stress test scenario.

### Benchmark Expectations

Under proper usage:

* Near-zero overhead passing borrowed strings (ABI layer: 0.5–3 ns)
* No async work inside handle
* Plugin callback ≈ O(1)

### Optimization Opportunities

The current implementation uses `DashMap` for concurrent access, which provides:

1. **Fine-grained locking**: Better than `Mutex<HashMap>` for high-concurrency workloads
2. **Lock-free reads**: Read operations don't require locking
3. **No poison errors**: Unlike `Mutex`, `DashMap` doesn't have poison errors

Further optimizations if needed:

1. **Separate unary/stream maps**: Avoid enum matching in hot path
2. **Plugin-side optimization**: Minimize thread spawning, use thread pools
3. **Request building**: Reuse buffers/arenas for high-level request construction (~216 ns currently)

The ABI types themselves should not be modified for performance—they are already optimal.

---

## 14. State Key/Value Management

nylon-ring supports **per-request and per-stream state** without changing the ABI layout.

### Per-SID State

Host keeps state per request/stream using `DashMap`:

```rust
state_per_sid: DashMap<u64, HashMap<String, Vec<u8>>>
```

### Host Extension API

```rust
#[repr(C)]
pub struct NrHostExt {
    pub set_state: unsafe extern "C" fn(host_ctx, sid, key: NrStr, value: NrBytes) -> NrBytes,
    pub get_state: unsafe extern "C" fn(host_ctx, sid, key: NrStr) -> NrBytes,
}
```

This allows plugins to:

* Store arbitrary metadata per request / per stream
* Implement WebSocket/session logic
* Implement plugin-local agents
* Persist data between frames

### State Lifecycle

* Created at first `set_state()` call for a `sid`
* Updated any time via `set_state()`
* Destroyed automatically when:
  * Unary call returns (via `send_result`)
  * Streaming call emits `StreamEnd` (or error status)

### Global State (Optional)

Plugins may use `plugin_ctx` to store global state:

```rust
struct PluginState {
    global_map: Mutex<HashMap<String, Vec<u8>>>,
}
```

### Accessing State from Plugin

Plugins access state through helper function:

```rust
// In plugin_init, get host_ext using helper function
let host_ext = unsafe {
    nylon_ring_host::NylonRingHost::get_host_ext(host_ctx)
};

// Store host_ext for later use (e.g., in a static OnceLock)
static HOST_HANDLE: OnceLock<HostHandle> = OnceLock::new();

struct HostHandle {
    ctx: *mut c_void,
    vtable: *const NrHostVTable,
    ext: *const NrHostExt,
}

// In plugin_init:
HOST_HANDLE.set(HostHandle {
    ctx: host_ctx,
    vtable: host_vtable,
    ext: host_ext,
});

// Later in handlers, use state functions:
if let Some(host) = HOST_HANDLE.get() {
    if !host.ext.is_null() {
        let set_state = (*host.ext).set_state;
        set_state(host.ctx, sid, NrStr::from_str("key"), NrBytes::from_slice(value));
        
        let get_state = (*host.ext).get_state;
        let value = get_state(host.ctx, sid, NrStr::from_str("key"));
    }
}
```

**Note**: The `host_ext` pointer may be null if the host doesn't support state management. Always check for null before using.

---

## 15. Extensions (Type-Safe Metadata)

The host provides an `Extensions` struct for type-safe metadata storage in `HighLevelRequest`:

```rust
pub struct Extensions {
    // Type-safe storage using TypeId as keys
    // Zero-cost when empty (1 word vs 3 words for HashMap)
}
```

### Usage

```rust
let mut req = HighLevelRequest {
    method: "GET".to_string(),
    path: "/api".to_string(),
    query: "".to_string(),
    headers: vec![],
    body: vec![],
    extensions: Extensions::new(),
};

// Store type-safe metadata
req.extensions.insert(MyMetadata { user_id: 123 });
req.extensions.insert("routing_key".to_string());

// Retrieve later
if let Some(metadata) = req.extensions.get::<MyMetadata>() {
    println!("User ID: {}", metadata.user_id);
}
```

**Note**: Extensions are **not sent to plugins** - they're for host-side use only (routing, logging, etc.).

---

## 16. Rust Coding Rules

The nylon-ring ecosystem follows strict Rust coding rules for production safety:

### 1. No `unwrap()` or `expect()` in Production Code

* Only allowed in unit tests and benchmarks
* All production code must use proper error handling

### 2. No `anyhow` or `anyhow::Context`

* Use `thiserror` for error type definitions
* Use `Result<T, NylonRingHostError>` or `Result<T, PluginError>`
* Propagate errors with `?` operator only

### 3. All Fallible Functions Return `Result`

* No panic as control flow
* Especially critical in FFI and `extern "C"` functions

### 4. Panic-Safe `extern "C"` Functions

* All `extern "C"` and plugin handlers must be panic-safe
* Use `std::panic::catch_unwind` before crossing FFI boundary
* Never allow panics to propagate across boundaries

### 5. Error Consolidation

* Use a single error enum per crate (e.g., `NylonRingHostError`)
* Error enum must `derive(Debug, thiserror::Error)`
* Do not create custom error structs that pass data through pointers

### 6. Clear Error Messages

Errors must be descriptive:

```rust
#[error("failed to load plugin library: {0}")]
FailedToLoadLibrary(#[source] libloading::Error)
```

### 7. Avoid `panic!` and `assert!`

* Only allowed in benchmarks/tests
* Production code must handle errors gracefully

---

## 17. Naming Conventions

* Library: **nylon-ring**
* ABI: **nylon-ring ABI**
* Plugin entrypoint: `nylon_ring_get_plugin_v1`
* Version: `NR_ABI_VERSION = 1`

---

## 18. Project Structure

The workspace contains:

1. **`nylon-ring`** - Core ABI types + helper functions + `define_plugin!` macro
2. **`nylon-ring-host`** - Host adapter with:
   - `NylonRingHost::load()` - Load plugin
   - `NylonRingHost::call(entry, req)` - Unary RPC with entry-based routing
   - `NylonRingHost::call_stream(entry, req)` - Streaming RPC with entry-based routing
   - `HighLevelRequest` - High-level request builder with `Extensions`
   - `Extensions` - Type-safe metadata storage (similar to `http::Extensions`)
   - Uses `DashMap` for concurrent access (not `Mutex<HashMap>`)
3. **`nylon-ring-plugin-example`** - Example Rust plugin supporting:
   - Multiple entry points ("unary", "stream", "state")
   - Both unary and streaming modes
   - State management demonstration
4. **`nylon-ring-go/`** - Go implementation:
   - **`sdk/`** - High-level Go SDK (similar to Rust's `define_plugin!` macro)
   - **`plugin-example-simple/`** - Simple example using SDK
   - **`plugin-example/`** - Low-level CGO example (advanced)
5. **`nylon-ring-bench`** - Benchmark suite using Criterion.rs
6. **`nylon-ring-bench-plugin`** - Lightweight plugin for benchmarking

---

## 19. Summary

**nylon-ring** is an ABI-level interface for high-performance proxy systems like Nylon/Pingora that require:

* High performance
* High stability (ABI stable)
* Multi-language plugin support
* Async / background task support
* Zero-copy interface between host ↔ plugin
* Both unary and streaming/WebSocket-style communication
* Entry-based routing (plugins support multiple handlers)
* Type-safe metadata storage (Extensions)
* Per-request/stream state management
* Fine-grained concurrent access (DashMap)
* Panic-safe FFI boundaries (via `define_plugin!` macro)

---

## For AI Agents

You are an expert Rust systems engineer helping to design and implement an ABI-stable, non-blocking plugin system called **nylon-ring**.

### Core ABI Types (DO NOT MODIFY LAYOUT)

The core ABI types are defined in `nylon-ring/src/lib.rs`. These are the source of truth:

* `NrStatus` - Status codes (including `StreamEnd` for streaming)
* `NrStr` - UTF-8 string slice (`#[repr(C)]`)
* `NrBytes` - Byte slice (`#[repr(C)]`)
* `NrHeader` - Key-value header pair
* `NrRequest` - Request metadata
* `NrHostVTable` - Host callback table
* `NrHostExt` - Host extension table (state management)
* `NrPluginVTable` - Plugin function table
* `NrPluginInfo` - Plugin metadata

**CRITICAL**: DO NOT change the layout or definitions of these core types. Build helpers and layers AROUND them.

### Current Implementation

The workspace contains:

1. **`nylon-ring`** - Core ABI types + helper functions + `define_plugin!` macro
2. **`nylon-ring-host`** - Host adapter with:
   - `NylonRingHost::load()` - Load plugin
   - `NylonRingHost::call(entry, req)` - Unary RPC with entry-based routing
   - `NylonRingHost::call_stream(entry, req)` - Streaming RPC with entry-based routing
   - `HighLevelRequest` - High-level request builder with `Extensions`
   - `Extensions` - Type-safe metadata storage (similar to `http::Extensions`)
   - Uses `DashMap` for concurrent access (not `Mutex<HashMap>`)
   - Examples: `simple_host`, `streaming_host`, `go_plugin_host`, `go_plugin_host_lowlevel`
3. **`nylon-ring-plugin-example`** - Example Rust plugin supporting:
   - Multiple entry points ("unary", "stream", "state")
   - Both unary and streaming modes
   - State management demonstration
4. **`nylon-ring-go/`** - Go implementation:
   - **`sdk/`** - High-level Go SDK with simple API (similar to Rust's `define_plugin!` macro)
   - **`plugin-example-simple/`** - Simple example using SDK
   - **`plugin-example/`** - Low-level CGO example (advanced, full control)
5. **`nylon-ring-bench`** - Benchmark suite using Criterion.rs
6. **`nylon-ring-bench-plugin`** - Lightweight plugin for benchmarking

### Key Constraints

* All structs must remain `#[repr(C)]` and ABI-stable
* Plugin `handle(entry, sid, req, payload)` must return immediately (non-blocking)
* Entry-based routing: plugins support multiple handlers via entry names
* Host uses `oneshot` for unary, `mpsc::UnboundedSender` for streaming
* Host uses `DashMap` for concurrent access (fine-grained locking)
* Tests assert struct layouts (sizes/alignments) - must always pass
* Assume 64-bit little-endian platform (Linux/macOS)
* All `extern "C"` functions must be panic-safe (handled by `define_plugin!` macro)

### Testing & Benchmarking

Run tests and examples:
```bash
make build         # Build everything (Rust + Go plugins)
make example       # Build and run all examples (Rust + Go)
make example-simple      # Run unary example (Rust plugin)
make example-streaming   # Run streaming example (Rust plugin)
make example-go-plugin   # Run Go plugin example (with SDK)
make example-go-plugin-lowlevel # Run Go plugin example (low-level)
make test          # Run all tests
make benchmark     # Run all benchmarks
make benchmark-abi # ABI type benchmarks
make benchmark-host # Host overhead benchmarks
```

### Error Handling

The host adapter uses `NylonRingHostError` (defined with `thiserror`):

* All functions return `Result<T, NylonRingHostError>`
* No `unwrap()` or `expect()` in production code
* All `extern "C"` functions are panic-safe
* No `MutexPoisoned` error (using `DashMap` instead)

### Performance Notes

* All performance numbers are measured on **Apple M1 Pro (10-core)**
* ABI layer: 0.5–3 ns per operation
* Host overhead: ~0.5-1.3 us per call
* Uses `DashMap` for better concurrency than `Mutex<HashMap>`