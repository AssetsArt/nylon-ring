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

- **Single stable ABI** — C-compatible types, fixed status values, version and
  structure-size validation; zero-copy responses, host-leased output buffers,
  and integer entry dispatch are all part of the one vtable.
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
nylon-ring = "0.2.0"

# Host
[dependencies]
nylon-ring-host = "0.2.0"
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

Reference snapshot on an Apple M1 Pro (8 performance + 2 efficiency cores),
release build. All host numbers come from the worker-loop harness in
`ex-nyring-host`: workers await each call sequentially (in-flight depth 1 per
worker) and every counted call is asserted `Ok`. Single-stream is the same
harness at one worker, so the two columns are directly comparable. Each value
is the best of at least three 10-second runs, all captured in one session
(2026-07-25, harness at `198518c`) — compare rows within this table, not
against older snapshots.

### Throughput

| Host operation | 1 worker | 10 workers | Scaling |
|---|---:|---:|---:|
| Fire-and-forget | 85.92M calls/s (11.6 ns) | 690.98M calls/s | 8.0× |
| Fire-and-forget (entry id) | 93.32M calls/s (10.7 ns) | 757.89M calls/s | 8.1× |
| Synchronous fast path | 40.51M calls/s (24.7 ns) | 319.99M calls/s | 7.9× |
| Synchronous fast path (entry id) | 58.40M calls/s (17.1 ns) | 462.01M calls/s | 7.9× |
| Standard unary | 17.07M calls/s (58.6 ns) | 132.15M calls/s | 7.7× |
| Streaming | 32.41M frames/s (30.9 ns) | 202.41M frames/s | 6.2× |

"Entry id" rows dispatch through a pre-resolved `PluginEntry`
(`handle.entry(name)` once, then id-only calls) instead of comparing the
entry name on every call. Streaming times full 9-frame round trips (8 data
frames + `StreamEnd`) through pooled per-stream channels.

### Unary payload curve (1 worker, time per call)

| Payload | `send_result` (copying) | `send_result_owned` | buffer lease |
|---|---:|---:|---:|
| empty | 58.6 ns | 57.1 ns | 64.2 ns |
| 128 B | 93.9 ns | 56.6 ns | 80.7 ns |
| 1 KiB | 153.3 ns | 56.2 ns | 108.4 ns |
| 4 KiB | 218.9 ns | 56.1 ns | 149.6 ns |

The owned column is flat: the plugin answers from its own long-lived buffer
and the host consumes it zero-copy through `call_response_bytes`. The lease
column pays one host alloc plus the plugin's serialize-in-place, and returns
a plain `Vec<u8>` through `call_response`.

Non-empty payloads pay two alloc/free pairs and two copies on the copying
`send_result` path: the response crosses the boundary as a foreign
allocation and is copied into host-owned memory because allocator
provenance cannot be proven across images. The `send_result_owned` and
buffer-lease paths avoid this; see `docs/ABI_EVOLUTION.md`.

These are reference measurements, not cross-platform guarantees; absolute
numbers shift with thermal and scheduling state (up to tens of percent
between sessions on the same machine), which is why every value above comes
from one session. Numbers published before 0.1.3 used per-iteration
`block_on` (single-stream) and a `join_all` batch loop (multi-core), and
multi-worker numbers published before `198518c` under-read by 15–22%
(the harness updated shared counters every batch); neither is comparable
with this table. Reproduce with:

```bash
cargo build --release --package ex-nyring-host --package ex-nyring-plugin
NYRING_BENCH_OPERATION=all NYRING_BENCH_WORKERS=10 ./target/release/ex-nyring-host
# NYRING_BENCH_OPERATION: fire | fireid | fast | fastid | unary | unaryid
#                         | owned | lease | stream | all
# Also: NYRING_BENCH_WORKERS, NYRING_BENCH_SECONDS, NYRING_BENCH_BATCH_SIZE,
#       NYRING_BENCH_PAYLOAD_BYTES, NYRING_BENCH_STREAM_FRAMES,
#       NYRING_BENCH_CPU_SAMPLES
cargo bench --package nylon-ring --bench abi_types   # ABI type microbenches
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
                          | C ABI
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
