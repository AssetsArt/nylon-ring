use nylon_ring::{define_plugin, NrBytes, NrHostVTable, NrStatus};
use std::ffi::c_void;

// Global state to store host context and vtable
static mut HOST_CTX: *mut c_void = std::ptr::null_mut();
static mut HOST_VTABLE: *const NrHostVTable = std::ptr::null();

// Initialize the plugin
unsafe fn init(host_ctx: *mut c_void, host_vtable: *const NrHostVTable) -> NrStatus {
    println!("[Plugin] Initialized!");
    HOST_CTX = host_ctx;
    HOST_VTABLE = host_vtable;
    NrStatus::Ok
}

// Shutdown the plugin
fn shutdown() {
    println!("[Plugin] Shutting down!");
}

// Echo handler - simply returns the input data
unsafe fn handle_echo(sid: u64, payload: NrBytes) -> NrStatus {
    let data = payload.as_slice();
    let text = String::from_utf8_lossy(data);
    println!("[Plugin] Echo received: {}", text);

    // Send response back to host
    if !HOST_VTABLE.is_null() && !HOST_CTX.is_null() {
        ((*HOST_VTABLE).send_result)(HOST_CTX, sid, NrStatus::Ok, payload);
    }

    NrStatus::Ok
}

// Uppercase handler - converts input to uppercase
unsafe fn handle_uppercase(sid: u64, payload: NrBytes) -> NrStatus {
    let data = payload.as_slice();
    let text = String::from_utf8_lossy(data).to_uppercase();
    println!("[Plugin] Uppercase received, sending back: {}", text);

    // Send response back to host
    if !HOST_VTABLE.is_null() && !HOST_CTX.is_null() {
        let response = text.as_bytes();
        let response_bytes = NrBytes::from_slice(response);
        ((*HOST_VTABLE).send_result)(HOST_CTX, sid, NrStatus::Ok, response_bytes);
    }

    NrStatus::Ok
}

// Define the plugin with its entry points
define_plugin! {
    init: init,
    shutdown: shutdown,
    entries: {
        "echo" => handle_echo,
        "uppercase" => handle_uppercase,
    }
}
