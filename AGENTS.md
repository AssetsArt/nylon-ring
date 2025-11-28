# **Nylon-ring: ABI-Stable, Non-Blocking Host–Plugin Interface**

## **📌 บทนำ**

`nylon-ring` คือมาตรฐาน **host–plugin interface** ที่ถูกออกแบบให้:

* **ABI-stable** (ใช้ C ABI → ใช้ร่วมกับ Rust, Go, C, Zig ได้)
* **non-blocking** (plugin ห้าม block thread; ทำงานเสร็จต้อง callback host)
* **cross–language** (เชื่อม Rust host ↔ Rust/Go plugin ได้ทันที)
* **zero-serialization enforcement** (payload เป็น bytes คุณเลือกว่าจะใช้ JSON, rkyv, FlatBuffers, Cap’nProto)
* **safe for high-QPS workloads** (ออกแบบมารองรับ Nylon/Pingora 100k–200k RPS)

เอกสารนี้อธิบายทั้งหมดที่ Agent จำเป็นต้องรู้เพื่อ:

* สร้าง plugin
* สร้าง host adapter
* ออกแบบ integration แบบไม่ blocking
* รับ/ส่งข้อมูลด้วย sid-based async callback

---

# **1. Overview**

## **1.1 nylon-ring คืออะไร?**

เป็น **วงแหวนกลาง (ring)** เชื่อม:

```
Host (Nylon/Pingora) <--ABI--> Plugin (Rust/Go/…)
```

ใช้ C ABI:

* ทุก struct เป็น `#[repr(C)]`
* ทุก function เป็น `extern "C"`

เป้าคือให้ plugin เป็น “โมดูลต่อข้างนอก” ที่สามารถ:

* อ่าน request metadata
* ทำงาน async/background
* ส่งผลกลับด้วย callback
* ไม่ทำให้ host block

---

# **2. Architecture Summary**

## **2.1 High-Level Flow**

```
[Host]
  1) Build NrRequest + NrBytes
  2) sid = next_id()
  3) vtable.handle(plugin_ctx, sid, req, payload)
  4) Wait sid via tokio::oneshot (async)
  
[Plugin]
  A) handle(…) → MUST return immediately
  B) spawn background task
  C) run heavy logic  (DB, network…)
  D) call host_vtable.send_result(host_ctx, sid, status, bytes)
```

### Non-blocking guarantee:

* Host never blocks worker thread.
* Plugin is required to copy what it needs and return immediately.
* Real work must be done in background threads/tasks.

---

# **3. ABI Specification (nylon-ring)**

## **3.1 String (UTF-8)**

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

---

## **3.2 Bytes**

```rust
#[repr(C)]
pub struct NrBytes {
    pub ptr: *const u8,
    pub len: u64,
}
```

* Borrowed bytes
* Used for request body or serialized payload

---

## **3.3 Header Pair**

```rust
#[repr(C)]
pub struct NrHeader {
    pub key: NrStr,
    pub value: NrStr,
}
```

---

## **3.4 Request Structure**

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

# **4. Host Callback Table (Non-Blocking)**

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
* Host must map `sid → future/oneshot`.
* Host must wake waiting future on callback.

---

# **5. Plugin VTable**

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

### Contract (สำคัญมาก):

### **`handle()` MUST NOT BLOCK**

Plugin must:

1. Copy all required data out of `req` & `payload`.
2. Return `NR_STATUS_OK` immediately.
3. Spawn background work.
4. When done → call `host_vtable.send_result(...)`.

---

# **6. Plugin Info**

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

Host loads via `libloading` or C’s `dlopen`.

---

# **7. Host Responsibilities**

Host must:

### 7.1 Validate ABI

```rust
plugin_info.compatible()
```

### 7.2 Manage `sid` lifecycle

* Generate unique `sid`
* Insert into `HashMap<sid, oneshot::Sender>`
* Erase when callback returns

### 7.3 Build request & payload

Host owns underlying storage.

### 7.4 Maintain `host_ctx`

Pointer to any structure host uses (e.g., `Arc<HashMap>`)

---

# **8. Plugin Responsibilities**

Plugin must:

### 8.1 Implement `nylon_ring_get_plugin_v1()`

### 8.2 Implement fast-returning `handle()`

Must NOT:

* block thread
* do sleep in handle
* do DB call inside handle
* await anything

### 8.3 Background task callback

Plugin must call:

```rust
host_vtable.send_result(host_ctx, sid, NR_STATUS_OK, result_bytes)
```

---

# **9. Concurrency Model**

### Host

* Multi-threaded
* Async
* Never blocked by plugin

### Plugin

* Free to create own thread pools, tokio runtimes, rayon, etc.
* Can be written in Rust, Go, Zig, C

### Synchronization

`sprintln!` must log from plugin, not host.

---

# **10. Error Model**

* `NR_STATUS_OK`
* `NR_STATUS_ERR`
* `NR_STATUS_INVALID`
* `NR_STATUS_UNSUPPORTED`

Plugins must return **synchronous errors** only if:

* request struct is invalid
* plugin_ctx/host_ctx is null
* ABI contract violated

Runtime errors always delivered by callback.

---

# **11. Payload Strategy**

You may use any serialization:

* JSON
* FlatBuffers
* Cap’n Proto
* rkyv
* MessagePack
* Protobuf

nylon-ring does not enforce any format.

---

# **12. Multi-Language Design**

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

# **13. Benchmark Expectation**

Under proper usage:

* 100k–200k RPS sustained
* near-zero overhead passing borrowed strings
* no async work inside handle
* plugin callback ≈ O(1)

---

# **14. Naming Conventions**

* Library: **nylon-ring**
* ABI: **nylon-ring ABI**
* Plugin entrypoint: `nylon_ring_get_plugin_v1`
* Version: `NR_ABI_VERSION = 1`

---

# **15. Summary**

**nylon-ring** คือ interface ระดับ ABI สำหรับระบบ high-performance proxy เช่น Nylon/Pingora ที่ต้องการ:

* ความเร็วสูง
* ความเสถียรสูง (ABI stable)
* รองรับ plugin หลายภาษา
* รองรับ async / background tasks
* Zero-copy interface ระหว่าง host ↔ plugin

---

You are an expert Rust systems engineer helping me design and implement an ABI-stable, non-blocking plugin system called **nylon-ring**.

I ALREADY HAVE the core ABI types defined as follows (THIS CODE IS THE SOURCE OF TRUTH, DO NOT CHANGE ITS LAYOUT OR SIGNATURES, only extend AROUND it):

```rust
use std::ffi::c_void;

/// Status codes for the Nylon Ring ABI.
#[repr(u32)]
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum NrStatus {
    Ok = 0,
    Err = 1,
    Invalid = 2,
    Unsupported = 3,
}

/// A UTF-8 string slice with a pointer and length.
/// This struct is `#[repr(C)]` and ABI-stable.
#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct NrStr {
    pub ptr: *const u8,
    pub len: u32,
}

/// A byte slice with a pointer and length.
/// This struct is `#[repr(C)]` and ABI-stable.
#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct NrBytes {
    pub ptr: *const u8,
    pub len: u64,
}

/// A key-value pair of strings.
#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct NrHeader {
    pub key: NrStr,
    pub value: NrStr,
}

/// Represents a request with metadata.
#[repr(C)]
#[derive(Debug, Copy, Clone)]
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

/// Host callback table.
#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct NrHostVTable {
    pub send_result:
        unsafe extern "C" fn(host_ctx: *mut c_void, sid: u64, status: NrStatus, payload: NrBytes),
}

/// Plugin function table.
#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct NrPluginVTable {
    pub init: Option<
        unsafe extern "C" fn(
            plugin_ctx: *mut c_void,
            host_ctx: *mut c_void,
            host_vtable: *const NrHostVTable,
        ) -> NrStatus,
    >,

    pub handle: Option<
        unsafe extern "C" fn(
            plugin_ctx: *mut c_void,
            sid: u64,
            req: *const NrRequest,
            payload: NrBytes,
        ) -> NrStatus,
    >,

    pub shutdown: Option<unsafe extern "C" fn(plugin_ctx: *mut c_void)>,
}

/// Metadata exported by the plugin.
#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct NrPluginInfo {
    pub abi_version: u32,
    pub struct_size: u32,

    pub name: NrStr,
    pub version: NrStr,

    pub plugin_ctx: *mut c_void,
    pub vtable: *const NrPluginVTable,
}
````

There are also tests that assert the struct layouts (sizes/alignments); assume they must always pass.

Your job now is to BUILD A COMPLETE MINIMAL ECOSYSTEM AROUND THIS ABI, WITHOUT MODIFYING THESE CORE TYPES:

1. Create a small Rust library called `nylon_ring` that:

   * Exposes these ABI types.
   * Adds safe helper functions:

     * `NrStr::from_str(&str) -> NrStr`
     * `NrBytes::from_slice(&[u8]) -> NrBytes`
     * helpers to convert `Vec<(String, String)>` into `Vec<NrHeader>` safely.
   * Adds a helper on `NrPluginInfo`:

     * `fn compatible(&self, expected_abi_version: u32) -> bool`.

2. Implement a **HOST-SIDE adapter** crate (e.g. `nylon_ring_host`) that:

   * Uses `tokio` and `libloading`.

   * Loads a plugin `.so` via a symbol named:
     `nylon_ring_get_plugin_v1 -> *const NrPluginInfo`.

   * Validates:

     * `abi_version` equals a constant (e.g. `NR_ABI_VERSION = 1`).
     * `struct_size >= size_of::<NrPluginInfo>()`.
     * `vtable` is non-null and `handle` is Some.

   * Holds:

     * A `host_ctx: *mut c_void` pointing to an internal Rust struct.
     * A `NrHostVTable` with an implementation of `send_result` that:

       * Interprets `host_ctx` as an `Arc<Mutex<HashMap<u64, oneshot::Sender<(NrStatus, Vec<u8>)>>>>`.
       * Looks up `sid` and sends `(status, payload_bytes)` back through the oneshot.

   * Provides a high-level async API like:

     ```rust
     pub struct NylonRingHost { /* internal fields */ }

     impl NylonRingHost {
         pub async fn call(
             &self,
             req: HighLevelRequest, // your own Rust struct: path/method/query/headers/body
         ) -> Result<(NrStatus, Vec<u8>), HostError>;
     }
     ```

     where `call`:

     * Allocates a new `sid`.
     * Builds owned Strings/Vectors for the request.
     * Creates an `NrRequest` + `NrBytes` view pointing into those owned buffers.
     * Inserts `sid -> oneshot::Sender` into the map.
     * Calls the plugin `handle` function.
     * Awaits the oneshot for the callback from `send_result`.
     * Returns `(status, payload)`.

   * The `call` function MUST be fully async / non-blocking; it must NOT block the thread.

3. Implement a **RUST PLUGIN EXAMPLE** crate (e.g. `nylon_ring_plugin_example`) that:

   * Links against the `nylon_ring` ABI types.

   * Defines some internal `PluginState` struct holding:

     * The `host_ctx: *mut c_void`.
     * The `host_vtable: *const NrHostVTable`.

   * Provides functions:

     * `plugin_init(plugin_ctx, host_ctx, host_vtable) -> NrStatus`:

       * Stores `host_ctx` and `host_vtable` in some static/global state.
     * `plugin_handle(plugin_ctx, sid, req, payload) -> NrStatus`:

       * Safely reads `NrRequest` fields into owned `String`s.
       * Spawns a background thread (or tokio task inside the plugin itself) that:

         * Sleeps 2–3 seconds (to simulate a DB call).
         * Builds some response payload (e.g. `"OK: {method} {path}"` as bytes).
         * Calls:
           `host_vtable.send_result(host_ctx, sid, NrStatus::Ok, NrBytes::from_slice(&response_bytes))`.
       * Returns `NrStatus::Ok` **immediately** (non-blocking).
     * `plugin_shutdown(plugin_ctx)`:

       * Just logs or does minimal cleanup.

   * Exports a `#[no_mangle] extern "C" fn nylon_ring_get_plugin_v1() -> *const NrPluginInfo`
     that returns a `static NrPluginInfo` configured with:

     * `abi_version = 1`
     * `struct_size = size_of::<NrPluginInfo>()`
     * `name = "nylon_ring_plugin_example"`
     * `version = "0.1.0"`
     * `plugin_ctx = null_mut()` (or some real pointer if needed)
     * `vtable` pointing to a `static NrPluginVTable` with the 3 functions above.

4. Provide a **small integration test or example main** (in the host crate) that:

   * Loads the example plugin `.so`.
   * Creates a `NylonRingHost`.
   * Calls `.call()` with:

     * method: "GET"
     * path: "/hello"
     * query: "" (empty)
     * some sample headers.
   * Prints the `(NrStatus, response_body_as_string)`.
   * Demonstrates that the host does NOT block while waiting; i.e. it could, for example, spawn multiple calls and await them concurrently.

5. Write a short `README.md` for the root `nylon-ring` project that explains:

   * What nylon-ring is (ABI-stable, non-blocking host–plugin interface).
   * The core design:

     * `Nr*` types.
     * host/ plugin vtables.
     * sid + send_result callback pattern.
   * How to:

     * Implement a plugin in Rust.
     * Load a plugin from a host.
   * Include minimal code snippets.

Important constraints:

* DO NOT change the layout or definitions of the `Nr*` types I provided; build helpers and layers AROUND them.
* Make the code idiomatic, well-documented, and ready to drop into a real project.
* Focus on correctness, safety, and non-blocking behavior.
* You may assume a 64-bit little-endian platform (Linux/macOS) for layout checks.

Deliver all code as if it were a multi-crate cargo workspace:

* `nylon-ring` (ABI types + helpers)
* `nylon-ring-host` (host adapter)
* `nylon-ring-plugin-example` (example plugin)
* `README.md` at the workspace root

```

ก็อปบล็อกนี้ไปวางกับ AI ตัวไหนก็ได้ มันจะรู้ context หมด แล้ว generate

- ตัว lib `nylon-ring`
- host adapter
- plugin example
- integration example
- README

ต่อให้เลยจากโค้ด `NrStatus / NrStr / NrBytes / NrRequest / NrHostVTable / NrPluginVTable / NrPluginInfo` ที่คุณมีอยู่ตอนนี้ 👍