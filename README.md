<div align="center">

# 🔗 Nylon Ring

**Ultra-Fast ABI-Stable Host–Plugin Interface for Rust**

[![CI](https://github.com/AssetsArt/nylon-ring/actions/workflows/ci.yml/badge.svg)](https://github.com/AssetsArt/nylon-ring/actions/workflows/ci.yml)
[![Crates.io](https://img.shields.io/crates/v/nylon-ring.svg)](https://crates.io/crates/nylon-ring)
[![Documentation](https://docs.rs/nylon-ring/badge.svg)](https://docs.rs/nylon-ring)
[![License](https://img.shields.io/badge/License-MIT-green.svg)](LICENSE)

*Build fast native plugins with an explicit C-compatible ABI.*

[Features](#-features) • [Quick Start](#-quick-start) • [Performance](#-performance) • [System Overview](#-system-overview)

</div>

---

## 🌟 Features

- **Stable ABI v1** — C-compatible types, fixed status values, version and
  structure-size validation.
- **Flexible calls** — fire-and-forget, async unary, synchronous fast path, and
  bounded streaming with backpressure.
- **Safe lifecycle** — in-flight call guards, graceful unload/reload, timeouts,
  and automatic request cleanup.
- **Cross-language validation** — Rust examples plus a buildable
  [C plugin](https://github.com/AssetsArt/nylon-ring/tree/main/examples/c-plugin).

The workspace provides:

- [`nylon-ring`](https://crates.io/crates/nylon-ring) — ABI types, vtables, and
  the `define_plugin!` macro.
- [`nylon-ring-host`](https://crates.io/crates/nylon-ring-host) — dynamic loading,
  routing, streaming, lifecycle controls, and metrics.

MSRV: Rust 1.88.

---

## 🚀 Quick start

### Install

```toml
# Plugin
[dependencies]
nylon-ring = "0.1.2"

# Host
[dependencies]
nylon-ring-host = "0.1.2"
```

Plugin crates must also build as a dynamic library:

```toml
[lib]
crate-type = ["cdylib"]
```

### Host

```rust,no_run
use nylon_ring_host::{NrStatus, NylonRingHost};

# async fn run() -> Result<(), Box<dyn std::error::Error>> {
let mut host = NylonRingHost::new();
host.load("example", "target/release/libexample_plugin.so")?;

let plugin = host.plugin("example").expect("plugin was loaded");
let (status, response) = plugin.call_response("echo", b"hello").await?;

assert_eq!(status, NrStatus::Ok);
assert_eq!(response, b"hello");
# Ok(())
# }
```

Other call patterns:

```rust,ignore
plugin.call("notify", b"payload").await?;
plugin.call_response_fast("echo", b"hello").await?;
plugin.call_response_timeout("echo", b"hello", timeout).await?;
let (sid, stream) = plugin.call_stream("events", b"start").await?;
```

See the complete [Rust plugin example](https://github.com/AssetsArt/nylon-ring/tree/main/examples/ex-nyring-plugin)
and [Rust host example](https://github.com/AssetsArt/nylon-ring/tree/main/examples/ex-nyring-host).

Run the workspace demo:

```bash
cargo run --release --package ex-nyring-host
```

---

## 📊 Performance

Release-build snapshot on an Apple M1 Pro.

### Single-stream (Criterion)

| Host operation | Time | Throughput |
|---|---:|---:|
| Fire-and-forget | 40.610 ns | 24.625M calls/s |
| Synchronous fast path | 59.121 ns | 16.914M calls/s |
| Standard unary | 95.797 ns | 10.439M calls/s |
| Unary + 128-byte payload | 110.07 ns | 9.085M calls/s |
| Unary + 1 KiB payload | 172.30 ns | 5.804M calls/s |
| Unary + 4 KiB payload | 232.85 ns | 4.295M calls/s |

### Multi-core (10 workers)

| Host operation | Throughput |
|---|---:|
| Fire-and-forget | 126.01M calls/s |
| Synchronous fast path | 99.18M calls/s |
| Standard unary | 25.34M calls/s |

These are reference measurements, not cross-platform guarantees. Single-stream
uses Criterion estimates; multi-core is a direct-terminal run with 100 requests
per batch for 10 seconds. Reproduce them with:

```bash
cargo bench --package nylon-ring-host
cargo build --release --package ex-nyring-host --package ex-nyring-plugin
NYRING_BENCH_WORKERS=10 ./target/release/ex-nyring-host
```

---

## 🏗 System overview

```text
+------------------------------------------------------+
| Host (nylon-ring-host)                               |
|  NylonRingHost                                       |
|   ├─ LoadedPlugin + in-flight call gate              |
|   └─ HostContext                                     |
|       ├─ thread-local synchronous slot               |
|       ├─ sharded unary router + inline completion    |
|       └─ bounded stream queues                       |
+-------------------------+----------------------------+
                          | C ABI v1
                          | NrPluginVTable / NrHostVTable
+-------------------------+----------------------------+
| Plugin                                               |
|  define_plugin! → named handlers → send_result       |
+------------------------------------------------------+
```

The host validates and initializes a library, assigns each call a session ID,
then routes plugin callbacks to the matching unary request or stream. Call guards
keep the library loaded until active work finishes; graceful unload/reload stops
new calls and waits for existing calls to drain.

---

## 🛡 Safety

- Host and plugin must use the same target and ABI version.
- `NrStr` and `NrBytes` are borrowed; copy them before retaining their data.
- Owned `NrVec<T>` values carry the producer's drop callback, avoiding
  cross-allocator frees.
- Plugins must stop worker threads and callbacks during shutdown.
- Only load trusted native libraries; this is not a security sandbox.

---

## Development

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-targets --all-features
```

Publishing is handled by GitHub Actions in dependency order using the
organization-level `RUST_TOKEN` secret.

## License

[MIT](LICENSE)
