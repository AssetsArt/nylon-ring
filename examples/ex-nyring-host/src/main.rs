use futures::future::join_all;
use nylon_ring_host::NylonRingHost;
use std::{
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};

// --- CONFIGURATION ---
const DURATION_SECS: u64 = 10;
// BATCH_SIZE: used for Unary/Standard tests to create pipelining without spawn overhead
const BATCH_SIZE: usize = 130;

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
        ])
        .status()?;

    if !build_status.success() {
        eprintln!("Failed to build plugin");
        return Err("Plugin build failed".into());
    }

    // Load the plugin
    let plugin_path = if cfg!(target_os = "macos") {
        "target/debug/libex_nyring_plugin.dylib"
    } else if cfg!(target_os = "windows") {
        "target/debug/ex_nyring_plugin.dll"
    } else {
        "target/debug/libex_nyring_plugin.so"
    };

    println!("Loading plugin from: {}\n", plugin_path);
    let host = Arc::new(NylonRingHost::load(plugin_path).expect("Failed to load plugin"));

    // Demo 1: Echo
    println!("--- Demo 1: Echo ---");
    let message = b"Hello, Nylon Ring!";
    println!("Sending: {}", String::from_utf8_lossy(message));

    let (status, response) = host.call("echo", message).await?;
    println!("Status: {:?}", status);
    println!("Response: {}\n", String::from_utf8_lossy(&response));

    // Demo 2: Uppercase
    println!("--- Demo 2: Uppercase ---");
    let message = b"make me loud";
    println!("Sending: {}", String::from_utf8_lossy(message));

    let (status, response) = host.call("uppercase", message).await?;
    println!("Status: {:?}", status);
    println!("Response: {}\n", String::from_utf8_lossy(&response));

    // Demo 3: Multiple calls
    println!("--- Demo 3: Multiple Calls ---");
    for i in 1..=5 {
        let message = format!("Message #{}", i);
        let (status, _) = host.call("echo", message.as_bytes()).await?;
        println!("Call {}: {:?}", i, status);
    }

    // Demo 4: Benchmark
    println!("\n--- Demo 4: Benchmark ---");
    let concurrency = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(8);
    let mut handles = Vec::with_capacity(concurrency);
    let total_requests = Arc::new(AtomicU64::new(0));
    let start_signal = Arc::new(tokio::sync::Notify::new());
    println!("  -> Using {} threads", concurrency);
    println!("  -> Using {} requests per batch", BATCH_SIZE);
    println!("  -> Using {} seconds for benchmark", DURATION_SECS);
    for _ in 0..concurrency {
        let host = host.clone();
        let counter = total_requests.clone();
        let start_signal = start_signal.clone();
        let handle = tokio::spawn(async move {
            let payload: &'static [u8] = b"bench";

            // Wait for signal
            start_signal.notified().await;

            let start_time = Instant::now();
            let bench_duration = Duration::from_secs(DURATION_SECS);
            let mut futures_batch = Vec::with_capacity(BATCH_SIZE);
            while start_time.elapsed() < bench_duration {
                for _ in 0..BATCH_SIZE {
                    futures_batch.push(host.call("benchmark", payload));
                }
                let _ = join_all(futures_batch.drain(..)).await;
                counter.fetch_add(BATCH_SIZE as u64, Ordering::Relaxed);
            }
        });
        handles.push(handle);
    }
    // Warmup / Sync time
    tokio::time::sleep(Duration::from_millis(100)).await;

    let start_time = Instant::now();
    start_signal.notify_waiters();

    for h in handles {
        let _ = h.await;
    }

    let elapsed = start_time.elapsed();
    let total = total_requests.load(Ordering::Relaxed);
    let rps = total as f64 / elapsed.as_secs_f64();

    println!("  -> Processed {} requests in {:.2?}", total, elapsed);
    println!("  -> RPS: {:.2}/sec", rps);

    println!("\n=== Demo Complete ===");
    Ok(())
}
