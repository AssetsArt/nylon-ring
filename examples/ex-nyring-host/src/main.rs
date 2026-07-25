mod benchmark;

use nylon_ring_host::NylonRingHost;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let benchmark_config = benchmark::BenchmarkConfig::from_env()?;
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(benchmark_config.workers)
        .enable_all()
        .build()?
        .block_on(run(benchmark_config))
}

async fn run(
    benchmark_config: benchmark::BenchmarkConfig,
) -> Result<(), Box<dyn std::error::Error>> {
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
    #[cfg(not(debug_assertions))]
    let plugin_path = if cfg!(target_os = "macos") {
        "target/release/libex_nyring_plugin.dylib"
    } else if cfg!(target_os = "windows") {
        "target/release/ex_nyring_plugin.dll"
    } else {
        "target/release/libex_nyring_plugin.so"
    };
    #[cfg(debug_assertions)]
    let plugin_path = if cfg!(target_os = "macos") {
        "target/debug/libex_nyring_plugin.dylib"
    } else if cfg!(target_os = "windows") {
        "target/debug/ex_nyring_plugin.dll"
    } else {
        "target/debug/libex_nyring_plugin.so"
    };

    println!("Loading plugin from: {}\n", plugin_path);
    let mut host = NylonRingHost::new();
    if std::env::var("NYRING_BENCH_PINNED").is_ok_and(|value| value == "1") {
        println!("(pinned mode: unload/reload disabled, per-call guards elided)");
        host.load_pinned("default", plugin_path)
            .expect("Failed to load plugin");
    } else {
        host.load("default", plugin_path)
            .expect("Failed to load plugin");
    }

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
    println!("  Path: STANDARD ASYNC PATH (DashMap + inline completion)");
    println!("  → Uses Sharded DashMap for pending request tracking");
    println!("  → Stores the completion state and Waker in the pending entry");
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
    println!("  → Does not insert into the pending-request map");
    let message = b"Fire and forget!";
    println!("  Sending: {}", String::from_utf8_lossy(message));
    let now = std::time::Instant::now();
    let status = plugin.call("notify", message).await?;
    println!("  Call time: {:?}", now.elapsed());
    println!("  Status: {:?}\n", status);

    // Demo 4: call_stream() - Streaming responses
    println!("--- Demo 4: call_stream() ---");
    println!("  Path: STREAMING with bounded channel");
    println!("  → Uses Sharded DashMap to register stream channel");
    println!("  → Reports NrStatus::Backpressure when the channel is full");
    let message = b"start";
    println!("  Sending: {}", String::from_utf8_lossy(message));
    let now = std::time::Instant::now();
    let (sid, mut rx) = plugin.call_stream("stream", message).await?;
    println!("  Stream started with SID: {}", sid);

    // Receive streaming frames
    let mut frame_count = 0;
    while let Some(frame) = rx.recv().await {
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

    // Demo 5: Multiple rapid calls (showing robustness)
    println!("--- Demo 5: Multiple Rapid Calls ---");
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

    // Fire-and-Forget Benchmark
    if benchmark_config.runs_fire_and_forget() {
        benchmark::run_fire_and_forget_benchmark(plugin.clone(), benchmark_config, false).await;
    }
    if benchmark_config.runs_fire_by_id() {
        benchmark::run_fire_and_forget_benchmark(plugin.clone(), benchmark_config, true).await;
    }

    // Request-Response Fast Benchmark
    if benchmark_config.runs_fast() {
        benchmark::run_request_response_fast_benchmark(plugin.clone(), benchmark_config, false)
            .await;
    }
    if benchmark_config.runs_fast_by_id() {
        benchmark::run_request_response_fast_benchmark(plugin.clone(), benchmark_config, true)
            .await;
    }

    // Request-Response Benchmark
    if benchmark_config.runs_unary() {
        benchmark::run_request_response_benchmark(plugin.clone(), benchmark_config, false).await;
    }
    if benchmark_config.runs_unary_by_id() {
        benchmark::run_request_response_benchmark(plugin.clone(), benchmark_config, true).await;
    }

    // Owned-Response Benchmark (zero-copy)
    if benchmark_config.runs_owned() {
        benchmark::run_owned_response_benchmark(plugin.clone(), benchmark_config).await;
    }

    // Lease-Response Benchmark (host-owned output buffers)
    if benchmark_config.runs_lease() {
        benchmark::run_lease_response_benchmark(plugin.clone(), benchmark_config).await;
    }

    // Streaming Benchmark
    if benchmark_config.runs_stream() {
        benchmark::run_stream_benchmark(plugin.clone(), benchmark_config).await;
    }

    println!("\n=== Demo Complete ===");
    println!("\nExecution Path Summary:");
    println!("  1. call_response_fast() → ULTRA-FAST DIRECT SLOT (TLS)");
    println!("  2. call_response()      → STANDARD ASYNC (DashMap + inline completion)");
    println!("  3. call()               → FIRE-AND-FORGET (No pending entry)");
    println!("  4. call_stream()        → STREAMING (bounded mpsc + Map)");
    Ok(())
}
