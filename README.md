<div align="center">

# 🔗 Nylon Ring

**High-Performance ABI-Stable Host–Plugin Interface**

[![Rust](https://img.shields.io/badge/Rust-000000?style=flat-square&logo=rust&logoColor=white)](https://www.rust-lang.org/)
[![Go](https://img.shields.io/badge/Go-00ADD8?style=flat-square&logo=go&logoColor=white)](https://golang.org/)
[![License](https://img.shields.io/badge/License-MIT-green)](#)

*Write plugins in Rust, Go, C, C++, Zig and more — communicate seamlessly with ABI stability*

[Features](#-features) • [Quick Start](#-quick-start) • [Usage](#-usage) • [Performance](#-performance) • [Architecture](#-architecture)

</div>

---

## 🌟 Features

<table>
<tr>
<td width="50%">

**🔒 ABI-Stable**
- All data structures use C ABI (`#[repr(C)]`)
- Guaranteed compatibility across language boundaries
- Version-safe plugin loading

**🚀 High Performance**
- Optimized for high-throughput workloads
- Excellent multi-core scaling
- Zero-copy data passing with borrowed slices

**🌐 Cross-Language**
- Rust (first-class support)
- Go (high-level SDK)
- C, C++, Zig (native C ABI)

</td>
<td width="50%">

**⚡ Dual Communication Mode**
- **Unary**: Simple request/response
- **Streaming**: WebSocket-style multi-frame
- **Bidirectional**: Full duplex communication

**🔧 Flexible Design**
- Blocking and non-blocking plugins
- Entry-based routing for multiple handlers
- Per-request/stream state management

</td>
</tr>
</table>

---

## 📦 Project Structure

```
nylon-ring/
├── nylon-ring/                    # 🔧 Core ABI library
│   ├── ABI types (NrStr, NrBytes, NrRequest)
│   ├── define_plugin! macro
│   └── Helper functions
│
├── nylon-ring-host/               # 🏠 Host adapter (Rust)
│   ├── NylonRingHost - Main interface
│   ├── HighLevelRequest - Request builder
│   ├── Extensions - Type-safe metadata
│   └── Examples: simple, streaming, go-plugin
│
├── nylon-ring-plugin-example/     # 📝 Example Rust plugin
│   ├── Unary handlers
│   ├── Streaming handlers
│   └── State management examples
│
├── nylon-ring-go/                 # Go implementation
│   ├── sdk/ - High-level Go SDK
│   ├── plugin-example-simple/ - SDK example
│   └── plugin-example/ - Low-level CGO example
│
├── nylon-ring-bench/              # 📊 Benchmark suite (Criterion.rs)
└── nylon-ring-bench-plugin/       # ⚡ Optimized benchmark plugin
```

---

## 🚀 Quick Start

### Build Everything

```bash
# Build all crates (Rust + Go plugins)
make build

# Or build individually
cargo build --release
```

### Run Examples

```bash
# Run all examples (Rust + Go)
make example

# Run individual examples
make example-simple           # Rust plugin - unary
make example-streaming        # Rust plugin - streaming
make example-go-plugin        # Go plugin with SDK
make example-go-plugin-lowlevel  # Go plugin (low-level)
make example-bidirectional       # Rust plugin - bidirectional
make example-bidirectional-go    # Go plugin - bidirectional
```

### Run Tests

```bash
make test                     # Run all tests
make test-all                 # Verbose output
```

---

## 💻 Usage

### 🎯 Entry-Based Routing

Nylon-ring uses **entry-based routing** to support multiple handlers per plugin:

```rust
// Route to different handlers based on entry name
host.call("unary", req).await?;          // → "unary" handler
host.call_stream("stream", req).await?;  // → "stream" handler
host.call_raw("echo", payload).await?;   // → "echo" handler (raw bytes)
host.fast_raw_unary_call("echo", payload).await?;   // → "echo" handler (raw bytes)
```

---

## 🔨 Implementing a Plugin

### [![Rust](https://img.shields.io/badge/Rust-000000?style=flat-square&logo=rust&logoColor=white)](https://www.rust-lang.org/) Plugin

Use the `define_plugin!` macro for easy plugin creation:

```rust
use nylon_ring::{define_plugin, NrBytes, NrHostExt, NrHostVTable, NrRequest, NrStatus, NrStr};
use std::ffi::c_void;
use std::sync::OnceLock;

struct HostHandle {
    ctx: *mut c_void,
    vtable: *const NrHostVTable,
    ext: *const NrHostExt,
}

unsafe impl Send for HostHandle {}
unsafe impl Sync for HostHandle {}

static HOST_HANDLE: OnceLock<HostHandle> = OnceLock::new();

unsafe fn plugin_init(
    _plugin_ctx: *mut c_void,
    host_ctx: *mut c_void,
    host_vtable: *const NrHostVTable,
) -> NrStatus {
    let host_ext = nylon_ring_host::NylonRingHost::get_host_ext(host_ctx);
    let handle = HostHandle {
        ctx: host_ctx,
        vtable: host_vtable,
        ext: host_ext,
    };
    HOST_HANDLE.set(handle).map_or(NrStatus::Err, |_| NrStatus::Ok)
}

unsafe fn handle_unary(
    _plugin_ctx: *mut c_void,
    sid: u64,
    req: *const NrRequest,
    _payload: NrBytes,
) -> NrStatus {
    if req.is_null() {
        return NrStatus::Invalid;
    }
    
    if let Some(host) = HOST_HANDLE.get() {
        let response = b"Hello from plugin!";
        let send_result = (*host.vtable).send_result;
        send_result(
            host.ctx,
            sid,
            NrStatus::Ok,
            NrBytes::from_slice(response),
        );
    }
    
    NrStatus::Ok
}

unsafe fn plugin_shutdown(_plugin_ctx: *mut c_void) {
    // Cleanup if needed
}

define_plugin! {
    init: plugin_init,
    shutdown: plugin_shutdown,
    entries: {
        "unary" => handle_unary,
    },
}
```

**The `define_plugin!` macro automatically:**
- ✅ Creates panic-safe FFI wrappers
- ✅ Exports the `nylon_ring_get_plugin_v1()` entry point
- ✅ Routes requests to handlers based on entry name
- ✅ Handles panics safely across FFI boundaries

---

### [![Go](https://img.shields.io/badge/Go-00ADD8?style=flat-square&logo=go&logoColor=white)](https://golang.org/) Plugin

#### Using SDK (Recommended)

Simple API similar to Rust's `define_plugin!` macro:

```go
package main

import (
    "time"
    "github.com/AssetsArt/nylon-ring/nylon-ring-go/sdk"
)

func init() {
    plugin := sdk.NewPlugin("my-plugin", "1.0.0")
    
    // Async handler - automatically runs in goroutine
    plugin.Handle("unary", func(req sdk.Request, payload []byte, callback func(sdk.Response)) {
        time.Sleep(2 * time.Second)  // Blocking work is OK
        callback(sdk.Response{Status: sdk.StatusOk, Data: []byte("OK")})
    })

    // Sync handler - runs on host thread (for very fast operations)
    plugin.HandleSync("fast", func(req sdk.Request, payload []byte, callback func(sdk.Response)) {
        callback(sdk.Response{Status: sdk.StatusOk, Data: []byte("FAST")})
    })
    
    // Streaming handler
    plugin.Handle("stream", func(req sdk.Request, payload []byte, callback func(sdk.Response)) {
        for i := 1; i <= 5; i++ {
            time.Sleep(1 * time.Second)
            callback(sdk.Response{Status: sdk.StatusOk, Data: []byte("Frame " + string(rune('0'+i)))})
        }
        callback(sdk.Response{Status: sdk.StatusStreamEnd, Data: []byte{}})
    })
    
    sdk.BuildPlugin(plugin)
}
```

#### Low-Level CGO (Advanced)

For full control, use CGO directly. See `nylon-ring-go/plugin-example/` for a complete example.

---

## 🏠 Loading a Plugin (Host)

### Unary Call

```rust
use nylon_ring_host::{Extensions, HighLevelRequest, NylonRingHost};

let host = NylonRingHost::load("path/to/plugin.so")?;

let req = HighLevelRequest {
    method: "GET".to_string(),
    path: "/api/data".to_string(),
    query: "".to_string(),
    headers: vec![("User-Agent".to_string(), "MyApp/1.0".to_string())],
    body: vec![],
    extensions: Extensions::new(),
};

// Async call - routes to "unary" handler in plugin
let (status, payload) = host.call("unary", req).await?;
println!("Status: {:?}, Response: {}", status, String::from_utf8_lossy(&payload));
```

### Streaming Call

```rust
use nylon_ring::NrStatus;
use nylon_ring_host::{Extensions, HighLevelRequest, NylonRingHost};

let host = NylonRingHost::load("path/to/plugin.so")?;

let req = HighLevelRequest {
    method: "GET".to_string(),
    path: "/stream".to_string(),
    query: "".to_string(),
    headers: vec![],
    body: vec![],
    extensions: Extensions::new(),
};

// Get stream receiver - routes to "stream" handler
let mut stream = host.call_stream("stream", req).await?;

// Receive frames
while let Some(frame) = stream.recv().await {
    println!("Frame - Status: {:?}, Data: {}", 
        frame.status, 
        String::from_utf8_lossy(&frame.data)
    );
    
    if matches!(
        frame.status,
        NrStatus::StreamEnd | NrStatus::Err | NrStatus::Invalid | NrStatus::Unsupported
    ) {
        break;
    }
}

// Send data back to plugin (Bidirectional)
host.send_stream_data(sid, b"Hello Plugin")?;

// Close stream from host
host.close_stream(sid)?;
```

### Raw Call (Bypass NrRequest)

```rust
// Send raw bytes directly (fastest path)
let payload = b"Hello, Raw World!";
let (status, response) = host.call_raw("echo", payload).await?;
println!("Status: {:?}, Response: {:?}", status, String::from_utf8_lossy(&response));
```

---

## 🏗️ Architecture

### Unary Flow

```
┌─────────┐                         ┌────────┐
│  Host   │                         │ Plugin │
└────┬────┘                         └───┬────┘
     │                                  │
     │  handle(entry, sid, req)         │
     │─────────────────────────────────>│
     │                                  │
     │         return Ok                │
     │<─────────────────────────────────│
     │                                  │
     │                                  │ [spawn background task]
     │                                  │ [do work...]
     │                                  │
     │      send_result(sid)            │
     │<─────────────────────────────────│
     │                                  │
```

### Streaming Flow

```
┌─────────┐                         ┌────────┐
│  Host   │                         │ Plugin │
└────┬────┘                         └───┬────┘
     │                                  │
     │  handle(entry, sid, req)         │
     │─────────────────────────────────>│
     │                                  │
     │         return Ok                │
     │<─────────────────────────────────│
     │                                  │ [spawn background task]
     │                                  │
     │      send_result(sid) [frame 1]  │
     │<─────────────────────────────────│
     │      send_result(sid) [frame 2]  │
     │<─────────────────────────────────│
     │      send_result(sid) [frame 3]  │
     │<─────────────────────────────────│
     │      send_result(sid) [StreamEnd]│
     │<─────────────────────────────────│
     │                                  │
```

---

## ⚡ Performance

> **Note**: All benchmarks measured on **Apple M1 Pro (10-core)** with release builds.

### 🔧 ABI Types Performance

The ABI layer is extremely lightweight:

| Operation | Time (ns) | Notes |
|-----------|-----------|-------|
| `NrStr::from_str` | ~0.51 | Creating string view |
| `NrStr::as_str` | ~0.49 | Reading string |
| `NrBytes::from_slice` | ~0.35 | Creating byte view |
| `NrBytes::as_slice` | ~0.43 | Reading bytes |
| `NrHeader::new` | ~1.09 | Creating header |
| `NrRequest::build` | ~2.48 | Building request |

**💡 Conclusion**: ABI overhead is negligible (0.5–2.5 ns) — the bottleneck will never be the ABI layer.

---

### 🏎️ Host Overhead Performance

Full round-trip performance (host → plugin → host callback):

| Benchmark | Time | Throughput | Notes |
|-----------|------|------------|-------|
| **Unary call** | ~0.43 µs | **~2.32M calls/sec** | Single core |
| **Unary + 1KB body** | ~0.49 µs | **~2.05M calls/sec** | Body size has minimal impact |
| **Raw unary** | ~0.16 µs | **~6.31M calls/sec** | Bypass NrRequest |
| **Fast raw unary** | ~0.14 µs | **~7.14M calls/sec** | Thread-local optimization (see below) |
| **Streaming** | ~0.83 µs | **~1.20M calls/sec** | All frames consumed |
| **Raw streaming** | ~0.77 µs | **~1.29M calls/sec** | Bypass NrRequest |
| **Bidirectional** | ~0.93 µs | **~1.07M calls/sec** | 5 frames + 1 echo |
| **Build request** | ~216 ns | N/A | `HighLevelRequest` creation |

**Overhead sources:**
- FFI crossing (`extern "C"` calls)
- Async scheduling (Tokio runtime)
- Concurrent map operations (`DashMap`)
- Plugin's own work

---

### ⚡ Fast Unary Path

The **fast raw unary** path (`call_raw_unary_fast`) is an optimized code path that achieves the highest throughput by making specific trade-offs:

**✅ Advantages:**
- **Highest performance**: ~7.14M calls/sec (single-core)
- Thread-local optimization reduces contention
- Minimal overhead (no request parsing)

**⚠️ Constraints:**
- **Plugin handler must be synchronous** — cannot use `async`/`.await`
- **No thread spawning** — cannot use `thread::spawn()` or task executors
- **Must complete immediately** — all work done in the calling thread
- Use only for CPU-bound, non-blocking operations (<1µs)

**When to use:**
- Simple transformations (echo, hash, encode/decode)
- Stateless operations
- Hot path optimizations

**When NOT to use:**
- I/O operations (file, network, database)
- Long-running computations
- Operations requiring background tasks

For most use cases, prefer the standard paths (`call`, `call_raw`) which support async and threading.

---

### 🔥 Multi-Core Scaling

**Stress test results** (10-core Apple M1 Pro, 10-second run):

| Path | Throughput | Total Requests | Notes |
|------|------------|----------------|-------|
| **Standard** (`call_raw`) | **~11.16M req/sec** | 111.6M requests | Good scaling |
| **Fast path** (`call_raw_unary_fast`) | **~14.65M req/sec** | 146.5M requests | **+31.2% faster** |

**📊 Scaling efficiency**: Nearly **2x** throughput per core vs single-core benchmarks, indicating excellent parallel processing with minimal contention.

> **Note**: These tests use a custom allocator (MiMalloc) and batch processing for maximum throughput. See [Benchmark Methodology](#-benchmark-methodology) below.

---

### 📊 Benchmark Methodology

We use two types of benchmarks to measure different aspects of performance:

#### Micro-Benchmarks (Criterion.rs)

Measure isolated component performance with statistical rigor:

- **ABI types**: `NrStr`, `NrBytes`, `NrHeader`, `NrRequest` construction and access (~0.35–2.5 ns)
- **Host overhead**: Full round-trip including FFI, async scheduling, and callbacks (~0.14–0.83 µs)
- **Method**: Criterion.rs with warmup, multiple iterations, outlier detection
- **Environment**: Single-threaded, minimal external load
- **Use case**: Validate that ABI layer adds negligible overhead

#### Stress Tests (Multi-Core)

Measure aggregate throughput under sustained load:

- **Setup**: 10 worker threads, each making continuous requests for 10 seconds
- **Measurement**: Total requests completed / elapsed time
- **Allocator**: MiMalloc for reduced allocation contention (optional optimization)
- **Environment**: Multi-core, batch processing, realistic concurrency
- **Use case**: Validate scaling efficiency and find maximum sustainable throughput

**Key difference**: Micro-benchmarks measure latency per operation; stress tests measure aggregate throughput.

### 🧪 Run Benchmarks

```bash
make benchmark              # All benchmarks (micro + stress)
make benchmark-abi         # ABI types only (micro)
make benchmark-host        # Host overhead (micro)
make stress-test           # Multi-core stress test
```

> ⚠️ **Note**: Results are hardware-dependent. Your mileage may vary based on CPU architecture, clock speed, core count, and system load.

---

## 🗂️ State Management

Nylon-ring supports **per-request and per-stream state** without changing the ABI.

### Host-Side State Store

```rust
// Host maintains concurrent state per SID
state_per_sid: DashMap<u64, HashMap<String, Vec<u8>>>
```

### 🔌 Host Extension API

Plugins access state through `NrHostExt`:

```rust
#[repr(C)]
pub struct NrHostExt {
    pub set_state: unsafe extern "C" fn(host_ctx, sid, key: NrStr, value: NrBytes) -> NrBytes,
    pub get_state: unsafe extern "C" fn(host_ctx, sid, key: NrStr) -> NrBytes,
}
```

### Using State in Plugins

```rust
// Get host extension in plugin_init
let host_ext = unsafe {
    nylon_ring_host::NylonRingHost::get_host_ext(host_ctx)
};

// Set state
host_ext.set_state(host_ctx, sid, NrStr::from_str("key"), NrBytes::from_slice(value));

// Get state
let value = host_ext.get_state(host_ctx, sid, NrStr::from_str("key"));
```

### 🔄 State Lifecycle

- ✅ Created automatically on first `set_state()` call
- ✅ Persists for the entire request/stream lifetime
- ✅ Automatically cleared when:
  - Unary call completes
  - Streaming call ends (`StreamEnd` or error)

**Use cases:**
- WebSocket session management
- Per-request metadata storage
- Plugin-local agent state
- Frame-to-frame data persistence

---

## 🏷️ Extensions (Type-Safe Metadata)

`HighLevelRequest` supports type-safe metadata storage:

```rust
use nylon_ring_host::{Extensions, HighLevelRequest};

let mut req = HighLevelRequest {
    method: "GET".to_string(),
    path: "/api".to_string(),
    query: "".to_string(),
    headers: vec![],
    body: vec![],
    extensions: Extensions::new(),
};

// Store type-safe metadata (host-side only)
req.extensions.insert(MyMetadata { user_id: 123 });
req.extensions.insert("routing_key".to_string());

// Retrieve later
if let Some(metadata) = req.extensions.get::<MyMetadata>() {
    println!("User ID: {}", metadata.user_id);
}
```

> ⚠️ **Note**: Extensions are **not sent to plugins** — use them for host-side routing, logging, or metadata.

---

## 📋 Core Design Principles

| Principle | Description |
|-----------|-------------|
| **🔒 ABI Stability** | All data structures are `#[repr(C)]` |
| **⚡ Flexibility** | Plugins can be blocking or non-blocking |
| **🔄 Callback Mechanism** | Plugin reports results via `send_result` callback |
| **📡 Streaming Support** | Multiple frames per request (WebSocket-style) |
| **🛡️ Panic-Safe FFI** | All `extern "C"` functions catch panics |
| **🧵 Thread-Safe** | `send_result` can be called from any thread |
| **🎯 Entry Routing** | Multiple handlers per plugin via entry names |

---

## ⚠️ Key Constraints

- ✅ Plugin `handle()` **can block** (but use background tasks for high performance)
- ✅ All ABI types are `#[repr(C)]` — **do not modify layout**
- ✅ Host owns request data — **plugin must copy if needed**
- ✅ Thread-safe callbacks — `send_result` callable from any thread
- ✅ Panic-safe FFI — handled automatically by `define_plugin!`
- ❌ No `unwrap()` in production — **proper error handling required**
- ✅ Concurrent access — Host uses `DashMap` for fine-grained locking
- ✅ Entry-based routing — Plugins support multiple handlers

---

## ❌ Error Handling

Uses `thiserror` for clean error types:

```rust
// All functions return Result
pub enum NylonRingHostError {
    #[error("Failed to load library: {0}")]
    LoadError(String),
    
    #[error("ABI version mismatch")]
    VersionMismatch,
    
    // ... more variants
}
```

**Principles:**
- ✅ All fallible functions return `Result`
- ✅ Clear, descriptive error messages
- ✅ No `anyhow` dependency
- ✅ Panic-safe callbacks

See `nylon-ring-host/src/error.rs` for implementation.

---

## 🦀 Rust Coding Rules

Strict production-safety guidelines:

1. ❌ No `unwrap()` or `expect()` in production (only tests/benchmarks)
2. ❌ No `anyhow` — use `thiserror` for error types
3. ✅ All fallible functions return `Result`
4. ✅ Panic-safe `extern "C"` functions
5. ✅ Single error enum per crate with `thiserror::Error`
6. ✅ Clear, descriptive error messages
7. ❌ Avoid `panic!` and `assert!` (only in tests/benchmarks)

---

## 🌍 Multi-Language Support

| Language | Support Level | Notes |
|----------|---------------|-------|
| **![Rust](https://img.shields.io/badge/Rust-000000?style=flat-square&logo=rust&logoColor=white)** | ⭐⭐⭐⭐⭐ | First-class with `define_plugin!` macro |
| **[![Go](https://img.shields.io/badge/Go-00ADD8?style=flat-square&logo=go&logoColor=white)](https://golang.org/)** | ⭐⭐⭐⭐⭐ | High-level SDK + low-level CGO |
| **![C](https://img.shields.io/badge/C-000000?style=flat-square&logo=c&logoColor=white) / ![C++](https://img.shields.io/badge/C++-000000?style=flat-square&logo=c++&logoColor=white)** | ⭐⭐⭐⭐ | Direct C ABI match (low-level) |
| **![Zig](https://img.shields.io/badge/Zig-000000?style=flat-square&logo=zig&logoColor=white)** | ⭐⭐⭐⭐ | C ABI support (low-level) |
| **🔧 Others** | ⭐⭐⭐ | Any language with C FFI (low-level) |

> **Note**: High-level SDKs for C, C++, Zig, and other languages are **coming soon**. Currently, only Rust and Go have high-level SDK support.

### 📁 Examples

- **Rust**: `nylon-ring-plugin-example/`
- **Go SDK**: `nylon-ring-go/plugin-example-simple/`
- **Go CGO**: `nylon-ring-go/plugin-example/`

---

## 💻 Platform Support

| Platform | Extension | Status |
|----------|-----------|--------|
| **🐧 Linux** | `.so` | ✅ Supported |
| **🍎 macOS** | `.dylib` | ✅ Supported |
| **🪟 Windows** | `.dll` | ✅ Supported |

---

## 📚 Additional Resources

### Core Types

- `NrStr` / `NrBytes` — ABI-stable string and byte slices
- `NrRequest` — Request metadata (method, path, headers)
- `NrStatus` — Status codes (`Ok`, `Err`, `Invalid`, `Unsupported`, `StreamEnd`)
- `NrHostVTable` — Host function pointers (e.g., `send_result`)
- `NrPluginVTable` — Plugin function pointers (`init`, `handle`, `shutdown`)

### Examples

Run the examples to see everything in action:

```bash
make example              # All examples
make example-simple      # Rust unary
make example-streaming   # Rust streaming
make example-go-plugin   # Go SDK
```

---
