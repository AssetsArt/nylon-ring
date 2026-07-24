use nylon_ring::{NrBytes, NrHostVTable, NrStatus, NrVec, define_plugin};
use std::ffi::c_void;
use std::sync::atomic::{AtomicPtr, Ordering};

static HOST_CTX: AtomicPtr<c_void> = AtomicPtr::new(std::ptr::null_mut());
static HOST_VTABLE: AtomicPtr<NrHostVTable> = AtomicPtr::new(std::ptr::null_mut());

#[inline(always)]
pub fn send_result(sid: u64, status: NrStatus, data: nylon_ring::NrVec<u8>) -> NrStatus {
    let host_ctx = HOST_CTX.load(Ordering::Acquire);
    let host_vtable = HOST_VTABLE.load(Ordering::Acquire);
    assert!(!host_ctx.is_null(), "plugin is not initialized");
    assert!(!host_vtable.is_null(), "plugin is not initialized");
    let send_result = unsafe { (*host_vtable).send_result };
    unsafe { send_result(host_ctx, sid, status, data) }
}

// Initialize the plugin
unsafe fn init(host_ctx: *mut c_void, host_vtable: *const NrHostVTable) -> NrStatus {
    if host_ctx.is_null() || host_vtable.is_null() {
        return NrStatus::Invalid;
    }
    HOST_CTX.store(host_ctx, Ordering::Release);
    HOST_VTABLE.store(host_vtable.cast_mut(), Ordering::Release);
    NrStatus::Ok
}

// Shutdown the plugin
fn shutdown() {
    HOST_VTABLE.store(std::ptr::null_mut(), Ordering::Release);
    HOST_CTX.store(std::ptr::null_mut(), Ordering::Release);
}

// Echo handler - simply returns the input data
unsafe fn handle_echo(sid: u64, payload: NrBytes) -> NrStatus {
    let data = match unsafe { payload.as_slice() } {
        Ok(data) => data,
        Err(_) => return NrStatus::Invalid,
    };
    let text_str = String::from_utf8_lossy(data);
    println!("[Plugin] Echo received: {}", text_str);

    // Modify the text
    let new_text = format!("{}, Nylon Ring!", text_str);

    // Convert to NrVec (Zero Copy transfer)
    let vec_bytes = new_text.into_bytes();
    let nr_vec = NrVec::from_vec(vec_bytes);

    // Send response back to host (transfer ownership)
    send_result(sid, NrStatus::Ok, nr_vec)
}

// Uppercase handler - converts input to uppercase
unsafe fn handle_uppercase(sid: u64, payload: NrBytes) -> NrStatus {
    let data = match unsafe { payload.as_slice() } {
        Ok(data) => data,
        Err(_) => return NrStatus::Invalid,
    };
    let text = String::from_utf8_lossy(data).to_uppercase();
    println!("[Plugin] Uppercase received, sending back: {}", text);

    // Send response back to host
    let nr_vec = NrVec::from_string(text);
    send_result(sid, NrStatus::Ok, nr_vec)
}

// Stream handler - sends multiple responses
unsafe fn handle_stream(sid: u64, _payload: NrBytes) -> NrStatus {
    println!("[Plugin] Stream handler started for SID: {}", sid);

    // Send 5 frames
    for i in 1..=5 {
        let message = format!("Frame {}/5", i);
        let nr_vec = NrVec::from_string(message);
        let status = send_result(sid, NrStatus::Ok, nr_vec);
        if status != NrStatus::Ok {
            return status;
        }
    }

    // Send final frame with StreamEnd status
    let final_message = "Stream complete";
    let nr_vec = NrVec::from_string(final_message.to_string());
    send_result(sid, NrStatus::StreamEnd, nr_vec)
}

// Minimal synchronous handler used by both response benchmarks.
unsafe fn handle_benchmark(sid: u64, payload: NrBytes) -> NrStatus {
    let response = match unsafe { NrVec::from_nr_bytes(payload) } {
        Ok(response) => response,
        Err(_) => return NrStatus::Invalid,
    };
    send_result(sid, NrStatus::Ok, response)
}

// Fire-and-forget handler: consumes the payload without sending a response.
unsafe fn handle_notify(_sid: u64, payload: NrBytes) -> NrStatus {
    let data = match unsafe { payload.as_slice() } {
        Ok(data) => data,
        Err(_) => return NrStatus::Invalid,
    };
    println!(
        "[Plugin] Notification received: {}",
        String::from_utf8_lossy(data)
    );
    NrStatus::Ok
}

// benchmark - without response
unsafe fn handle_benchmark_without_response(_sid: u64, _payload: NrBytes) -> NrStatus {
    NrStatus::Ok
}

// Define the plugin with its entry points
define_plugin! {
    init: init,
    shutdown: shutdown,
    entries: {
        "echo" => handle_echo,
        "uppercase" => handle_uppercase,
        "stream" => handle_stream,
        "notify" => handle_notify,
        "benchmark" => handle_benchmark,
        "benchmark_without_response" => handle_benchmark_without_response,
    }
}
