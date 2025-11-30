use nylon_ring_host::NylonRingHost;
use std::env;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Instant;

/// Resolve benchmark plugin path under the workspace `target/release`
fn get_plugin_path() -> PathBuf {
    let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    path.pop(); // workspace root
    path.push("target");
    path.push("release");

    #[cfg(target_os = "macos")]
    path.push("libnylon_ring_bench_plugin.dylib");
    #[cfg(target_os = "linux")]
    path.push("libnylon_ring_bench_plugin.so");
    #[cfg(target_os = "windows")]
    path.push("nylon_ring_bench_plugin.dll");

    path
}

#[tokio::main(flavor = "multi_thread")]
async fn main() {
    // -----------------------------
    // 1) Load plugin
    // -----------------------------
    let plugin_path = get_plugin_path();
    if !plugin_path.exists() {
        eprintln!(
            "Plugin not found at {:?}. \
             Please run 'cargo build --release -p nylon-ring-bench-plugin' first.",
            plugin_path
        );
        std::process::exit(1);
    }

    println!("Loading plugin: {:?}", plugin_path);

    let host = Arc::new(
        NylonRingHost::load(plugin_path.to_str().unwrap()).expect("Failed to load plugin"),
    );

    // -----------------------------
    // 2) Benchmark parameters
    // -----------------------------

    let duration_secs = 10;
    let concurrency = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(8);

    let estimated_rps_per_thread = 1_300_000usize;
    let iters_per_thread = estimated_rps_per_thread * duration_secs;

    println!(
        "Starting NylonRing stress test: {} threads × {} iterations each...",
        concurrency, iters_per_thread
    );

    // -----------------------------
    // 3) Run workers
    // -----------------------------
    let total_requests = Arc::new(AtomicUsize::new(0));
    let start_time = Instant::now();

    let mut handles = Vec::with_capacity(concurrency);

    for _ in 0..concurrency {
        let host = host.clone();
        let counter = total_requests.clone();
        let iters = iters_per_thread;

        handles.push(tokio::spawn(async move {
            let payload: &'static [u8] = b"bench";
            let mut local_count = 0usize;

            for _ in 0..iters {
                // let now = Instant::now();
                if let Ok(_) = host.call_raw("echo", payload).await {
                    local_count += 1;
                }
                // println!("roundtrip: {:?}", now.elapsed());
                // break;
            }

            counter.fetch_add(local_count, Ordering::Relaxed);
        }));
    }

    // Wait all workers
    for h in handles {
        let _ = h.await;
    }

    // -----------------------------
    // 4) Summary
    // -----------------------------
    let elapsed = start_time.elapsed();
    let total = total_requests.load(Ordering::Relaxed);
    let rps = total as f64 / elapsed.as_secs_f64();

    println!("--------------------------------------------");
    println!("Completed {} calls in {:.2?}", total, elapsed);
    println!("Throughput: {:.2} calls/sec", rps);
    println!("Per-thread avg: {:.2} calls/sec", rps / concurrency as f64);

    // Test fast unary path
    println!(
        "\n\nStarting NylonRing fast unary stress test: {} threads × {} iterations each...",
        concurrency, iters_per_thread
    );
    let total_requests = Arc::new(AtomicUsize::new(0));
    let start_time = Instant::now();

    let mut handles = Vec::with_capacity(concurrency);

    for _ in 0..concurrency {
        let host = host.clone();
        let counter = total_requests.clone();
        let iters = iters_per_thread;

        handles.push(tokio::spawn(async move {
            let payload: &'static [u8] = b"bench";
            let mut local_count = 0usize;

            for _ in 0..iters {
                // let now = Instant::now();
                if let Ok(_) = host.call_raw_unary_fast("echo", payload).await {
                    local_count += 1;
                }
                // println!("roundtrip: {:?}", now.elapsed());
                // break;
            }

            counter.fetch_add(local_count, Ordering::Relaxed);
        }));
    }

    // Wait all workers
    for h in handles {
        let _ = h.await;
    }

    let elapsed = start_time.elapsed();
    let total = total_requests.load(Ordering::Relaxed);
    let rps = total as f64 / elapsed.as_secs_f64();

    println!("--------------------------------------------");
    println!("Completed {} calls in {:.2?}", total, elapsed);
    println!("Throughput: {:.2} calls/sec", rps);
    println!("Per-thread avg: {:.2} calls/sec", rps / concurrency as f64);
}
