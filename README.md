<div align="center">

# 🔗 Nylon Ring

**Ultra-Fast ABI-Stable Host–Plugin Interface for Rust**

[![Rust](https://img.shields.io/badge/Rust-000000?style=flat-square&logo=rust&logoColor=white)](https://www.rust-lang.org/)
[![License](https://img.shields.io/badge/License-MIT-green)](#)

*Write blazing-fast plugins with ABI stability — extreme multi-thread performance*

[Features](#-features) • [Quick Start](#-quick-start) • [Usage](#-usage) • [Performance](#-performance)

</div>

---

## ⚡ Performance Highlights

> Benchmarked on **Apple M1 Pro (10-core)** — Release builds

```
call(...):               140M~ req/sec (10 threads, fire-and-forget)
call_response_fast(...): 124M~ req/sec (10 threads, unary fast with response *thread-local optimized*)
call_response(...):      27M~ req/sec (10 threads, unary with response)
```

---

## 🌟 Features

### 🔒 **ABI-Stable**
- All data structures use C ABI (`#[repr(C)]`)
- Version-safe plugin loading across Rust versions
- Compatible with C, C++, Zig, Go, Rust, ...

### 🚀 **Extreme Performance**
- **140M+ req/sec** multi-thread throughput
- **Thread-local SID** generation (zero atomic operations)
- **Zero-copy** data transfer with `NrVec<u8>`
- Sub-nanosecond ABI overhead

### ⚡ **Flexible Call Patterns**
- **Fire-and-forget**: ~71.7ns (fastest)
- **Unary with response**: ~143.2ns
- **Fast path**: ~95.5ns (thread-local optimized)
- **Streaming**: Bi-directional communication (Host ↔ Plugin)
- **Plugin-to-Plugin**: Low-latency dispatch between plugins

### 🔧 **Production Ready**
- Thread-safe for 24/7 HTTP servers
- Safe SID wrapping (no collision)
- Entry-based routing for multiple handlers
- Panic-safe FFI boundaries

---

## 📦 Project Structure

```
nylon-ring/
├── crates/
│   ├── nylon-ring/              # Core ABI library & Plugin Dispatcher
│   │   ├── src/                 # NrStr, NrBytes, NrKV, NrVec
│   │   └── benches/             # ABI benchmarks
│   │
│   └── nylon-ring-host/         # Host adapter
│       ├── src/                 # NylonRingHost interface
│       └── benches/             # Host overhead benchmarks
│
└── examples/
    ├── ex-nyring-plugin/        # Example plugin (Async, Stream, Dispatch)
    └── ex-nyring-host/          # Example host + stress test
```

---

## 🏗️ System Overview (Architecture)

The **Nylon Ring** architecture is designed around a strictly defined ABI boundary that separates the Host runtime from Plugin logic, connected by a high-performance routing layer.

```text
+-----------------------------------------------------------+
|               Host Layer (nylon-ring-host)                |
|                                                           |
|  [Public API] NylonRingHost (Container)                   |
|       |          |                                        |
|       |          +---- [LoadedPlugin A] <----+            |
|       |          |                           |            |
|       |          +---- [LoadedPlugin B]      |            |
|       |                                      |            |
|       v (1. Get SID)                         |            |
|    [ID Generator] <-----> [Shared Host Context]           |
|       |                   +---------------------------+   |
|       |                   |  [Thread-Local Slot]      |   |
|       |                   |   (Zero Contention)       |   |
|       |                   +---------------------------+   |
|       |                   |  [Sharded DashMap]        |   |
|       |                   |   (64 Shards)             |   |
|       |                   +---------------------------+   |
|       |                                 ^                 |
|       v (2. FFI Call via PluginHandle)  |                 |
|    [PluginHandle] ----------------------+                 |
|       |                                 |                 |
|       v                                 | (3. send_result)|
+-------+---------------------------------+-------^---------+
|       |            ABI Boundary         |       |         |
|       v                                 |       |         |
|   [VTable Interface]               [Callback Router]      |
|                                         ^       ^         |
|                                         |       |         |
+-------+---------------------------------+-------+---------+
        |                                 |       |
        v                                 |       | (4. Dispatch)
+-------+---------------------------------+-------+---------+
|       |            Plugin Layer         |       |         |
|       v                                 |       |         |
|   [Business Logic] ---------------------+       |         |
|         |                                       |         |
|         +----> [PluginDispatcher] --------------+         |
|                                                           |
+-----------------------------------------------------------+
```

### 1. The Host Layer (`nylon-ring-host`)
The runtime environment that manages plugin lifecycles and request routing.
- **Multi-Plugin Support**: `NylonRingHost` acts as a container for multiple `LoadedPlugin` instances. Each plugin is isolated but shares the underlying host context (state map, ID generator).
- **Hybrid State Management**:
    - **Fast Path (Sync)**: Uses `Thread-Local Storage` (TLS) to store result slots. This eliminates all lock contention and atomic operations for synchronous calls.
    - **Standard Path (Async)**: Uses a **Sharded DashMap** (64 shards) to track pending requests. Sharding minimizes lock contention in multi-threaded environments.
- **ID Generation**: simple, thread-local counter with blocked allocation (1M per block) to avoid global atomic contention.
- **Routing**: The callback handler uses a **Waterfall Strategy**:
    1.  Check **TLS Slot** (Is this a fast synchronous response on the same thread?).
    2.  Check **Sharded Map** (Is this an async response from any thread?).

### 2. The ABI Layer (`nylon-ring`)
Defines the strictly stable interface between Host and Plugin.
- **Stable Memory Layout**: All exchanged types (`NrVec`, `NrStr`, `NrStatus`) are `#[repr(C)]`, guaranteeing identical memory representation across languages (Rust, C++, etc.).
- **Zero-Copy Protocol**: `NrVec<T>` allows ownership of heap-allocated memory (like a `Vec<u8>`) to be transferred across the FFI boundary without copying.

### 3. The Plugin Layer
The implementer of business logic.
- **Stateless & Async-Agnostic**: Plugins receive an ID and Payload. They process it (sync or async) and call `send_result` when finished. The Host handles the complexity of mapping that result back to the original caller.

### 4. The Dispatcher Layer
Enables advanced plugin interaction.
- **Plugin-to-Plugin calls**: The `PluginDispatcher` allows a plugin to call back into the Host to invoke another plugin.
- **Streaming**: Manages stream initiation and frame transmission back to the host.

---

## 🚀 Quick Start

### Build

```bash
cargo build --release
```

### Run Demo

```bash
cargo run --release --bin ex-nyring-host
```

### Run Benchmarks

```bash
cargo bench                           # All benchmarks
cargo bench --package nylon-ring      # ABI types only
cargo bench --package nylon-ring-host # Host overhead only
```

---

## 💻 Usage

### Host: Plugin Management

```rust
use nylon_ring_host::NylonRingHost;

let mut host = NylonRingHost::new();

// Load plugins
host.load("plugin_a", "libs/plugin_a.so")?;
host.load("plugin_b", "libs/plugin_b.so")?;

// Get a handle to a specific plugin
let plugin_a = host.plugin("plugin_a").expect("Plugin A not found");

// Reload all plugins (useful for hot-swapping)
host.reload()?;

// Unload a plugin
host.unload("plugin_b")?;
```

### Host: Calling a Plugin

#### Fire-and-Forget (Fastest)

```rust
use nylon_ring_host::NylonRingHost;

let mut host = NylonRingHost::new();
host.load("default", "target/release/libmy_plugin.so")?;

let plugin = host.plugin("default").expect("Plugin not found");

// Fire-and-forget - no response waiting (~71.7ns, 13.95M calls/sec)
let status = plugin.call("handler_name", b"payload").await?;
```

#### Unary with Response

```rust
// Wait for response from plugin (~143.2ns, 6.98M calls/sec)
let (status, response) = plugin.call_response("handler_name", b"payload").await?;
println!("Response: {}", String::from_utf8_lossy(&response));
```

#### Fast Path

```rust
// Thread-local optimized path (~95.5ns, 10.47M calls/sec)
// Synchronous, avoids DashMap overhead by using Thread-Local Storage.
let (status, response) = plugin.call_response_fast("handler_name", b"payload").await?;
```

#### Streaming

Bi-directional streaming support. Example: Plugin produces multiple frames.

```rust
use nylon_ring::NrStatus;

// Start streaming call
let (sid, rx) = plugin.call_stream("stream_handler", b"payload").await?;

// Receive frames using blocking iterator (compatible with async runtime)
for frame in rx {
    println!("Data: {}", String::from_utf8_lossy(&frame.data));
    
    if matches!(frame.status, NrStatus::StreamEnd | NrStatus::Err) {
        break;
    }
}

// Host can also send data TO the stream
plugin.send_stream_data(sid, b"host_data")?;

// Close stream from host side
plugin.close_stream(sid)?;
```

---

### Plugin: Implementing Handlers

Using the `define_plugin!` macro and `NrVec` for zero-copy transfers.

```rust
use nylon_ring::{define_plugin, NrBytes, NrHostVTable, NrStatus, NrVec};
use std::ffi::c_void;

// Global state to store host context and vtable
static mut HOST_CTX: *mut c_void = std::ptr::null_mut();
static mut HOST_VTABLE: *const NrHostVTable = std::ptr::null();

// Initialize plugin
unsafe fn init(host_ctx: *mut c_void, host_vtable: *const NrHostVTable) -> NrStatus {
    HOST_CTX = host_ctx;
    HOST_VTABLE = host_vtable;
    println!("[Plugin] Initialized!");
    NrStatus::Ok
}

// 1. Unary Handler (Zero-Copy Response)
unsafe fn handle_echo(sid: u64, payload: NrBytes) -> NrStatus {
    // Zero-copy read
    let data = payload.as_slice();
    let text = String::from_utf8_lossy(data);
    
    // Create zero-copy response vector
    let response = format!("Echo: {}", text);
    let nr_vec = NrVec::from_string(response);

    // Send result back to host
    ((*HOST_VTABLE).send_result)(HOST_CTX, sid, NrStatus::Ok, nr_vec);
    
    NrStatus::Ok
}

// 2. Stream Handler (Multiple Frames)
unsafe fn handle_stream(sid: u64, _payload: NrBytes) -> NrStatus {
    for i in 1..=3 {
        let msg = format!("Frame {}", i);
        let nr_vec = NrVec::from_string(msg);
        ((*HOST_VTABLE).send_result)(HOST_CTX, sid, NrStatus::Ok, nr_vec);
    }
    // End stream
    ((*HOST_VTABLE).send_result)(HOST_CTX, sid, NrStatus::StreamEnd, NrVec::new());
    NrStatus::Ok
}

// 3. Plugin-to-Plugin Dispatch
unsafe fn handle_dispatch(_sid: u64, _payload: NrBytes) -> NrStatus {
    // Create a dispatcher helper
    let dispatcher = nylon_ring::PluginDispatcher::new(HOST_CTX, &*HOST_VTABLE, "other_plugin");
    
    // Call another plugin
    let (status, response) = dispatcher.call_response("entry_point", b"hello");
    
    NrStatus::Ok
}

// Plugin shutdown
fn shutdown() {
    println!("[Plugin] Shutdown");
}

// Define plugin with entry points
define_plugin! {
    init: init,
    shutdown: shutdown,
    entries: {
        "echo" => handle_echo,
        "stream" => handle_stream,
        "dispatch" => handle_dispatch,
    }
}
```

**The `define_plugin!` macro:**
- ✅ Creates panic-safe FFI wrappers
- ✅ Exports `nylon_ring_get_plugin_v1()` entry point
- ✅ Routes requests by method name
- ✅ Handles panics across FFI boundaries

---

## 📊 Performance

> Measured on **Apple M1 Pro (10-core)** with release builds

### ABI Types (Criterion Benchmarks)

| Operation | Time | Notes |
|-----------|------|-------|
| `NrStr::new` | **1.03 ns** | Create string view |
| `NrStr::as_str` | **0.33 ns** | Read string |
| `NrBytes::from_slice` | **0.54 ns** | Create byte view |
| `NrBytes::as_slice` | **0.33 ns** | Read bytes |
| `NrKV::new` | **1.99 ns** | Key-value pair |
| `NrVec::from_vec` | **22.7 ns** | Vec conversion |
| `NrVec::into_vec` | **9.38 ns** | Back to Vec |
| `NrVec::push` (100 items) | **323 ns** | Push 100 values |

**Key Insight**: ABI overhead is negligible (sub-ns to 23ns)

---

### Host Overhead (Single-Thread)

| Operation | Time | Throughput | Notes |
|-----------|------|------------|-------|
| **Fire-and-forget** | **71.7 ns** | **13.95M calls/sec** | Fastest ⚡ |
| **Fast path** | **95.5 ns** | **10.47M calls/sec** | Thread-local |
| **Standard unary** | **143.2 ns** | **6.98M calls/sec** | With response |
| **+ 128B payload** | **158.7 ns** | **6.30M calls/sec** | Small data |
| **+ 1KB payload** | **193.7 ns** | **5.16M calls/sec** | Medium data |
| **+ 4KB payload** | **228.5 ns** | **4.38M calls/sec** | Large data |

---

### Multi-Core Scaling

| Configuration | Throughput | Latency |
|--------------|------------|---------|
| **10 threads (fire-and-forget)** | **140M+ req/sec** | **70 ns** |
| **10 threads (fast path)** | **124.8M req/sec** | **77 ns** |
| **10 threads (standard)** | **27.3M req/sec** | **362 ns** |

**Key Optimization**: Thread-local SID generation eliminates atomic operations entirely

---

## 🔬 Design Principles

| Principle | Implementation |
|-----------|----------------|
| **ABI Stability** | All types are `#[repr(C)]` |
| **Zero Atomic Ops** | Thread-local SID generation |
| **Zero Copy** | `NrVec<u8>` ownership transfer |
| **Panic Safety** | FFI boundaries catch panics |
| **Thread Safety** | Safe for multi-threaded hosts |
| **Fast Path** | Specialized optimizations for sync calls |

---

## License

MIT License

---

## Acknowledgments

Inspired by high-performance plugin systems and FFI best practices.

Built with:
- **Tokio** — Async runtime
- **DashMap** — Concurrent hashmap
- **FxHash** — Fast hashing
- **Criterion** — Benchmarking
- **libloading** — Dynamic library loading
