use nylon_ring::{define_plugin, NrBytes, NrHostVTable, NrStatus, NrVec};
use std::ffi::c_void;

// Global state to store host context and vtable
static mut HOST_CTX: *mut c_void = std::ptr::null_mut();
static mut HOST_VTABLE: *const NrHostVTable = std::ptr::null();

#[inline(always)]
pub fn send_result(sid: u64, status: NrStatus, data: nylon_ring::NrVec<u8>) {
    unsafe {
        let f = (*HOST_VTABLE).send_result;
        f(HOST_CTX, sid, status, data);
    }
}

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
    let text_str = String::from_utf8_lossy(data);
    println!("[Plugin] Echo received: {}", text_str);

    // Modify the text
    let new_text = format!("{}, Nylon Ring!", text_str);

    // Convert to NrVec (Zero Copy transfer)
    let vec_bytes = new_text.into_bytes();
    let nr_vec = NrVec::from_vec(vec_bytes);

    // Send response back to host (transfer ownership)
    send_result(sid, NrStatus::Ok, nr_vec);

    NrStatus::Ok
}

// Uppercase handler - converts input to uppercase
unsafe fn handle_uppercase(sid: u64, payload: NrBytes) -> NrStatus {
    let data = payload.as_slice();
    let text = String::from_utf8_lossy(data).to_uppercase();
    println!("[Plugin] Uppercase received, sending back: {}", text);

    // Send response back to host
    let nr_vec = NrVec::from_string(text);
    send_result(sid, NrStatus::Ok, nr_vec);

    NrStatus::Ok
}

// benchmark
unsafe fn handle_benchmark(_sid: u64, _payload: NrBytes) -> NrStatus {
    // let data = payload.as_slice();
    // let _text = String::from_utf8_lossy(data);
    // println!("[Plugin] Benchmark received: {}", text);
    // Send response back to host
    // send_result(sid, NrStatus::Ok, NrVec::from_nr_bytes(payload));

    NrStatus::Ok
}

// Define the plugin with its entry points
define_plugin! {
    init: init,
    shutdown: shutdown,
    entries: {
        "echo" => handle_echo,
        "uppercase" => handle_uppercase,
        "benchmark" => handle_benchmark,
    }
}
