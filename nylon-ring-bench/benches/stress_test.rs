// stress_test.rs

// 1. Setup Allocator: MiMalloc (Requires 'mimalloc' crate)
#[cfg(not(debug_assertions))]
#[cfg(not(target_os = "windows"))]
use mimalloc::MiMalloc;

#[cfg(not(debug_assertions))]
#[cfg(not(target_os = "windows"))]
#[global_allocator]
static GLOBAL: MiMalloc = MiMalloc;

use futures::future::join_all;
// Ensure these items are exported from your nylon_ring_host lib
use nylon_ring_host::{Extensions, HighLevelRequest, NylonRingHost};
use std::env;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

// --- CONFIGURATION ---
const DURATION_SECS: u64 = 10;
// BATCH_SIZE: used for Unary/Standard tests to create pipelining without spawn overhead
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
        eprintln!(
            "Plugin not found at {:?}. Please build the bench plugin first.",
            plugin_path
        );
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

    #[cfg(not(debug_assertions))]
    #[cfg(not(target_os = "windows"))]
    println!("Allocator            : MiMalloc (Optimized)");
    #[cfg(debug_assertions)]
    println!("Allocator            : System (Debug Mode)");

    println!("========================================================\n");

    println!(">>> Running Test 1: Standard call_raw (Normal Path)...");
    let rps_standard = run_benchmark(concurrency, host.clone(), BenchmarkMode::Standard).await;

    println!("\n>>> Running Test 2: Fast Unary call_raw_unary_fast...");
    let rps_fast = run_benchmark(concurrency, host.clone(), BenchmarkMode::FastUnary).await;

    println!("\n>>> Running Test 3: Bidirectional Streaming (5 frames + 1 echo)...");
    let rps_bidi = run_benchmark(concurrency, host.clone(), BenchmarkMode::Bidirectional).await;

    println!("\n========================================================");
    println!("FINAL RESULTS");
    println!("Standard Path : {:>12.2} req/sec", rps_standard);
    println!("Fast Path     : {:>12.2} req/sec", rps_fast);
    println!("Bidirectional : {:>12.2} req/sec", rps_bidi);

    let diff = rps_fast - rps_standard;
    let pct = (diff / rps_standard) * 100.0;

    if pct > 0.0 {
        println!("Improvement (Fast vs Std) : \x1b[32m+{:.2}%\x1b[0m", pct);
    } else {
        println!("Difference (Fast vs Std)  : {:.2}%", pct);
    }
    println!("========================================================");
}

#[derive(Clone, Copy)]
enum BenchmarkMode {
    Standard,
    FastUnary,
    Bidirectional,
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

            // Wait for signal
            start_signal.notified().await;

            let start_time = Instant::now();
            let bench_duration = Duration::from_secs(DURATION_SECS);

            // แยก Loop เพื่อประสิทธิภาพสูงสุดและ Type checking
            match mode {
                BenchmarkMode::FastUnary => {
                    let mut futures_batch = Vec::with_capacity(BATCH_SIZE);
                    while start_time.elapsed() < bench_duration {
                        for _ in 0..BATCH_SIZE {
                            futures_batch.push(host.call_raw_unary_fast("echo", payload));
                        }
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
                BenchmarkMode::Bidirectional => {
                    // Streaming ต้องมีการ maintain state ต่อ request จึงไม่ใช้ Batching แบบ Unary
                    // เราวัดจำนวน "Session ที่จบสมบูรณ์" (Completed Flows)

                    while start_time.elapsed() < bench_duration {
                        let req = HighLevelRequest {
                            method: "GET".to_string(),
                            path: "/stream".to_string(),
                            query: "".to_string(),
                            headers: vec![],
                            body: vec![],
                            extensions: Extensions::new(),
                        };

                        // 1. Open Stream
                        if let Ok((sid, mut rx)) = host.call_stream("bidi_stream", req).await {
                            // 2. Consume Initial Greeting/Data (e.g., up to 5 frames)
                            let mut count = 0;
                            while let Some(_) = rx.recv().await {
                                count += 1;
                                // สมมติว่า Plugin ส่งมา 5 frames แรกแล้วรอ input เรา
                                if count >= 5 {
                                    break;
                                }
                            }

                            // 3. Send Data back to Plugin (Bidirectional)
                            let _ = host.send_stream_data(sid, payload);

                            // 4. Close Stream (Explicitly)
                            let _ = host.close_stream(sid);

                            // 5. Drain remaining frames (if any)
                            while let Some(_) = rx.recv().await {}

                            // Count as 1 completed transaction
                            counter.fetch_add(1, Ordering::Relaxed);
                        }
                    }
                }
            }
        }));
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
    println!("  -> RPS: {:.2}", rps);

    rps
}
