use futures::future::join_all;
use nylon_ring_host::PluginHandle;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

const DEFAULT_DURATION_SECS: u64 = 10;
const DEFAULT_BATCH_SIZE: usize = 100;

#[derive(Debug, Copy, Clone)]
pub struct BenchmarkConfig {
    pub workers: usize,
    duration_secs: u64,
    batch_size: usize,
}

impl BenchmarkConfig {
    pub fn from_env() -> Result<Self, Box<dyn std::error::Error>> {
        let default_workers = std::thread::available_parallelism()
            .map(|workers| workers.get())
            .unwrap_or(8);

        Ok(Self {
            workers: parse_positive_env("NYRING_BENCH_WORKERS", default_workers)?,
            duration_secs: parse_positive_env("NYRING_BENCH_SECONDS", DEFAULT_DURATION_SECS)?,
            batch_size: parse_positive_env("NYRING_BENCH_BATCH_SIZE", DEFAULT_BATCH_SIZE)?,
        })
    }
}

fn parse_positive_env<T>(name: &str, default: T) -> Result<T, Box<dyn std::error::Error>>
where
    T: std::str::FromStr + PartialEq + Default,
    T::Err: std::error::Error + 'static,
{
    let Some(value) = std::env::var_os(name) else {
        return Ok(default);
    };
    let value = value.to_string_lossy().parse::<T>()?;
    if value == T::default() {
        return Err(format!("{name} must be greater than zero").into());
    }
    Ok(value)
}

/// Run a fire-and-forget benchmark (calls without waiting for response)
pub async fn run_fire_and_forget_benchmark(plugin: PluginHandle, config: BenchmarkConfig) {
    println!("\n--- Benchmark: Fire-and-Forget ---");

    let mut handles = Vec::with_capacity(config.workers);
    let total_requests = Arc::new(AtomicU64::new(0));
    let total_latency_nanos = Arc::new(AtomicU64::new(0));
    let start_signal = Arc::new(tokio::sync::Notify::new());

    println!("  -> Using {} threads", config.workers);
    println!("  -> Using {} requests per batch", config.batch_size);
    println!("  -> Using {} seconds for benchmark", config.duration_secs);

    let payload: &'static [u8] = b"";
    println!("  -> Payload Size: {}", payload.len());

    for _ in 0..config.workers {
        let plugin = plugin.clone();
        let counter = total_requests.clone();
        let latency_counter = total_latency_nanos.clone();
        let start_signal = start_signal.clone();

        let handle = tokio::spawn(async move {
            // Wait for signal
            start_signal.notified().await;

            let start_time = Instant::now();
            let bench_duration = Duration::from_secs(config.duration_secs);
            let mut futures_batch = Vec::with_capacity(config.batch_size);

            while start_time.elapsed() < bench_duration {
                let batch_start = Instant::now();
                for _ in 0..config.batch_size {
                    futures_batch.push(plugin.call("benchmark_without_response", payload));
                }
                let _ = join_all(futures_batch.drain(..)).await;
                let batch_elapsed = batch_start.elapsed();

                counter.fetch_add(config.batch_size as u64, Ordering::Relaxed);
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
    let avg_latency_nanos = total_lat_nanos.checked_div(total).unwrap_or(0);

    println!("  -> Processed {} requests in {:.2?}", total, elapsed);
    println!("  -> RPS: {:.2}/sec", rps);
    println!("  -> Average latency: {:.2} ns/request", avg_latency_nanos);
}

/// Run a request-response benchmark
pub async fn run_request_response_benchmark(plugin: PluginHandle, config: BenchmarkConfig) {
    println!("\n--- Benchmark: Request-Response ---");

    let mut handles = Vec::with_capacity(config.workers);
    let total_requests = Arc::new(AtomicU64::new(0));
    let total_latency_nanos = Arc::new(AtomicU64::new(0));
    let start_signal = Arc::new(tokio::sync::Notify::new());

    println!("  -> Using {} threads", config.workers);
    println!("  -> Using {} requests per batch", config.batch_size);
    println!("  -> Using {} seconds for benchmark", config.duration_secs);

    let payload: &'static [u8] = b"";
    println!("  -> Payload Size: {}", payload.len());

    for _ in 0..config.workers {
        let plugin = plugin.clone();
        let counter = total_requests.clone();
        let latency_counter = total_latency_nanos.clone();
        let start_signal = start_signal.clone();

        let handle = tokio::spawn(async move {
            // Wait for signal
            start_signal.notified().await;

            let start_time = Instant::now();
            let bench_duration = Duration::from_secs(config.duration_secs);
            let mut futures_batch = Vec::with_capacity(config.batch_size);

            while start_time.elapsed() < bench_duration {
                let batch_start = Instant::now();
                for _ in 0..config.batch_size {
                    futures_batch.push(plugin.call_response("benchmark", payload));
                }
                let _ = join_all(futures_batch.drain(..)).await;
                let batch_elapsed = batch_start.elapsed();

                counter.fetch_add(config.batch_size as u64, Ordering::Relaxed);
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
    let avg_latency_nanos = total_lat_nanos.checked_div(total).unwrap_or(0);

    println!("  -> Processed {} requests in {:.2?}", total, elapsed);
    println!("  -> RPS: {:.2}/sec", rps);
    println!("  -> Average latency: {:.2} ns/request", avg_latency_nanos);
}

/// Run a request-response fast benchmark
pub async fn run_request_response_fast_benchmark(plugin: PluginHandle, config: BenchmarkConfig) {
    println!("\n--- Benchmark: Request-Response Fast ---");

    let mut handles = Vec::with_capacity(config.workers);
    let total_requests = Arc::new(AtomicU64::new(0));
    let total_latency_nanos = Arc::new(AtomicU64::new(0));
    let start_signal = Arc::new(tokio::sync::Notify::new());

    println!("  -> Using {} threads", config.workers);
    println!("  -> Using {} requests per batch", config.batch_size);
    println!("  -> Using {} seconds for benchmark", config.duration_secs);

    let payload: &'static [u8] = b"";
    println!("  -> Payload Size: {}", payload.len());

    for _ in 0..config.workers {
        let plugin = plugin.clone();
        let counter = total_requests.clone();
        let latency_counter = total_latency_nanos.clone();
        let start_signal = start_signal.clone();

        let handle = tokio::spawn(async move {
            // Wait for signal
            start_signal.notified().await;

            let start_time = Instant::now();
            let bench_duration = Duration::from_secs(config.duration_secs);
            let mut futures_batch = Vec::with_capacity(config.batch_size);

            while start_time.elapsed() < bench_duration {
                let batch_start = Instant::now();
                for _ in 0..config.batch_size {
                    futures_batch.push(plugin.call_response_fast("benchmark", payload));
                }
                let _ = join_all(futures_batch.drain(..)).await;
                let batch_elapsed = batch_start.elapsed();

                counter.fetch_add(config.batch_size as u64, Ordering::Relaxed);
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
    let avg_latency_nanos = total_lat_nanos.checked_div(total).unwrap_or(0);

    println!("  -> Processed {} requests in {:.2?}", total, elapsed);
    println!("  -> RPS: {:.2}/sec", rps);
    println!("  -> Average latency: {:.2} ns/request", avg_latency_nanos);
}
