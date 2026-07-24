<div align="center">

# 🔗 Nylon Ring

**Fast, explicit host–plugin ABI for Rust**

[![CI](https://github.com/AssetsArt/nylon-ring/actions/workflows/ci.yml/badge.svg)](https://github.com/AssetsArt/nylon-ring/actions/workflows/ci.yml)
[![Crates.io](https://img.shields.io/crates/v/nylon-ring.svg)](https://crates.io/crates/nylon-ring)
[![Documentation](https://docs.rs/nylon-ring/badge.svg)](https://docs.rs/nylon-ring)
[![Rust](https://img.shields.io/badge/MSRV-1.88-000000?logo=rust)](https://www.rust-lang.org/)
[![License](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)

*Build native plugins with a small C-compatible boundary, async routing, and safe lifecycle controls.*

[Features](#-features) • [Quick Start](#-quick-start) • [Usage](#-usage) • [Performance](#-performance) • [System Overview](#-system-overview)

</div>

---

## 🌟 Features

### 🔒 Explicit ABI boundary

- C-compatible wire types and vtables with stable discriminants.
- ABI version and plugin-structure-size validation during loading.
- Panic containment at generated Rust plugin entry points.
- A documented [ABI evolution policy](https://github.com/AssetsArt/nylon-ring/blob/main/docs/ABI_EVOLUTION.md).
- A buildable [reference C plugin](https://github.com/AssetsArt/nylon-ring/tree/main/examples/c-plugin).

### ⚡ Flexible call patterns

- Fire-and-forget calls.
- Async unary request/response calls.
- A synchronous thread-local fast path.
- Bounded bidirectional streams with explicit backpressure.
- Optional response timeouts with automatic pending-request cleanup.

### 🔧 Production lifecycle controls

- Multiple named plugins in one host.
- Unload and reload gates that reject new calls while draining active calls.
- Grace-period variants for controlled shutdown and hot reload.
- Metrics for loaded plugins, pending requests, state sessions, and in-flight calls.
- Lazily allocated request-routing shards to keep an idle host small.

### 🧪 Release-focused validation

- Workspace tests for lifecycle, callback routing, TLS re-entry, cancellation, and C ABI loading.
- Loom models for concurrency-sensitive state transitions.
- Miri coverage for the core ABI types.
- MSRV, formatting, Clippy, docs, packaging, and C compilation in CI.

---

## 📦 Project structure

```text
nylon-ring/
├── crates/
│   ├── nylon-ring/                 # Core ABI types, vtables, and macros
│   │   ├── src/
│   │   └── benches/
│   └── nylon-ring-host/            # Dynamic loading and request routing
│       ├── src/
│       └── benches/
├── docs/
│   └── ABI_EVOLUTION.md            # Compatibility and versioning policy
└── examples/
    ├── ex-nyring-plugin/           # Rust cdylib plugin
    ├── ex-nyring-host/             # Rust host and benchmark driver
    └── c-plugin/                    # C header and reference plugin
```

The workspace publishes two crates:

- [`nylon-ring`](https://crates.io/crates/nylon-ring) contains only the ABI
  types, vtables, and `define_plugin!` macro.
- [`nylon-ring-host`](https://crates.io/crates/nylon-ring-host) loads dynamic
  libraries and provides the higher-level host API.

Version `0.1.0` implements ABI version 1. The minimum supported Rust version is
Rust 1.88.

---

## 🚀 Quick start

### Install

Plugin crates need the ABI crate:

```toml
[lib]
crate-type = ["cdylib"]

[dependencies]
nylon-ring = "0.1.0"
```

Host applications need the host crate:

```toml
[dependencies]
nylon-ring-host = "0.1.0"
```

### Build and run the workspace example

```bash
cargo build --release
cargo run --release --package ex-nyring-host
```

The host example builds and loads `ex-nyring-plugin`, then exercises the
supported call patterns and optional benchmarks.

### Run benchmarks

```bash
cargo bench --package nylon-ring
cargo bench --package nylon-ring-host
```

Benchmark results depend on the machine, target, allocator, plugin behavior,
and payload. The repository intentionally reports measurements from the machine
running Criterion instead of presenting fixed throughput claims.

---

## 📊 Performance

The following snapshot was measured with Criterion in a release build on an
Apple M1 Pro (10 cores). Values are the center estimates from one local run;
they are a reference point, not a cross-platform guarantee.

### ABI types

| Operation | Time | Notes |
|---|---:|---|
| `NrStr::new` | 0.77 ns | Create a borrowed string view |
| `NrStr::as_str` | 6.79 ns | Validate and read UTF-8 |
| `NrBytes::from_slice` | 0.48 ns | Create a borrowed byte view |
| `NrBytes::as_slice` | 0.32 ns | Read a validated byte view |
| `NrKV::new` | 2.67 ns | Create a key-value pair |
| `NrVec::from_vec` | 21.96 ns | Wrap an owned vector with its drop callback |
| `NrVec::into_vec` | 31.66 ns | Copy into the receiver allocator and release the source |
| `NrVec::as_slice` | 0.32 ns | Read a vector view |

### Host round trips

| Operation | Time | Throughput | Notes |
|---|---:|---:|---|
| Fire-and-forget | 50.98 ns | 19.62M calls/s | No response registration |
| Synchronous fast path | 76.63 ns | 13.05M calls/s | Thread-local response slot |
| Standard unary | 162.06 ns | 6.17M calls/s | Async request/response routing |
| Unary + 128-byte payload | 196.40 ns | 5.09M calls/s | Payload round trip |
| Unary + 1 KiB payload | 242.63 ns | 4.12M calls/s | Payload round trip |
| Unary + 4 KiB payload | 321.19 ns | 3.11M calls/s | Payload round trip |

The allocator-safe `NrVec::into_vec` path deliberately copies into the receiving
module's allocator before invoking the producer's drop callback. This trades the
old cross-allocator zero-copy claim for defined ownership behavior at the ABI
boundary.

To reproduce the snapshot, run the benchmark commands above and open the HTML
reports under `target/criterion/`.

---

## 💻 Usage

### Host: plugin management

```rust,no_run
use nylon_ring_host::NylonRingHost;
use std::time::Duration;

# async fn run() -> Result<(), Box<dyn std::error::Error>> {
let mut host = NylonRingHost::with_stream_capacity(128);

host.load("plugin_a", "plugins/libplugin_a.so")?;
host.load("plugin_b", "plugins/libplugin_b.so")?;

let plugin_a = host.plugin("plugin_a").expect("plugin_a was just loaded");

host.unload_with_grace("plugin_b", Duration::from_secs(5)).await?;
host.reload_with_grace(Duration::from_secs(5)).await?;

let metrics = host.metrics();
println!("active calls: {}", metrics.in_flight_calls);
# drop(plugin_a);
# Ok(())
# }
```

Use the dynamic-library suffix for the target platform:

- Linux: `.so`
- macOS: `.dylib`
- Windows: `.dll`

### Host: fire-and-forget

```rust,ignore
let status = plugin.call("notify", b"payload").await?;
```

### Host: unary response

```rust,ignore
use nylon_ring::NrStatus;

let (status, response) = plugin.call_response("echo", b"hello").await?;
assert_eq!(status, NrStatus::Ok);
assert_eq!(response, b"hello");
```

For a bounded wait, use `call_response_timeout`:

```rust,ignore
use std::time::Duration;

let result = plugin
    .call_response_timeout("echo", b"hello", Duration::from_secs(2))
    .await?;
```

### Host: synchronous fast path

```rust,ignore
let (status, response) = plugin.call_response_fast("echo", b"hello").await?;
```

The fast path is for handlers that invoke `send_result` synchronously on the
same thread. Re-entry while a fast slot is occupied is rejected instead of
aliasing the response slot.

### Host: streaming

```rust,ignore
use nylon_ring::NrStatus;

let (sid, mut stream) = plugin.call_stream("events", b"start").await?;
plugin.send_stream_data(sid, b"next")?;

while let Some(frame) = stream.recv().await {
    println!("{} bytes", frame.data.len());
    if frame.status.is_terminal() {
        break;
    }
}

plugin.close_stream(sid)?;
```

Streams use a bounded queue. When it is full, the host callback returns
`NrStatus::Backpressure` so the plugin can retry, coalesce, or drop work.
Dropping a `StreamReceiver` unregisters its pending request.

### Plugin: implement handlers

```rust
use nylon_ring::{define_plugin, NrBytes, NrHostVTable, NrStatus, NrVec};
use std::ffi::c_void;
use std::sync::atomic::{AtomicPtr, Ordering};

static HOST_CTX: AtomicPtr<c_void> = AtomicPtr::new(std::ptr::null_mut());
static HOST_VTABLE: AtomicPtr<NrHostVTable> = AtomicPtr::new(std::ptr::null_mut());

unsafe fn init(ctx: *mut c_void, vtable: *const NrHostVTable) -> NrStatus {
    if ctx.is_null() || vtable.is_null() {
        return NrStatus::Invalid;
    }

    HOST_CTX.store(ctx, Ordering::Release);
    HOST_VTABLE.store(vtable.cast_mut(), Ordering::Release);
    NrStatus::Ok
}

unsafe fn echo(sid: u64, payload: NrBytes) -> NrStatus {
    let bytes = match unsafe { payload.as_slice() } {
        Ok(bytes) => bytes,
        Err(_) => return NrStatus::Invalid,
    };

    let ctx = HOST_CTX.load(Ordering::Acquire);
    let vtable = HOST_VTABLE.load(Ordering::Acquire);
    if ctx.is_null() || vtable.is_null() {
        return NrStatus::Err;
    }

    let response = NrVec::from_vec(bytes.to_vec());
    let send_result = unsafe { (*vtable).send_result };
    unsafe { send_result(ctx, sid, NrStatus::Ok, response) }
}

fn shutdown() {
    HOST_VTABLE.store(std::ptr::null_mut(), Ordering::Release);
    HOST_CTX.store(std::ptr::null_mut(), Ordering::Release);
}

define_plugin! {
    init: init,
    shutdown: shutdown,
    entries: {
        "echo" => echo,
    }
}
```

The `define_plugin!` macro:

- Exports `nylon_ring_get_plugin_v1`.
- Publishes package name and version metadata.
- Routes calls by UTF-8 entry name.
- Converts unwinding panics into `NrStatus::Panic` before they cross the FFI
  boundary.

---

## 🏗 System overview

```text
+----------------------------------------------------------------+
|                    Host (nylon-ring-host)                       |
|                                                                |
|  NylonRingHost                                                 |
|    ├── LoadedPlugin A ── call gate ── dynamic library guard     |
|    ├── LoadedPlugin B ── call gate ── dynamic library guard     |
|    └── Shared HostContext                                      |
|         ├── thread-local synchronous response slot              |
|         ├── lazy sharded pending-request maps                   |
|         ├── bounded stream queues                               |
|         └── session state                                      |
+----------------------------+-----------------------------------+
                             | C ABI v1
                             | NrPluginVTable / NrHostVTable
+----------------------------+-----------------------------------+
|                         Plugin                                 |
|                                                                |
|  define_plugin! entry point                                    |
|    ├── init / shutdown                                         |
|    ├── named handlers                                          |
|    └── send_result callback ────────────────────────────────┐   |
+------------------------------------------------------------|---+
                                                             |
                 callback router <────────────────────────────┘
```

1. The host loads `nylon_ring_get_plugin_v1` and validates the ABI version and
   `NrPluginInfo` size before calling `init`.
2. A `PluginHandle` creates a session ID and registers any expected response.
3. The plugin handles the named entry and returns data through
   `NrHostVTable::send_result`.
4. The callback router completes one unary request or advances one stream state
   atomically for that session ID.
5. Call guards keep the dynamic library alive until active handles and streams
   finish, even after the host starts unloading it.

### 1. Host layer (`nylon-ring-host`)

`NylonRingHost` owns the plugin registry and shared callback context. Each
loaded library has an acceptance gate and in-flight counter. Normal calls stop
being accepted as soon as unload or reload starts, while existing call guards
keep the old library mapped until their work finishes.

The callback router uses two paths:

- A thread-local slot for synchronous `call_response_fast` responses.
- Lazily initialized sharded maps for async unary and streaming responses.

Stream transitions happen while holding the relevant map entry, preventing the
old lookup/remove/reinsert race. Queues are bounded and report backpressure to
the plugin instead of growing without limit.

### 2. ABI layer (`nylon-ring`)

The ABI crate defines the wire types shared by both sides. Borrowed types expose
validated views, while owned vectors carry their producer's release callback.
`NrStatus` is a transparent integer wrapper with fixed ABI v1 values and a
reserved range for future additions.

### 3. Plugin layer

A plugin implements named handlers and uses `define_plugin!` to export the ABI
v1 entry point. Handlers can return immediately, send one response, or keep a
session ID and emit bounded stream frames later. The plugin remains responsible
for stopping its own workers during shutdown.

---

## Core types

### ABI crate (`nylon-ring`)

| Type | Purpose |
|---|---|
| `NrStatus` | Stable numeric status value with reserved future values |
| `NrStr` | Borrowed UTF-8 string view |
| `NrBytes` | Borrowed byte-slice view |
| `NrVec<T>` | Owned or borrowed vector representation with producer drop callback |
| `NrKV` / `NrMap` | ABI-compatible key-value data |
| `NrAny` | Typed erased payload with clone and drop callbacks |
| `NrHostVTable` | Callbacks exposed by the host |
| `NrPluginVTable` | Entry points exposed by a plugin |

### Host crate (`nylon-ring-host`)

| Type | Purpose |
|---|---|
| `NylonRingHost` | Plugin container, loader, lifecycle manager, and metrics source |
| `PluginHandle` | Cloneable call interface that keeps its plugin loaded |
| `StreamFrame` | Status and bytes returned for one stream frame |
| `StreamReceiver` | Bounded response receiver with cancellation cleanup |
| `HostMetrics` | Snapshot of host lifecycle and routing counts |

---

## 🎯 Use cases

### ✅ Good fit

- Trusted native plugin systems.
- Hot-reloadable service or application logic.
- High-throughput request routing within one process.
- Rust hosts with Rust or C-compatible plugins.
- Systems that need unary, synchronous, and streaming call patterns together.

### ⚠️ Consider alternatives

- Untrusted third-party code: use process or sandbox isolation.
- Cross-machine calls: use an RPC protocol.
- A boundary that must be portable across arbitrary targets and data models:
  use a serialized wire format.
- Plugins that cannot obey the ownership, threading, and shutdown contracts.

---

## 🔬 Design principles

| Principle | Implementation |
|---|---|
| Explicit compatibility | Versioned entry point, fixed status values, and size checks |
| Allocator ownership | Owned vectors carry the producer's drop callback |
| Bounded resource use | Bounded stream queues and configurable response timeouts |
| Race-resistant routing | Entry-locked pending transitions and terminal delivery once |
| Safe lifecycle | Call gates, library guards, and graceful drain operations |
| Observable operation | Host metrics and distinct lifecycle/timeout errors |
| Regression protection | Host tests, Loom models, Miri, C validation, and package checks |

---

## 🛡 Safety and compatibility

Nylon Ring makes native-plugin invariants explicit, but it cannot make an
arbitrary dynamic library safe. Plugin authors must uphold these rules:

- Host and plugin must target the same operating system, architecture, data
  model, and ABI version.
- `NrStr` and `NrBytes` are borrowed views. Copy their contents before retaining
  them after a callback returns. Their accessors are unsafe because a C ABI
  cannot express their Rust lifetimes.
- `NrVec<T>` carries explicit ownership and a producer-side drop callback. The
  receiver copies elements into its allocator before invoking that callback.
  Borrowed foreign buffers use `owned = 0`; owned buffers provide the matching
  `drop_fn`. `T` must have a compatible C layout.
- Plugin shutdown must stop worker threads and callbacks before the library can
  be released.
- Panic containment applies to unwinding panics. `panic = "abort"` still aborts
  the process.
- `#[repr(C)]` stabilizes field layout; it does not make arbitrary Rust types or
  standard-library internals portable across compiler versions.

Loading third-party native code is not a security boundary. Only load trusted
plugins.

---

## 🧰 Development

Run the same checks as CI:

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-targets --all-features
RUSTDOCFLAGS="-D warnings" cargo doc --workspace --all-features --no-deps
```

The core unsafe code can also be checked with Miri:

```bash
cargo +nightly miri test --package nylon-ring --lib
```

---

## 📤 Publishing

The crates are published in dependency order:

```bash
cargo publish --package nylon-ring
# Wait until the matching version is available from the crates.io index.
cargo publish --package nylon-ring-host
```

Publishing is automated by `.github/workflows/publish.yml`. Publishing a GitHub
Release whose tag matches the workspace version—for example, `v0.1.0`—deploys
both crates. The workflow can also be started manually and reads the crates.io
token from the organization-level `RUST_TOKEN` Actions secret.

---

## License

Licensed under the [MIT License](LICENSE).

## Acknowledgments

Built with [Tokio](https://tokio.rs/), [DashMap](https://github.com/xacrimon/dashmap),
[libloading](https://github.com/nagisa/rust_libloading), and
[Criterion.rs](https://bheisler.github.io/criterion.rs/book/).
