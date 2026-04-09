# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project

Nylon Ring — an ABI-stable, high-performance host↔plugin interface for Rust (and other C-ABI languages). Two crates plus example host/plugin binaries.

## Workspace layout

- `crates/nylon-ring/` — Core ABI types (`#[repr(C)]`): `NrStr`, `NrBytes`, `NrKV`, `NrVec<T>`, `NrStatus`, `NrHostVTable`, `NrPluginVTable`. Also exports the `define_plugin!` macro that generates the `nylon_ring_get_plugin_v1()` entry point and panic-safe FFI wrappers.
- `crates/nylon-ring-host/` — Host runtime: `NylonRingHost` container, `LoadedPlugin`/`PluginHandle`, streaming (`StreamFrame`, `StreamReceiver`), shared host context.
- `examples/ex-nyring-plugin/` — Example cdylib plugin.
- `examples/ex-nyring-host/` — Example host binary + multi-thread stress test.

## Common commands

```bash
cargo build --release
cargo run --release --bin ex-nyring-host        # demo / stress test
cargo bench                                      # all benches
cargo bench --package nylon-ring                 # ABI type benches
cargo bench --package nylon-ring-host            # host overhead benches
cargo test -p nylon-ring-host <name>             # run a single test
```

Benches use Criterion; HTML reports land in `target/criterion/`. The example host loads the example plugin's release `.so`/`.dylib` from `target/release/`, so build release first.

## Architecture (the parts that span multiple files)

The host↔plugin boundary is a strict C ABI. Understanding the request lifecycle requires reading both crates together:

1. **SID generation (host).** Each call gets a Stream ID from a thread-local counter that allocates in 1M-id blocks — no global atomics on the hot path. See the ID generator in `nylon-ring-host`.
2. **Pending-request tracking uses a waterfall.** On the fast/sync path the host stores the result slot in **Thread-Local Storage**; on the standard async path it uses a **64-shard DashMap**. When the plugin calls back via `send_result`, the host's callback router checks TLS first, then the sharded map. This is why there are three call variants:
   - `call` — fire-and-forget (no slot allocated)
   - `call_response_fast` — TLS slot, must complete on same thread
   - `call_response` — sharded-map slot, any thread
   - `call_stream` — returns `StreamReceiver`; frames flow until `NrStatus::StreamEnd` or `Err`
3. **Zero-copy transfer.** `NrVec<T>` transfers heap ownership across the FFI boundary without copying. `Clone` and `NrStr::push_str` are implemented manually to keep allocator ownership ABI-safe — be careful editing them; recent commits (`f911308`) fixed exactly this.
4. **Plugin side.** Plugins keep `HOST_CTX` and `HOST_VTABLE` as globals set in `init`, and call `(*HOST_VTABLE).send_result(ctx, sid, status, nr_vec)` from any handler. The `define_plugin!` macro wraps each handler in panic-catching FFI shims and routes by entry name.

When changing ABI types, the layout must stay `#[repr(C)]` and stable across the host/plugin boundary — both crates compile against the same definitions, but a real plugin built separately will not, so treat field order/size as load-bearing.
