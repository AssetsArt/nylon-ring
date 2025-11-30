#[cfg(not(debug_assertions))]
use mimalloc::MiMalloc;

#[cfg(not(debug_assertions))]
#[global_allocator]
static GLOBAL: MiMalloc = MiMalloc;

use futures::future::join_all;
use nylon_ring_host::NylonRingHost;
use std::env;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

// --- CONFIGURATION ---
const DURATION_SECS: u64 = 10;
const BATCH_SIZE: usize = 130;

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
    let plugin_path = get_plugin_path();
    if !plugin_path.exists() {
        eprintln!("Plugin not found at {:?}.", plugin_path);
        return;
    }
    println!("Loading plugin: {:?}", plugin_path);

    let host = Arc::new(
        NylonRingHost::load(plugin_path.to_str().unwrap()).expect("Failed to load plugin"),
    );

    let concurrency = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(8);

    println!("========================================================");
    println!("STRESS TEST CONFIGURATION (BATCH PIPELINE MODE)");
    println!("Threads (Workers)    : {}", concurrency);
    println!("Batch Size (Pipeline): {}", BATCH_SIZE);
    println!("Duration             : {} seconds", DURATION_SECS);
    #[cfg(not(target_os = "windows"))]
    println!("Allocator            : MiMalloc (Optimized)");
    println!("========================================================\n");

    println!(">>> Running Test 1: Standard call_raw (Normal Path)...");
    let rps_standard = run_benchmark(concurrency, host.clone(), BenchmarkMode::Standard).await;

    println!("\n>>> Running Test 2: Fast Unary call_raw_unary_fast...");
    let rps_fast = run_benchmark(concurrency, host.clone(), BenchmarkMode::FastUnary).await;

    println!("\n========================================================");
    println!("FINAL RESULTS");
    println!("Standard Path : {:>12.2} req/sec", rps_standard);
    println!("Fast Path     : {:>12.2} req/sec", rps_fast);

    let diff = rps_fast - rps_standard;
    let pct = (diff / rps_standard) * 100.0;

    if pct > 0.0 {
        println!("Improvement   : \x1b[32m+{:.2}%\x1b[0m", pct);
    } else {
        println!("Difference    : {:.2}%", pct);
    }
    println!("========================================================");
}

#[derive(Clone, Copy)]
enum BenchmarkMode {
    Standard,
    FastUnary,
}

async fn run_benchmark(concurrency: usize, host: Arc<NylonRingHost>, mode: BenchmarkMode) -> f64 {
    let total_requests = Arc::new(AtomicU64::new(0));
    let start_signal = Arc::new(tokio::sync::Notify::new());
    let mut handles = Vec::with_capacity(concurrency);

    for _ in 0..concurrency {
        let host = host.clone();
        let counter = total_requests.clone();
        let start_signal = start_signal.clone();

        handles.push(tokio::spawn(async move {
            let payload: &'static [u8] = b"bench";

            start_signal.notified().await;

            let start_time = Instant::now();
            let bench_duration = Duration::from_secs(DURATION_SECS);

            // แยก Loop ออกจากกันชัดเจน เพื่อให้ Compiler สร้าง Type ของ Vector แยกกัน
            // แบบนี้จะไม่ติด error mismatched types และได้ performance สูงสุด
            match mode {
                BenchmarkMode::FastUnary => {
                    let mut futures_batch = Vec::with_capacity(BATCH_SIZE);
                    while start_time.elapsed() < bench_duration {
                        for _ in 0..BATCH_SIZE {
                            futures_batch.push(host.call_raw_unary_fast("echo", payload));
                        }
                        // join_all จะรอจนครบ batch นี้
                        let _ = join_all(futures_batch.drain(..)).await;
                        counter.fetch_add(BATCH_SIZE as u64, Ordering::Relaxed);
                    }
                }
                BenchmarkMode::Standard => {
                    let mut futures_batch = Vec::with_capacity(BATCH_SIZE);
                    while start_time.elapsed() < bench_duration {
                        for _ in 0..BATCH_SIZE {
                            futures_batch.push(host.call_raw("echo", payload));
                        }
                        let _ = join_all(futures_batch.drain(..)).await;
                        counter.fetch_add(BATCH_SIZE as u64, Ordering::Relaxed);
                    }
                }
            }
        }));
    }

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
    println!("  -> RPS: {:.2}", rps);

    rps
}
