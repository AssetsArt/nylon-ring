# Nylon Ring: ABI-Stable, Non-Blocking Host–Plugin Interface

## Introduction

`nylon-ring` is a **host–plugin interface** standard designed for:

* **ABI-stable** (uses C ABI → works with Rust, Go, C, Zig)
* **Non-blocking** (plugin must not block thread; must callback host when done)
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
* Work async/background
* Send results via callback
* Never block the host

---

## 2. Architecture Summary

### 2.1 High-Level Flow

**Unary Call (Request/Response):**
```
[Host]
  1) Build NrRequest + NrBytes
  2) sid = next_id()
  3) vtable.handle(plugin_ctx, sid, req, payload)
  4) Wait sid via tokio::oneshot (async)
  
[Plugin]
  A) handle(…) → MUST return immediately
  B) spawn background task
  C) run heavy logic (DB, network…)
  D) call host_vtable.send_result(host_ctx, sid, status, bytes)
```

**Streaming Call (WebSocket-style):**
```
[Host]
  1) Build NrRequest + NrBytes
  2) sid = next_id()
  3) vtable.handle(plugin_ctx, sid, req, payload)
  4) Return StreamReceiver immediately
  
[Plugin]
  A) handle(…) → MUST return immediately
  B) spawn background task
  C) loop: send multiple frames via send_result(host_ctx, sid, Ok, frame_bytes)
  D) call send_result(host_ctx, sid, StreamEnd, empty) to close
```

### Non-blocking guarantee:

* Host never blocks worker thread.
* Plugin is required to copy what it needs and return immediately.
* Real work must be done in background threads/tasks.

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
        unsafe extern "C" fn(plugin_ctx, sid, req, payload) -> NrStatus
    >,

    pub shutdown: Option<
        unsafe extern "C" fn(plugin_ctx)
    >,
}
```

### Contract (CRITICAL):

### **`handle()` MUST NOT BLOCK**

Plugin must:

1. Copy all required data out of `req` & `payload`.
2. Return `NR_STATUS_OK` immediately.
3. Spawn background work.
4. When done → call `host_vtable.send_result(...)`.
   - For unary: call once with final status.
   - For streaming: call multiple times with `Ok`, then once with `StreamEnd`.

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
* For unary: Insert into `HashMap<sid, oneshot::Sender<(NrStatus, Vec<u8>)>>`
* For streaming: Insert into `HashMap<sid, mpsc::UnboundedSender<StreamFrame>>`
* Erase when callback returns (unary) or stream ends (streaming)

### 7.3 Build request & payload

Host owns underlying storage.

### 7.4 Maintain `host_ctx`

Pointer to any structure host uses (e.g., `Arc<Mutex<HashMap<...>>>`)

### 7.5 Support both unary and streaming

* `call()` → unary RPC (single response)
* `call_stream()` → streaming RPC (multiple frames)

---

## 8. Plugin Responsibilities

Plugin must:

### 8.1 Implement `nylon_ring_get_plugin_v1()`

### 8.2 Implement fast-returning `handle()`

Must NOT:

* block thread
* do sleep in handle
* do DB call inside handle
* await anything

### 8.3 Background task callback

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
* Async
* Never blocked by plugin

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

* using `cgo`
* plugin exports `extern "C"` functions
* uses same ABI structs

### Zig Plugin

* perfect C ABI support

### C / C++ Plugin

* direct match

---

## 13. Benchmark Expectation

Under proper usage:

* High throughput sustained
* near-zero overhead passing borrowed strings
* no async work inside handle
* plugin callback ≈ O(1)

---

## 14. Naming Conventions

* Library: **nylon-ring**
* ABI: **nylon-ring ABI**
* Plugin entrypoint: `nylon_ring_get_plugin_v1`
* Version: `NR_ABI_VERSION = 1`

---

## 15. Summary

**nylon-ring** is an ABI-level interface for high-performance proxy systems like Nylon/Pingora that require:

* High performance
* High stability (ABI stable)
* Multi-language plugin support
* Async / background task support
* Zero-copy interface between host ↔ plugin
* Both unary and streaming/WebSocket-style communication

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
* `NrPluginVTable` - Plugin function table
* `NrPluginInfo` - Plugin metadata

**CRITICAL**: DO NOT change the layout or definitions of these core types. Build helpers and layers AROUND them.

### Current Implementation

The workspace contains:

1. **`nylon-ring`** - Core ABI types + helper functions
2. **`nylon-ring-host`** - Host adapter with:
   - `NylonRingHost::load()` - Load plugin
   - `NylonRingHost::call()` - Unary RPC
   - `NylonRingHost::call_stream()` - Streaming RPC
3. **`nylon-ring-plugin-example`** - Example plugin supporting both unary and streaming

### Key Constraints

* All structs must remain `#[repr(C)]` and ABI-stable
* Plugin `handle()` must return immediately (non-blocking)
* Host uses `oneshot` for unary, `mpsc::UnboundedSender` for streaming
* Tests assert struct layouts (sizes/alignments) - must always pass
* Assume 64-bit little-endian platform (Linux/macOS)

### Testing

Run tests and examples:
```bash
make test          # Run all tests
make examples      # Run all examples
make example-simple      # Unary example
make example-streaming   # Streaming example
```
