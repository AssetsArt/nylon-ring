use nylon_ring_host::NylonRingHost;
use std::env;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

fn get_plugin_path() -> PathBuf {
    let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    path.pop(); // Go up to workspace root
    path.push("target");
    path.push("release"); // Use release build
    #[cfg(target_os = "macos")]
    path.push("libnylon_ring_bench_plugin.dylib");
    #[cfg(target_os = "linux")]
    path.push("libnylon_ring_bench_plugin.so");
    #[cfg(target_os = "windows")]
    path.push("nylon_ring_bench_plugin.dll");
    path
}

#[tokio::main]
async fn main() {
    let plugin_path = get_plugin_path();
    if !plugin_path.exists() {
        eprintln!("Plugin not found at {:?}. Please run 'cargo build --release -p nylon-ring-bench-plugin' first.", plugin_path);
        std::process::exit(1);
    }

    println!("Loading plugin from {:?}", plugin_path);
    let host = Arc::new(
        NylonRingHost::load(plugin_path.to_str().unwrap()).expect("Failed to load plugin"),
    );

    let duration = Duration::from_secs(10);
    let concurrency = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(8);

    println!(
        "Starting stress test with {} concurrent tasks for {:?}...",
        concurrency, duration
    );

    let start_time = Instant::now();
    let total_requests = Arc::new(AtomicUsize::new(0));
    let mut handles = vec![];

    for _ in 0..concurrency {
        let host = host.clone();
        let total_requests = total_requests.clone();

        handles.push(tokio::spawn(async move {
            let start = Instant::now();
            let payload = b"bench";
            let mut local_count = 0;
            while start.elapsed() < duration {
                if let Ok(_) = host.call_raw("echo", payload).await {
                    local_count += 1;
                }
            }
            total_requests.fetch_add(local_count, Ordering::Relaxed);
        }));
    }

    for handle in handles {
        let _ = handle.await;
    }

    let elapsed = start_time.elapsed();
    let count = total_requests.load(Ordering::Relaxed);
    let req_per_sec = count as f64 / elapsed.as_secs_f64();

    println!("Completed {} requests in {:.2?}", count, elapsed);
    println!("Throughput: {:.2} req/s", req_per_sec);
}
