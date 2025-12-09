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
- **Streaming**: Bi-directional communication

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
│   ├── nylon-ring/              # Core ABI library
│   │   ├── src/                 # NrStr, NrBytes, NrKV, NrVec
│   │   └── benches/             # ABI benchmarks
│   │
│   └── nylon-ring-host/         # Host adapter
│       ├── src/                 # NylonRingHost interface
│       └── benches/             # Host overhead benchmarks
│
└── examples/
    ├── ex-nyring-plugin/        # Example plugin
    └── ex-nyring-host/          # Example host + stress test
```

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

### Host: Loading a Plugin

#### Fire-and-Forget (Fastest)

```rust
use nylon_ring_host::NylonRingHost;

let host = NylonRingHost::load("target/release/libmy_plugin.so")?;

// Fire-and-forget - no response waiting (~71.7ns, 13.95M calls/sec)
let status = host.call("handler_name", b"payload").await?;
```

#### Unary with Response

```rust
// Wait for response from plugin (~143.2ns, 6.98M calls/sec)
let (status, response) = host.call_response("handler_name", b"payload").await?;
println!("Response: {}", String::from_utf8_lossy(&response));
```

#### Fast Path

```rust
// Thread-local optimized path (~95.5ns, 10.47M calls/sec)
let (status, response) = host.call_response_fast("handler_name", b"payload").await?;
```

#### Streaming

```rust
use nylon_ring::NrStatus;

// Start streaming
let (sid, mut rx) = host.call_stream("stream_handler", b"payload").await?;

// Receive frames
while let Some(frame) = rx.recv().await {
    println!("Data: {}", String::from_utf8_lossy(&frame.data));
    
    if matches!(frame.status, NrStatus::StreamEnd | NrStatus::Err) {
        break;
    }
}
```

---

### Plugin: Implementing Handlers

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
    NrStatus::Ok
}

// Handler example
unsafe fn handle_echo(sid: u64, payload: NrBytes) -> NrStatus {
    // Echo back using zero-copy NrVec
    let nr_vec = NrVec::from_slice(payload.as_slice());
    let send_result = (*HOST_VTABLE).send_result;
    send_result(HOST_CTX, sid, NrStatus::Ok, nr_vec);
    NrStatus::Ok
}

// Plugin shutdown
fn shutdown() {
    // Cleanup
}

// Define plugin with entry points
define_plugin! {
    init: init,
    shutdown: shutdown,
    entries: {
        "echo" => handle_echo,
    },
}
```

**The `define_plugin!` macro:**
- ✅ Creates panic-safe FFI wrappers
- ✅ Exports `nylon_ring_get_plugin_v1()` entry point
- ✅ Routes requests by entry name
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

### System Overview

The **Nylon Ring** architecture is designed around a strictly defined ABI boundary that separates the Host runtime from Plugin logic, connected by a high-performance routing layer.

```mermaid
graph TD
    subgraph Host ["Host Layer (nylon-ring-host)"]
        API[Public API]
        SID[ID Generator]
        Router[Callback Router]
        
        subgraph State ["State Management"]
            TLS["Thread-Local Slot<br>(Zero Contention)"]
            Map["Sharded DashMap<br>(64 Shards)"]
        end
    end

    subgraph ABI ["ABI Boundary (nylon-ring)"]
        Structs["#[repr(C)] Types"]
        VTable["VTable Interface"]
    end

    subgraph Plugin ["Plugin Layer"]
        Logic[Business Logic]
    end

    API -->|1. Get SID| SID
    API -->|2. FFI Call| VTable
    VTable --> Logic
    
    Logic -->|3. send_result| Router
    
    Router -->|Waterfall Check 1| TLS
    Router -->|Waterfall Check 2| Map
    
    TLS -.->|Synchronous Wake| API
    Map -.->|Async Wake (Oneshot)| API
```

#### 1. The Host Layer (`nylon-ring-host`)
The runtime environment that manages plugin lifecycles and request routing.
- **Hybrid State Management**:
    - **Fast Path (Sync)**: Uses `Thread-Local Storage` (TLS) to store result slots. This eliminates all lock contention and atomic operations for synchronous calls.
    - **Standard Path (Async)**: Uses a **Sharded DashMap** (64 shards) to track pending requests. Sharding minimizes lock contention in multi-threaded environments.
- **ID Generation**: simple, thread-local counter with blocked allocation (1M per block) to avoid global atomic contention.
- **Routing**: The callback handler uses a **Waterfall Strategy**:
    1.  Check **TLS Slot** (Is this a fast synchronous response on the same thread?).
    2.  Check **Sharded Map** (Is this an async response from any thread?).

#### 2. The ABI Layer (`nylon-ring`)
Defines the strictly stable interface between Host and Plugin.
- **Stable Memory Layout**: All exchanged types (`NrVec`, `NrStr`, `NrStatus`) are `#[repr(C)]`, guaranteeing identical memory representation across languages (Rust, C++, etc.).
- **Zero-Copy Protocol**: `NrVec<T>` allows ownership of heap-allocated memory (like a `Vec<u8>`) to be transferred across the FFI boundary without copying.

#### 3. The Plugin Layer
The implementer of business logic.
- **Stateless & Async-Agnostic**: Plugins receive an ID and Payload. They process it (sync or async) and call `send_result` when finished. The Host handles the complexity of mapping that result back to the original caller.

---

## Core Types

### ABI Types (`nylon-ring`)

- **`NrStr`** — String view (`&str` equivalent)
- **`NrBytes`** — Byte slice view (`&[u8]` equivalent)
- **`NrKV`** — Key-value pair
- **`NrVec<T>`** — Owned vector with zero-copy transfer
- **`NrStatus`** — Result status enum
- **`NrHostVTable`** — Host callbacks
- **`NrPluginVTable`** — Plugin entry points

### Host Types (`nylon-ring-host`)

- **`NylonRingHost`** — Main host interface
- **`StreamFrame`** — Streaming data frame
- **`StreamReceiver`** — Stream receiver channel

---

## 🎯 Use Cases

### ✅ Perfect For
- **High-throughput HTTP servers** (REST, GraphQL)
- **WebSocket backends**
- **RPC services**
- **Plugin systems** requiring isolation
- **Hot-reloadable** business logic

### ⚠️ Consider Alternatives For
- Cross-language plugins (use direct FFI)
- Very low latency requirements (<10ns)
- Single-threaded only workloads

---

## 📈 Benchmark Methodology

### ABI Benchmarks
- **Tool**: Criterion.rs with statistical analysis
- **Iterations**: 100 samples, outlier detection
- **Warmup**: Automatic warmup period
- **Output**: HTML reports in `target/criterion/`

### Host Overhead Benchmarks
- **Method**: Full round-trip (host → plugin → callback)
- **Plugin**: Example plugin with minimal work
- **Runtime**: Tokio async runtime
- **Builds**: Release builds only

### Multi-Thread Stress Test
- **Method**: 10 threads, 100 req/batch, 10-second run
- **Pattern**: Fire-and-forget (no response wait)
- **Total**: 1.45B+ requests in 10 seconds

---

## 🔬 Design Principles

| Principle | Implementation |
|-----------|----------------|
| **ABI Stability** | All types are `#[repr(C)]` |
| **Zero Atomic Ops** | Thread-local SID generation |
| **Zero Copy** | `NrVec<u8>` ownership transfer |
| **Panic Safety** | FFI boundaries catch panics |
| **Thread Safety** | Safe for multi-threaded hosts |
| **Fast Path** | Specialized optimizations available |

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

