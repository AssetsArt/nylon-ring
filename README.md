# Nylon Ring

Nylon Ring is a small Rust workspace for building native host-plugin systems
around an explicit C-compatible ABI. It provides:

- `nylon-ring`: ABI data types, vtables, and the `define_plugin!` macro.
- `nylon-ring-host`: dynamic-library loading, request/response routing,
  streaming responses, and per-session state callbacks.

[![CI](https://github.com/AssetsArt/nylon-ring/actions/workflows/ci.yml/badge.svg)](https://github.com/AssetsArt/nylon-ring/actions/workflows/ci.yml)
[![Crates.io](https://img.shields.io/crates/v/nylon-ring.svg)](https://crates.io/crates/nylon-ring)
[![Documentation](https://docs.rs/nylon-ring/badge.svg)](https://docs.rs/nylon-ring)
[![License](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)

## Status

Version `0.1.0` implements ABI version 1. The API is suitable for native Rust
hosts and plugins that are built for the same target and use compatible global
allocators. See [Safety and compatibility](#safety-and-compatibility) before
using it in a production plugin system.

The minimum supported Rust version is 1.88.

## Installation

Plugin crates need the ABI crate:

```toml
[dependencies]
nylon-ring = "0.1.0"
```

Host applications normally need the host crate, which depends on the ABI
crate:

```toml
[dependencies]
nylon-ring-host = "0.1.0"
```

## Plugin example

Build a plugin as a `cdylib`:

```toml
[lib]
crate-type = ["cdylib"]

[dependencies]
nylon-ring = "0.1.0"
```

Define its entry points with `define_plugin!`:

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
    unsafe { send_result(ctx, sid, NrStatus::Ok, response) };
    NrStatus::Ok
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

The macro exports `nylon_ring_get_plugin_v1`, publishes package name/version
metadata, validates entry names as UTF-8, and catches unwinding panics before
they cross the FFI boundary.

## Host example

```rust,no_run
use nylon_ring_host::{NylonRingHost, NrStatus};

# async fn run() -> Result<(), Box<dyn std::error::Error>> {
let mut host = NylonRingHost::new();
host.load("example", "target/release/libexample_plugin.so")?;

let plugin = host.plugin("example").expect("plugin was just loaded");
let (status, response) = plugin.call_response("echo", b"hello").await?;
assert_eq!(status, NrStatus::Ok);
assert_eq!(response, b"hello");
# Ok(())
# }
```

Choose the dynamic-library suffix for the target platform:

- Linux: `.so`
- macOS: `.dylib`
- Windows: `.dll`

`PluginHandle` also provides:

- `call` for fire-and-forget requests.
- `call_response_fast` for handlers that respond synchronously on the calling
  thread.
- `call_stream`, `send_stream_data`, and `close_stream` for bidirectional
  streams.

Dropping an in-progress response future or `StreamReceiver` unregisters its
pending request from the host.

## Running the workspace example

```bash
cargo run --release --package ex-nyring-host
```

The host example builds and loads `ex-nyring-plugin`, then exercises the call
patterns and optional benchmarks.

## Safety and compatibility

Nylon Ring makes unsafe native-plugin operations explicit, but it cannot make
arbitrary dynamic libraries safe. Plugin authors must uphold these rules:

- A host and plugin must target the same operating system, architecture, and
  ABI version.
- `NrStr` and `NrBytes` are borrowed views. A plugin must copy their contents
  before retaining them after a callback returns. Their accessors are unsafe
  because lifetimes cannot be represented in the C ABI.
- `NrVec<T>` transfers a Rust allocation between modules. Both modules must use
  compatible global allocators, and `T` must have a compatible layout. Do not
  construct an `NrVec` from memory allocated by an unrelated C allocator.
- Plugin `shutdown` must stop worker threads and callbacks before the dynamic
  library is unloaded.
- Panic containment requires unwinding panics. A crate built with
  `panic = "abort"` will still abort the process.
- `#[repr(C)]` stabilizes field layout; it does not make arbitrary Rust types
  or Rust standard-library internals portable across compiler versions. Keep
  the ABI boundary to the provided concrete wire types.

Loading third-party native code is not a security boundary. Only load trusted
plugins.

## Development

Run the same checks as CI:

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-targets --all-features
RUSTDOCFLAGS="-D warnings" cargo doc --workspace --all-features --no-deps
```

Benchmarks are intentionally reported from the machine that runs them rather
than as fixed project claims:

```bash
cargo bench --package nylon-ring
cargo bench --package nylon-ring-host
```

## Publishing

The crates must be published in dependency order:

```bash
cargo publish --package nylon-ring
# Wait until nylon-ring 0.1.0 is available from the crates.io index.
cargo publish --package nylon-ring-host
```

Use `cargo publish --dry-run` for each crate first. The initial host dry run can
only fully verify after the matching `nylon-ring` version exists on crates.io.

## License

Licensed under the [MIT License](LICENSE).
