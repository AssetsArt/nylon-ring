use nylon_ring::{define_plugin, NrBytes, NrHostVTable, NrStatus, NrVec};
use std::ffi::c_void;
use std::sync::OnceLock;

// Global state to store host context and vtable
static mut HOST_CTX: *mut c_void = std::ptr::null_mut();
static mut HOST_VTABLE: *const NrHostVTable = std::ptr::null();

// Tokio runtime for async operations
static TOKIO_RT: OnceLock<tokio::runtime::Runtime> = OnceLock::new();

fn get_runtime() -> &'static tokio::runtime::Runtime {
    TOKIO_RT.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(4)
            .enable_all()
            .build()
            .expect("Failed to create Tokio runtime")
    })
}

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
    // Initialize Tokio runtime
    let _ = get_runtime();
    println!("[Plugin] Tokio runtime initialized with 4 worker threads");
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

// Stream handler - sends multiple responses
unsafe fn handle_stream(sid: u64, _payload: NrBytes) -> NrStatus {
    println!("[Plugin] Stream handler started for SID: {}", sid);

    // Send 5 frames
    for i in 1..=5 {
        let message = format!("Frame {}/5", i);
        let nr_vec = NrVec::from_string(message);
        send_result(sid, NrStatus::Ok, nr_vec);
    }

    // Send final frame with StreamEnd status
    let final_message = "Stream complete";
    let nr_vec = NrVec::from_string(final_message.to_string());
    send_result(sid, NrStatus::StreamEnd, nr_vec);

    NrStatus::Ok
}

// Async handler - demonstrates async operations using Tokio runtime
unsafe fn handle_async(sid: u64, payload: NrBytes) -> NrStatus {
    let data = payload.as_slice();
    let text = String::from_utf8_lossy(data).to_string();
    println!(
        "[Plugin] Async handler started for SID: {} with: {}",
        sid, text
    );
    println!("[Plugin] Spawning async task...");
    std::io::Write::flush(&mut std::io::stdout()).ok();

    // Spawn async task on Tokio runtime
    let rt = get_runtime();
    rt.block_on(async move {
        println!("[Plugin] Async task running on Tokio runtime...");
        std::io::Write::flush(&mut std::io::stdout()).ok();

        // Simulate async work (e.g., database query, HTTP request, etc.)
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

        println!("[Plugin] Async work completed!");
        std::io::Write::flush(&mut std::io::stdout()).ok();

        // Send result back to host
        let result = format!("Async result: {} (processed after 100ms)", text);
        println!("[Plugin] Sending result back to host: {}", result);
        std::io::Write::flush(&mut std::io::stdout()).ok();

        let nr_vec = NrVec::from_string(result);
        send_result(sid, NrStatus::Ok, nr_vec);

        println!("[Plugin] Result sent!");
        std::io::Write::flush(&mut std::io::stdout()).ok();
    });

    println!("[Plugin] Async handler returning Ok (task spawned)");
    std::io::Write::flush(&mut std::io::stdout()).ok();
    NrStatus::Ok
}

// benchmark - fast handler for benchmarking
unsafe fn handle_benchmark(sid: u64, payload: NrBytes) -> NrStatus {
    // Echo back the payload for benchmark
    let nr_vec = NrVec::from_nr_bytes(payload);
    send_result(sid, NrStatus::Ok, nr_vec);
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
        "async" => handle_async,
        "benchmark" => handle_benchmark,
        "benchmark_without_response" => handle_benchmark_without_response,
    }
}
