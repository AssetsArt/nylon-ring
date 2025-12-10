mod benchmark;

use nylon_ring_host::NylonRingHost;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("=== Nylon Ring Demo ===\n");

    // Build the plugin first
    println!("Building plugin...");
    let build_status = std::process::Command::new("cargo")
        .args([
            "build",
            "--manifest-path",
            "examples/ex-nyring-plugin/Cargo.toml",
            "-r",
        ])
        .status()?;

    if !build_status.success() {
        eprintln!("Failed to build plugin");
        return Err("Plugin build failed".into());
    }

    // Load the plugin
    let plugin_path = if cfg!(target_os = "macos") {
        "target/release/libex_nyring_plugin.dylib"
    } else if cfg!(target_os = "windows") {
        "target/release/ex_nyring_plugin.dll"
    } else {
        "target/release/libex_nyring_plugin.so"
    };

    println!("Loading plugin from: {}\n", plugin_path);
    let mut host = NylonRingHost::new();
    host.load("default", plugin_path)
        .expect("Failed to load plugin");

    // Get a handle to the plugin
    let plugin = host.plugin("default").expect("Plugin not found");

    // Demo 1: call_response_fast (Ultra-fast synchronous path)
    println!("--- Demo 1: call_response_fast() ---");
    println!("  Path: ULTRA-FAST DIRECT SLOT (synchronous, same-thread only)");
    println!("  → Plugin must call send_result synchronously on same thread");
    println!("  → Uses CURRENT_UNARY_RESULT thread-local slot");
    let message = b"Hello via fast path!";
    println!("  Sending: {}", String::from_utf8_lossy(message));
    let now = std::time::Instant::now();
    let (status, response) = plugin.call_response_fast("echo", message).await?;
    println!("  Round trip time: {:?}", now.elapsed());
    println!("  Status: {:?}", status);
    println!(
        "  Response: {}\n",
        String::from_utf8_lossy(response.as_slice())
    );

    // Demo 2: call_response (Standard Async Path)
    println!("--- Demo 2: call_response() ---");
    println!("  Path: STANDARD ASYNC PATH (DashMap + Oneshot)");
    println!("  → Uses Sharded DashMap for pending request tracking");
    println!("  → Uses Tokio Oneshot channel for awaiting response");
    println!("  → Safe for cross-thread (Plugin can reply from any thread)");
    let message = b"Hello via standard path!";
    println!("  Sending: {}", String::from_utf8_lossy(message));
    let now = std::time::Instant::now();
    let (status, response) = plugin.call_response("echo", message).await?;
    println!("  Round trip time: {:?}", now.elapsed());
    println!("  Status: {:?}", status);
    println!(
        "  Response: {}\n",
        String::from_utf8_lossy(response.as_slice())
    );

    // Demo 3: call() - Fire and forget
    println!("--- Demo 3: call() ---");
    println!("  Path: FIRE-AND-FORGET (no response expected)");
    println!("  → Does not wait for plugin response");
    println!("  → No pending request tracking (Zero Map overhead)");
    let message = b"Fire and forget!";
    println!("  Sending: {}", String::from_utf8_lossy(message));
    let now = std::time::Instant::now();
    let status = plugin.call("echo", message).await?;
    println!("  Call time: {:?}", now.elapsed());
    println!("  Status: {:?}\n", status);

    // Demo 4: Async plugin handler (using Tokio runtime in plugin)
    println!("--- Demo 4: Async Plugin Handler ---");
    println!("  Path: STANDARD ASYNC PATH (call_response)");
    println!("  → Plugin spawns async task on Tokio runtime");
    println!("  → Demonstrates async operations in plugin (100ms delay)");
    let message = b"Async test";
    println!("  Sending: {}", String::from_utf8_lossy(message));
    let now = std::time::Instant::now();
    let (status, response) = plugin.call_response("async", message).await?;
    println!("  Round trip time: {:?}", now.elapsed());
    println!("  Status: {:?}", status);
    println!(
        "  Response: {}\n",
        String::from_utf8_lossy(response.as_slice())
    );

    // Demo 5: call_stream() - Streaming responses
    println!("--- Demo 5: call_stream() ---");
    println!("  Path: STREAMING with unbounded channel");
    println!("  → Uses Sharded DashMap to register stream channel");
    println!("  → Multiple responses per request via mpsc::UnboundedSender");
    let message = b"start";
    println!("  Sending: {}", String::from_utf8_lossy(message));
    let now = std::time::Instant::now();
    let (sid, mut rx) = plugin.call_stream("stream", message).await?;
    println!("  Stream started with SID: {}", sid);

    // Receive streaming frames (blocking read is safe here since we are using std::sync::mpsc)
    let mut frame_count = 0;
    for frame in rx {
        frame_count += 1;
        println!(
            "  Frame {}: status={:?}, data={}",
            frame_count,
            frame.status,
            String::from_utf8_lossy(&frame.data)
        );

        // Check if stream ended
        if matches!(
            frame.status,
            nylon_ring_host::NrStatus::StreamEnd
                | nylon_ring_host::NrStatus::Err
                | nylon_ring_host::NrStatus::Invalid
        ) {
            break;
        }
    }
    println!("  Stream completed in {:?}", now.elapsed());
    println!("  Total frames received: {}\n", frame_count);

    // Demo 6: Multiple rapid calls (showing Robustness)
    println!("--- Demo 6: Multiple Rapid Calls ---");
    println!("  Path: Testing Sharded DashMap under load");
    println!("  → Running 10 sequential async calls");
    println!("  → Verifies map insertion/removal consistency");
    let now = std::time::Instant::now();
    for i in 1..=10 {
        let message = format!("Message #{}", i);
        let (status, _) = plugin.call_response("echo", message.as_bytes()).await?;
        println!("  Call {}: {:?}", i, status);
    }
    println!("  10 calls completed in {:?}\n", now.elapsed());

    // Demo 7: Full Dispatcher API
    println!("--- Demo 7: Full Dispatcher API ---");
    println!("  Path: Plugin -> Host -> Plugin (Dispatch)");
    println!("  Verifying all dispatcher modes:");

    // 7.1 Sync
    let message = b"Dispatch Sync";
    println!(
        "  [7.1] Sync: Sending {:?}",
        String::from_utf8_lossy(message)
    );
    let (status, response) = plugin.call_response("dispatch_sync", message).await?;
    println!("  Status: {:?}", status);
    println!("  Response: {}\n", String::from_utf8_lossy(&response));

    // 7.2 Fast
    let message = b"Dispatch Fast";
    println!(
        "  [7.2] Fast: Sending {:?}",
        String::from_utf8_lossy(message)
    );
    let (status, response) = plugin.call_response("dispatch_fast", message).await?;
    println!("  Status: {:?}", status);
    println!("  Response: {}\n", String::from_utf8_lossy(&response));

    // 7.3 Async (Fire and forget from plugin)
    let message = b"Dispatch Async";
    println!(
        "  [7.3] Async: Sending {:?}",
        String::from_utf8_lossy(message)
    );
    let (status, _) = plugin.call_response("dispatch_async", message).await?; // We use call_response to wait for plugin to *send* the dispatch
    println!("  Status: {:?}\n", status);

    // 7.4 Stream
    let message = b"Dispatch Stream";
    println!(
        "  [7.4] Stream: Sending {:?}",
        String::from_utf8_lossy(message)
    );
    // The plugin consumes the stream internally and sends us a result once done.
    let (status, response) = plugin.call_response("dispatch_stream", message).await?;
    println!("  Status: {:?}", status);
    println!("  Response: {}\n", String::from_utf8_lossy(&response));

    // Fire-and-Forget Benchmark
    benchmark::run_fire_and_forget_benchmark(plugin.clone()).await;

    // Request-Response Fast Benchmark
    benchmark::run_request_response_fast_benchmark(plugin.clone()).await;

    // Request-Response Benchmark
    benchmark::run_request_response_benchmark(plugin.clone()).await;

    println!("\n=== Demo Complete ===");
    println!("\nExecution Path Summary:");
    println!("  1. call_response_fast() → ULTRA-FAST DIRECT SLOT (TLS)");
    println!("  2. call_response()      → STANDARD ASYNC (DashMap + Oneshot)");
    println!("  3. call()               → FIRE-AND-FORGET (No Map)");
    println!("  4. async handler        → Verified Async Correctness");
    println!("  5. call_stream()        → STREAMING (mpsc + Map)");
    Ok(())
}
