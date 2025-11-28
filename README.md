# Nylon Ring

**Nylon Ring** is an ABI-stable, non-blocking host–plugin interface designed for high-performance systems. It allows plugins written in Rust (and potentially other languages like C, C++, Zig, Go) to communicate with a host application without blocking the host's execution threads.

## Core Design

The system relies on a few key concepts:

1.  **ABI Stability**: All data structures exchanged between host and plugin are `#[repr(C)]`.
2.  **Non-Blocking**: The plugin's `handle` function must return immediately. Actual work is done in the background.
3.  **Callback Mechanism**: The plugin reports results back to the host via a `send_result` callback, using a request ID (`sid`).

### Core Types (`nylon-ring` crate)

-   `NrStr` / `NrBytes`: ABI-stable string and byte slices.
-   `NrRequest`: Request metadata (method, path, headers).
-   `NrHostVTable`: Function pointers provided by the host (e.g., `send_result`).
-   `NrPluginVTable`: Function pointers provided by the plugin (`init`, `handle`, `shutdown`).

## Project Structure

This workspace contains:

-   `nylon-ring`: The core ABI library.
-   `nylon-ring-host`: A Rust host adapter using `tokio` and `libloading`.
-   `nylon-ring-plugin-example`: An example Rust plugin.

## Usage

### Implementing a Plugin

Create a `cdylib` crate and implement the required VTable functions:

```rust
use nylon_ring::{NrPluginInfo, NrPluginVTable, NrStatus, NrRequest, NrBytes};

extern "C" fn plugin_handle(
    _ctx: *mut c_void,
    sid: u64,
    req: *const NrRequest,
    _payload: NrBytes
) -> NrStatus {
    // 1. Read request (copy data if needed)
    // 2. Spawn background task
    // 3. Return NrStatus::Ok immediately
    NrStatus::Ok
}

// Export the plugin info
#[no_mangle]
pub extern "C" fn nylon_ring_get_plugin_v1() -> *const NrPluginInfo {
    &PLUGIN_INFO
}
```

### Loading a Plugin (Host)

Use `nylon-ring-host` to load and call the plugin:

```rust
use nylon_ring_host::{NylonRingHost, HighLevelRequest};

let host = NylonRingHost::load("path/to/plugin.so")?;

let req = HighLevelRequest {
    method: "GET".to_string(),
    path: "/api/data".to_string(),
    // ...
};

// Async call - does not block the thread
let (status, payload) = host.call(req).await?;
```

## Running the Example

1.  Build the plugin:
    ```bash
    cargo build
    ```

2.  Run the host example:
    ```bash
    cargo run --example simple_host
    ```
