use futures::future::join_all;
use nylon_ring_host::NylonRingHost;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

// Benchmark configuration
const DURATION_SECS: u64 = 10;
const BATCH_SIZE: usize = 100;

/// Run a fire-and-forget benchmark (calls without waiting for response)
pub async fn run_fire_and_forget_benchmark(host: Arc<NylonRingHost>) {
    println!("\n--- Benchmark: Fire-and-Forget ---");

    let concurrency = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(8);

    let mut handles = Vec::with_capacity(concurrency);
    let total_requests = Arc::new(AtomicU64::new(0));
    let total_latency_nanos = Arc::new(AtomicU64::new(0));
    let start_signal = Arc::new(tokio::sync::Notify::new());

    println!("  -> Using {} threads", concurrency);
    println!("  -> Using {} requests per batch", BATCH_SIZE);
    println!("  -> Using {} seconds for benchmark", DURATION_SECS);

    let payload: &'static [u8] = b"";
    println!("  -> Payload Size: {}", payload.len());

    for _ in 0..concurrency {
        let host = host.clone();
        let counter = total_requests.clone();
        let latency_counter = total_latency_nanos.clone();
        let start_signal = start_signal.clone();

        let handle = tokio::spawn(async move {
            // Wait for signal
            start_signal.notified().await;

            let start_time = Instant::now();
            let bench_duration = Duration::from_secs(DURATION_SECS);
            let mut futures_batch = Vec::with_capacity(BATCH_SIZE);

            while start_time.elapsed() < bench_duration {
                let batch_start = Instant::now();
                for _ in 0..BATCH_SIZE {
                    futures_batch.push(host.call("benchmark_without_response", payload));
                }
                let _ = join_all(futures_batch.drain(..)).await;
                let batch_elapsed = batch_start.elapsed();

                counter.fetch_add(BATCH_SIZE as u64, Ordering::Relaxed);
                latency_counter.fetch_add(batch_elapsed.as_nanos() as u64, Ordering::Relaxed);
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
    let total_lat_nanos = total_latency_nanos.load(Ordering::Relaxed);

    let rps = total as f64 / elapsed.as_secs_f64();
    let avg_latency_nanos = if total > 0 {
        total_lat_nanos / total
    } else {
        0
    };

    println!("  -> Processed {} requests in {:.2?}", total, elapsed);
    println!("  -> RPS: {:.2}/sec", rps);
    println!("  -> Average latency: {:.2} ns/request", avg_latency_nanos);
}

/// Run a request-response benchmark
pub async fn run_request_response_benchmark(host: Arc<NylonRingHost>) {
    println!("\n--- Benchmark: Request-Response ---");

    let concurrency = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(8);

    let mut handles = Vec::with_capacity(concurrency);
    let total_requests = Arc::new(AtomicU64::new(0));
    let total_latency_nanos = Arc::new(AtomicU64::new(0));
    let start_signal = Arc::new(tokio::sync::Notify::new());

    println!("  -> Using {} threads", concurrency);
    println!("  -> Using {} requests per batch", BATCH_SIZE);
    println!("  -> Using {} seconds for benchmark", DURATION_SECS);

    let payload: &'static [u8] = b"";
    println!("  -> Payload Size: {}", payload.len());

    for _ in 0..concurrency {
        let host = host.clone();
        let counter = total_requests.clone();
        let latency_counter = total_latency_nanos.clone();
        let start_signal = start_signal.clone();

        let handle = tokio::spawn(async move {
            // Wait for signal
            start_signal.notified().await;

            let start_time = Instant::now();
            let bench_duration = Duration::from_secs(DURATION_SECS);
            let mut futures_batch = Vec::with_capacity(BATCH_SIZE);

            while start_time.elapsed() < bench_duration {
                let batch_start = Instant::now();
                for _ in 0..BATCH_SIZE {
                    futures_batch.push(host.call_response("benchmark", payload));
                }
                let _ = join_all(futures_batch.drain(..)).await;
                let batch_elapsed = batch_start.elapsed();

                counter.fetch_add(BATCH_SIZE as u64, Ordering::Relaxed);
                latency_counter.fetch_add(batch_elapsed.as_nanos() as u64, Ordering::Relaxed);
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
    let total_lat_nanos = total_latency_nanos.load(Ordering::Relaxed);

    let rps = total as f64 / elapsed.as_secs_f64();
    let avg_latency_nanos = if total > 0 {
        total_lat_nanos / total
    } else {
        0
    };

    println!("  -> Processed {} requests in {:.2?}", total, elapsed);
    println!("  -> RPS: {:.2}/sec", rps);
    println!("  -> Average latency: {:.2} ns/request", avg_latency_nanos);
}

/// Run a request-response fast benchmark
pub async fn run_request_response_fast_benchmark(host: Arc<NylonRingHost>) {
    println!("\n--- Benchmark: Request-Response Fast ---");

    let concurrency = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(8);

    let mut handles = Vec::with_capacity(concurrency);
    let total_requests = Arc::new(AtomicU64::new(0));
    let total_latency_nanos = Arc::new(AtomicU64::new(0));
    let start_signal = Arc::new(tokio::sync::Notify::new());

    println!("  -> Using {} threads", concurrency);
    println!("  -> Using {} requests per batch", BATCH_SIZE);
    println!("  -> Using {} seconds for benchmark", DURATION_SECS);

    let payload: &'static [u8] = b"";
    println!("  -> Payload Size: {}", payload.len());

    for _ in 0..concurrency {
        let host = host.clone();
        let counter = total_requests.clone();
        let latency_counter = total_latency_nanos.clone();
        let start_signal = start_signal.clone();

        let handle = tokio::spawn(async move {
            // Wait for signal
            start_signal.notified().await;

            let start_time = Instant::now();
            let bench_duration = Duration::from_secs(DURATION_SECS);
            let mut futures_batch = Vec::with_capacity(BATCH_SIZE);

            while start_time.elapsed() < bench_duration {
                let batch_start = Instant::now();
                for _ in 0..BATCH_SIZE {
                    futures_batch.push(host.call_response_fast("benchmark", payload));
                }
                let _ = join_all(futures_batch.drain(..)).await;
                let batch_elapsed = batch_start.elapsed();

                counter.fetch_add(BATCH_SIZE as u64, Ordering::Relaxed);
                latency_counter.fetch_add(batch_elapsed.as_nanos() as u64, Ordering::Relaxed);
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
    let total_lat_nanos = total_latency_nanos.load(Ordering::Relaxed);

    let rps = total as f64 / elapsed.as_secs_f64();
    let avg_latency_nanos = if total > 0 {
        total_lat_nanos / total
    } else {
        0
    };

    println!("  -> Processed {} requests in {:.2?}", total, elapsed);
    println!("  -> RPS: {:.2}/sec", rps);
    println!("  -> Average latency: {:.2} ns/request", avg_latency_nanos);
}
