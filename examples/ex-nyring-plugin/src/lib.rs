use nylon_ring::{
    NrBytes, NrHostVTable, NrHostVTableV2, NrOwnedBytesV2, NrStatus, NrVec, define_plugin,
};
use std::ffi::c_void;
use std::sync::atomic::{AtomicPtr, AtomicU64, Ordering};

static HOST_CTX: AtomicPtr<c_void> = AtomicPtr::new(std::ptr::null_mut());
static HOST_VTABLE: AtomicPtr<NrHostVTable> = AtomicPtr::new(std::ptr::null_mut());
/// Set only when the host loaded us through the v2 entry point.
static HOST_VTABLE_V2: AtomicPtr<NrHostVTableV2> = AtomicPtr::new(std::ptr::null_mut());

#[inline(always)]
pub fn send_result(sid: u64, status: NrStatus, data: nylon_ring::NrVec<u8>) -> NrStatus {
    let host_ctx = HOST_CTX.load(Ordering::Acquire);
    let host_vtable = HOST_VTABLE.load(Ordering::Acquire);
    assert!(!host_ctx.is_null(), "plugin is not initialized");
    assert!(!host_vtable.is_null(), "plugin is not initialized");
    let send_result = unsafe { (*host_vtable).send_result };
    unsafe { send_result(host_ctx, sid, status, data) }
}

/// Sends a response the host consumes without copying (ABI v2 only).
#[inline(always)]
pub fn send_result_owned(sid: u64, status: NrStatus, data: NrOwnedBytesV2) -> NrStatus {
    let host_ctx = HOST_CTX.load(Ordering::Acquire);
    let host_vtable = HOST_VTABLE_V2.load(Ordering::Acquire);
    assert!(!host_ctx.is_null(), "plugin is not initialized");
    assert!(!host_vtable.is_null(), "host does not speak ABI v2");
    let send_result_owned = unsafe { (*host_vtable).send_result_owned };
    unsafe { send_result_owned(host_ctx, sid, status, data) }
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

// v2 initialization: remember the owned-response callback and reuse the
// embedded v1 table for everything else.
unsafe fn init_v2(host_ctx: *mut c_void, host_vtable: *const NrHostVTableV2) -> NrStatus {
    if host_vtable.is_null() {
        return NrStatus::Invalid;
    }
    let status = unsafe { init(host_ctx, std::ptr::addr_of!((*host_vtable).v1)) };
    if status == NrStatus::Ok {
        HOST_VTABLE_V2.store(host_vtable.cast_mut(), Ordering::Release);
    }
    status
}

// Shutdown the plugin
fn shutdown() {
    HOST_VTABLE_V2.store(std::ptr::null_mut(), Ordering::Release);
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

// Silent stream handler for benchmarks: 8 empty data frames + StreamEnd.
// ABI v2 handlers: responses the host consumes without copying.

/// Static response slab for the owned-response benchmark: models a plugin
/// that answers from its own long-lived buffers (cache, arena, mmap).
static RESPONSE_SLAB: [u8; 65536] = [42u8; 65536];

/// Responds with `payload.len()` bytes borrowed from the static slab —
/// nothing to allocate, copy, or release on either side.
unsafe fn handle_benchmark_owned(sid: u64, payload: NrBytes) -> NrStatus {
    let len = (payload.len as usize).min(RESPONSE_SLAB.len());
    send_result_owned(
        sid,
        NrStatus::Ok,
        NrOwnedBytesV2::from_static(&RESPONSE_SLAB[..len]),
    )
}

/// Number of times the host released an owned response of ours.
static OWNED_RELEASES: AtomicU64 = AtomicU64::new(0);

unsafe extern "C" fn release_counted(owner_ctx: *mut c_void, ptr: *const u8, len: u64) {
    OWNED_RELEASES.fetch_add(1, Ordering::AcqRel);
    if !ptr.is_null() {
        // SAFETY: reconstructs exactly the Vec parts moved into the payload
        // by handle_echo_owned.
        drop(unsafe { Vec::from_raw_parts(ptr.cast_mut(), len as usize, owner_ctx as usize) });
    }
}

/// Echoes the payload as an owned buffer whose release is counted, so the
/// host's release-exactly-once contract is observable from tests.
unsafe fn handle_echo_owned(sid: u64, payload: NrBytes) -> NrStatus {
    let data = match unsafe { payload.as_slice() } {
        Ok(data) => data,
        Err(_) => return NrStatus::Invalid,
    };
    let mut vec = std::mem::ManuallyDrop::new(data.to_vec());
    let owned = NrOwnedBytesV2 {
        ptr: vec.as_mut_ptr(),
        len: vec.len() as u64,
        owner_ctx: vec.capacity() as *mut c_void,
        release: Some(release_counted),
    };
    send_result_owned(sid, NrStatus::Ok, owned)
}

/// Reports how many counted owned responses were released, as u64 LE bytes.
unsafe fn handle_owned_release_count(sid: u64, _payload: NrBytes) -> NrStatus {
    let count = OWNED_RELEASES.load(Ordering::Acquire);
    send_result(
        sid,
        NrStatus::Ok,
        NrVec::from_vec(count.to_le_bytes().to_vec()),
    )
}

/// Data frames per benchmark stream; the host-side harness reads the same
/// environment variable so its frame-count assertion stays in sync.
fn benchmark_stream_data_frames() -> u32 {
    static FRAMES: std::sync::OnceLock<u32> = std::sync::OnceLock::new();
    *FRAMES.get_or_init(|| {
        std::env::var("NYRING_BENCH_STREAM_FRAMES")
            .ok()
            .and_then(|value| value.parse().ok())
            .unwrap_or(8)
    })
}

unsafe fn handle_benchmark_stream(sid: u64, _payload: NrBytes) -> NrStatus {
    for _ in 0..benchmark_stream_data_frames() {
        let status = send_result(sid, NrStatus::Ok, NrVec::from_vec(Vec::new()));
        if status != NrStatus::Ok {
            return status;
        }
    }
    send_result(sid, NrStatus::StreamEnd, NrVec::from_vec(Vec::new()))
}

// Define the plugin with its entry points
define_plugin! {
    init: init,
    init_v2: init_v2,
    shutdown: shutdown,
    entries: {
        "echo" => handle_echo,
        "uppercase" => handle_uppercase,
        "stream" => handle_stream,
        "notify" => handle_notify,
        "benchmark" => handle_benchmark,
        "benchmark_without_response" => handle_benchmark_without_response,
        "benchmark_stream" => handle_benchmark_stream,
        "benchmark_owned" => handle_benchmark_owned,
        "echo_owned" => handle_echo_owned,
        "owned_release_count" => handle_owned_release_count,
    }
}
