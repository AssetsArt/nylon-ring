use nylon_ring_host::{NrStatus, PluginHandle};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

const DEFAULT_DURATION_SECS: u64 = 10;
const DEFAULT_BATCH_SIZE: usize = 100;
const MAX_TRACKED_CPUS: usize = 64;
const CPU_SAMPLE_BATCH_INTERVAL: u64 = 1_024;

#[cfg(target_os = "macos")]
unsafe extern "C" {
    fn pthread_cpu_number_np(cpu_number_out: *mut usize) -> std::ffi::c_int;
}

#[derive(Debug)]
struct CpuSamples {
    counts: [u64; MAX_TRACKED_CPUS],
    untracked: u64,
}

impl Default for CpuSamples {
    fn default() -> Self {
        Self {
            counts: [0; MAX_TRACKED_CPUS],
            untracked: 0,
        }
    }
}

impl CpuSamples {
    fn record_current(&mut self) {
        let Some(cpu) = current_cpu() else {
            self.untracked += 1;
            return;
        };
        if let Some(count) = self.counts.get_mut(cpu) {
            *count += 1;
        } else {
            self.untracked += 1;
        }
    }

    fn merge(&mut self, other: Self) {
        for (count, other_count) in self.counts.iter_mut().zip(other.counts) {
            *count += other_count;
        }
        self.untracked += other.untracked;
    }

    fn print(&self) {
        let samples = self
            .counts
            .iter()
            .enumerate()
            .filter(|(_, count)| **count != 0)
            .map(|(cpu, count)| format!("CPU {cpu}: {count}"))
            .collect::<Vec<_>>();
        println!("  -> CPU placement samples: {}", samples.join(", "));
        if self.untracked != 0 {
            println!("  -> Untracked CPU samples: {}", self.untracked);
        }
    }
}

#[cfg(target_os = "macos")]
fn current_cpu() -> Option<usize> {
    let mut cpu = 0;
    // SAFETY: `cpu` is a valid writable `usize` and the system function does
    // not retain the pointer after returning.
    (unsafe { pthread_cpu_number_np(&mut cpu) } == 0).then_some(cpu)
}

#[cfg(not(target_os = "macos"))]
fn current_cpu() -> Option<usize> {
    None
}

#[derive(Debug, Copy, Clone)]
pub struct BenchmarkConfig {
    pub workers: usize,
    duration_secs: u64,
    batch_size: usize,
    sample_cpus: bool,
    payload_bytes: usize,
    operation: BenchmarkOperation,
}

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
enum BenchmarkOperation {
    All,
    FireAndForget,
    Fast,
    Unary,
    OwnedUnary,
    Stream,
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
            sample_cpus: parse_bool_env("NYRING_BENCH_CPU_SAMPLES")?,
            payload_bytes: parse_payload_env()?,
            operation: parse_operation_env()?,
        })
    }

    fn payload(self) -> Vec<u8> {
        vec![42u8; self.payload_bytes]
    }

    pub fn runs_fire_and_forget(self) -> bool {
        matches!(
            self.operation,
            BenchmarkOperation::All | BenchmarkOperation::FireAndForget
        )
    }

    pub fn runs_fast(self) -> bool {
        matches!(
            self.operation,
            BenchmarkOperation::All | BenchmarkOperation::Fast
        )
    }

    pub fn runs_unary(self) -> bool {
        matches!(
            self.operation,
            BenchmarkOperation::All | BenchmarkOperation::Unary
        )
    }

    pub fn runs_owned(self) -> bool {
        matches!(
            self.operation,
            BenchmarkOperation::All | BenchmarkOperation::OwnedUnary
        )
    }

    pub fn runs_stream(self) -> bool {
        matches!(
            self.operation,
            BenchmarkOperation::All | BenchmarkOperation::Stream
        )
    }
}

fn parse_operation_env() -> Result<BenchmarkOperation, Box<dyn std::error::Error>> {
    let Some(value) = std::env::var_os("NYRING_BENCH_OPERATION") else {
        return Ok(BenchmarkOperation::All);
    };
    match value.to_string_lossy().as_ref() {
        "all" => Ok(BenchmarkOperation::All),
        "fire" => Ok(BenchmarkOperation::FireAndForget),
        "fast" => Ok(BenchmarkOperation::Fast),
        "unary" => Ok(BenchmarkOperation::Unary),
        "owned" => Ok(BenchmarkOperation::OwnedUnary),
        "stream" => Ok(BenchmarkOperation::Stream),
        _ => Err(
            "NYRING_BENCH_OPERATION must be one of: all, fire, fast, unary, owned, stream".into(),
        ),
    }
}

/// Payload size in bytes; zero is a valid (and the default) size.
fn parse_payload_env() -> Result<usize, Box<dyn std::error::Error>> {
    let Some(value) = std::env::var_os("NYRING_BENCH_PAYLOAD_BYTES") else {
        return Ok(0);
    };
    Ok(value.to_string_lossy().parse::<usize>()?)
}

fn parse_bool_env(name: &str) -> Result<bool, Box<dyn std::error::Error>> {
    let Some(value) = std::env::var_os(name) else {
        return Ok(false);
    };
    match value.to_string_lossy().as_ref() {
        "1" | "true" => Ok(true),
        "0" | "false" => Ok(false),
        _ => Err(format!("{name} must be one of: 0, 1, false, true").into()),
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

    let payload = config.payload();
    println!("  -> Payload Size: {}", payload.len());

    for _ in 0..config.workers {
        let plugin = plugin.clone();
        let payload = payload.clone();
        let counter = total_requests.clone();
        let latency_counter = total_latency_nanos.clone();
        let start_signal = start_signal.clone();

        let handle = tokio::spawn(async move {
            // Wait for signal
            start_signal.notified().await;

            let start_time = Instant::now();
            let bench_duration = Duration::from_secs(config.duration_secs);
            let mut cpu_samples = CpuSamples::default();
            let mut completed_batches = 0;

            while start_time.elapsed() < bench_duration {
                let batch_start = Instant::now();
                // Await each call directly: these plugin ops complete
                // synchronously, so batching futures through join_all only
                // added a large per-batch allocation to the measurement.
                // Sequential awaits also pin the in-flight depth per worker
                // to exactly one, and every counted call must have succeeded.
                for _ in 0..config.batch_size {
                    let status = plugin
                        .call("benchmark_without_response", &payload)
                        .await
                        .expect("benchmarked call failed");
                    assert_eq!(status, NrStatus::Ok, "benchmarked call was not Ok");
                }
                let batch_elapsed = batch_start.elapsed();

                counter.fetch_add(config.batch_size as u64, Ordering::Relaxed);
                latency_counter.fetch_add(batch_elapsed.as_nanos() as u64, Ordering::Relaxed);
                if config.sample_cpus && completed_batches % CPU_SAMPLE_BATCH_INTERVAL == 0 {
                    cpu_samples.record_current();
                }
                completed_batches += 1;
            }
            cpu_samples
        });
        handles.push(handle);
    }

    // Warmup / Sync time
    tokio::time::sleep(Duration::from_millis(100)).await;

    let start_time = Instant::now();
    start_signal.notify_waiters();

    let mut cpu_samples = CpuSamples::default();
    for h in handles {
        if let Ok(samples) = h.await {
            cpu_samples.merge(samples);
        }
    }

    let elapsed = start_time.elapsed();
    let total = total_requests.load(Ordering::Relaxed);
    let total_lat_nanos = total_latency_nanos.load(Ordering::Relaxed);

    let rps = total as f64 / elapsed.as_secs_f64();
    let avg_latency_nanos = total_lat_nanos.checked_div(total).unwrap_or(0);

    println!("  -> Processed {} requests in {:.2?}", total, elapsed);
    println!("  -> RPS: {:.2}/sec", rps);
    println!("  -> Average latency: {:.2} ns/request", avg_latency_nanos);
    if config.sample_cpus {
        cpu_samples.print();
    }
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

    let payload = config.payload();
    println!("  -> Payload Size: {}", payload.len());

    for _ in 0..config.workers {
        let plugin = plugin.clone();
        let payload = payload.clone();
        let counter = total_requests.clone();
        let latency_counter = total_latency_nanos.clone();
        let start_signal = start_signal.clone();

        let handle = tokio::spawn(async move {
            // Wait for signal
            start_signal.notified().await;

            let start_time = Instant::now();
            let bench_duration = Duration::from_secs(config.duration_secs);
            let mut cpu_samples = CpuSamples::default();
            let mut completed_batches = 0;

            while start_time.elapsed() < bench_duration {
                let batch_start = Instant::now();
                // Await each call directly; see run_fire_and_forget_benchmark.
                for _ in 0..config.batch_size {
                    let (status, _data) = plugin
                        .call_response("benchmark", &payload)
                        .await
                        .expect("benchmarked call failed");
                    assert_eq!(status, NrStatus::Ok, "benchmarked call was not Ok");
                }
                let batch_elapsed = batch_start.elapsed();

                counter.fetch_add(config.batch_size as u64, Ordering::Relaxed);
                latency_counter.fetch_add(batch_elapsed.as_nanos() as u64, Ordering::Relaxed);
                if config.sample_cpus && completed_batches % CPU_SAMPLE_BATCH_INTERVAL == 0 {
                    cpu_samples.record_current();
                }
                completed_batches += 1;
            }
            cpu_samples
        });
        handles.push(handle);
    }

    // Warmup / Sync time
    tokio::time::sleep(Duration::from_millis(100)).await;

    let start_time = Instant::now();
    start_signal.notify_waiters();

    let mut cpu_samples = CpuSamples::default();
    for h in handles {
        if let Ok(samples) = h.await {
            cpu_samples.merge(samples);
        }
    }

    let elapsed = start_time.elapsed();
    let total = total_requests.load(Ordering::Relaxed);
    let total_lat_nanos = total_latency_nanos.load(Ordering::Relaxed);

    let rps = total as f64 / elapsed.as_secs_f64();
    let avg_latency_nanos = total_lat_nanos.checked_div(total).unwrap_or(0);

    println!("  -> Processed {} requests in {:.2?}", total, elapsed);
    println!("  -> RPS: {:.2}/sec", rps);
    println!("  -> Average latency: {:.2} ns/request", avg_latency_nanos);
    if config.sample_cpus {
        cpu_samples.print();
    }
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

    let payload = config.payload();
    println!("  -> Payload Size: {}", payload.len());

    for _ in 0..config.workers {
        let plugin = plugin.clone();
        let payload = payload.clone();
        let counter = total_requests.clone();
        let latency_counter = total_latency_nanos.clone();
        let start_signal = start_signal.clone();

        let handle = tokio::spawn(async move {
            // Wait for signal
            start_signal.notified().await;

            let start_time = Instant::now();
            let bench_duration = Duration::from_secs(config.duration_secs);
            let mut cpu_samples = CpuSamples::default();
            let mut completed_batches = 0;

            while start_time.elapsed() < bench_duration {
                let batch_start = Instant::now();
                // Await each call directly; see run_fire_and_forget_benchmark.
                for _ in 0..config.batch_size {
                    let (status, _data) = plugin
                        .call_response_fast("benchmark", &payload)
                        .await
                        .expect("benchmarked call failed");
                    assert_eq!(status, NrStatus::Ok, "benchmarked call was not Ok");
                }
                let batch_elapsed = batch_start.elapsed();

                counter.fetch_add(config.batch_size as u64, Ordering::Relaxed);
                latency_counter.fetch_add(batch_elapsed.as_nanos() as u64, Ordering::Relaxed);
                if config.sample_cpus && completed_batches % CPU_SAMPLE_BATCH_INTERVAL == 0 {
                    cpu_samples.record_current();
                }
                completed_batches += 1;
            }
            cpu_samples
        });
        handles.push(handle);
    }

    // Warmup / Sync time
    tokio::time::sleep(Duration::from_millis(100)).await;

    let start_time = Instant::now();
    start_signal.notify_waiters();

    let mut cpu_samples = CpuSamples::default();
    for h in handles {
        if let Ok(samples) = h.await {
            cpu_samples.merge(samples);
        }
    }

    let elapsed = start_time.elapsed();
    let total = total_requests.load(Ordering::Relaxed);
    let total_lat_nanos = total_latency_nanos.load(Ordering::Relaxed);

    let rps = total as f64 / elapsed.as_secs_f64();
    let avg_latency_nanos = total_lat_nanos.checked_div(total).unwrap_or(0);

    println!("  -> Processed {} requests in {:.2?}", total, elapsed);
    println!("  -> RPS: {:.2}/sec", rps);
    println!("  -> Average latency: {:.2} ns/request", avg_latency_nanos);
    if config.sample_cpus {
        cpu_samples.print();
    }
}

/// Run an ABI v2 owned-response benchmark: the plugin answers with
/// `payload.len()` bytes borrowed from a static slab and the host consumes
/// them zero-copy through `call_response_bytes`.
pub async fn run_owned_response_benchmark(plugin: PluginHandle, config: BenchmarkConfig) {
    println!("\n--- Benchmark: Request-Response Owned (ABI v2) ---");

    let mut handles = Vec::with_capacity(config.workers);
    let total_requests = Arc::new(AtomicU64::new(0));
    let total_latency_nanos = Arc::new(AtomicU64::new(0));
    let start_signal = Arc::new(tokio::sync::Notify::new());

    println!("  -> Using {} threads", config.workers);
    println!("  -> Using {} requests per batch", config.batch_size);
    println!("  -> Using {} seconds for benchmark", config.duration_secs);

    let payload = config.payload();
    println!("  -> Payload Size: {}", payload.len());

    for _ in 0..config.workers {
        let plugin = plugin.clone();
        let payload = payload.clone();
        let counter = total_requests.clone();
        let latency_counter = total_latency_nanos.clone();
        let start_signal = start_signal.clone();

        let handle = tokio::spawn(async move {
            start_signal.notified().await;

            let start_time = Instant::now();
            let bench_duration = Duration::from_secs(config.duration_secs);
            let mut cpu_samples = CpuSamples::default();
            let mut completed_batches = 0;

            while start_time.elapsed() < bench_duration {
                let batch_start = Instant::now();
                // Await each call directly; see run_fire_and_forget_benchmark.
                for _ in 0..config.batch_size {
                    let (status, data) = plugin
                        .call_response_bytes("benchmark_owned", &payload)
                        .await
                        .expect("benchmarked call failed");
                    assert_eq!(status, NrStatus::Ok, "benchmarked call was not Ok");
                    assert_eq!(data.len(), payload.len(), "owned response length mismatch");
                }
                let batch_elapsed = batch_start.elapsed();

                counter.fetch_add(config.batch_size as u64, Ordering::Relaxed);
                latency_counter.fetch_add(batch_elapsed.as_nanos() as u64, Ordering::Relaxed);
                if config.sample_cpus && completed_batches % CPU_SAMPLE_BATCH_INTERVAL == 0 {
                    cpu_samples.record_current();
                }
                completed_batches += 1;
            }
            cpu_samples
        });
        handles.push(handle);
    }

    tokio::time::sleep(Duration::from_millis(100)).await;

    let start_time = Instant::now();
    start_signal.notify_waiters();

    let mut cpu_samples = CpuSamples::default();
    for h in handles {
        if let Ok(samples) = h.await {
            cpu_samples.merge(samples);
        }
    }

    let elapsed = start_time.elapsed();
    let total = total_requests.load(Ordering::Relaxed);
    let total_lat_nanos = total_latency_nanos.load(Ordering::Relaxed);

    let rps = total as f64 / elapsed.as_secs_f64();
    let avg_latency_nanos = total_lat_nanos.checked_div(total).unwrap_or(0);

    println!("  -> Processed {} requests in {:.2?}", total, elapsed);
    println!("  -> RPS: {:.2}/sec", rps);
    println!("  -> Average latency: {:.2} ns/request", avg_latency_nanos);
    if config.sample_cpus {
        cpu_samples.print();
    }
}

/// Run a streaming benchmark: each iteration is one full stream round trip
/// of 8 empty data frames plus StreamEnd from the `benchmark_stream` entry.
/// Throughput is reported in frames per second (9 frames per stream).
pub async fn run_stream_benchmark(plugin: PluginHandle, config: BenchmarkConfig) {
    // Data frames per stream come from NYRING_BENCH_STREAM_FRAMES (default 8,
    // must stay below the stream capacity of 64); the example plugin reads
    // the same variable, so the expected count stays in sync. +1 for the
    // terminal StreamEnd frame.
    let frames_per_stream: u64 = 1 + std::env::var("NYRING_BENCH_STREAM_FRAMES")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(8);

    println!("\n--- Benchmark: Streaming ---");

    let mut handles = Vec::with_capacity(config.workers);
    let total_frames = Arc::new(AtomicU64::new(0));
    let total_latency_nanos = Arc::new(AtomicU64::new(0));
    let start_signal = Arc::new(tokio::sync::Notify::new());

    println!("  -> Using {} threads", config.workers);
    println!("  -> Using {} streams per batch", config.batch_size);
    println!("  -> Using {} seconds for benchmark", config.duration_secs);
    println!("  -> Frames per stream: {}", frames_per_stream);

    let payload = config.payload();
    println!("  -> Payload Size: {}", payload.len());

    for _ in 0..config.workers {
        let plugin = plugin.clone();
        let payload = payload.clone();
        let counter = total_frames.clone();
        let latency_counter = total_latency_nanos.clone();
        let start_signal = start_signal.clone();

        let handle = tokio::spawn(async move {
            // Wait for signal
            start_signal.notified().await;

            let start_time = Instant::now();
            let bench_duration = Duration::from_secs(config.duration_secs);
            let mut cpu_samples = CpuSamples::default();
            let mut completed_batches = 0;

            while start_time.elapsed() < bench_duration {
                let batch_start = Instant::now();
                for _ in 0..config.batch_size {
                    let (_sid, mut receiver) = plugin
                        .call_stream("benchmark_stream", &payload)
                        .await
                        .expect("benchmarked stream failed to start");
                    let mut frames = 0u64;
                    while let Some(frame) = receiver.recv().await {
                        assert!(
                            frame.status == NrStatus::Ok || frame.status == NrStatus::StreamEnd,
                            "benchmarked stream frame was not Ok/StreamEnd"
                        );
                        frames += 1;
                    }
                    assert_eq!(frames, frames_per_stream, "stream frame count mismatch");
                }
                let batch_elapsed = batch_start.elapsed();

                counter.fetch_add(
                    config.batch_size as u64 * frames_per_stream,
                    Ordering::Relaxed,
                );
                latency_counter.fetch_add(batch_elapsed.as_nanos() as u64, Ordering::Relaxed);
                if config.sample_cpus && completed_batches % CPU_SAMPLE_BATCH_INTERVAL == 0 {
                    cpu_samples.record_current();
                }
                completed_batches += 1;
            }
            cpu_samples
        });
        handles.push(handle);
    }

    // Warmup / Sync time
    tokio::time::sleep(Duration::from_millis(100)).await;

    let start_time = Instant::now();
    start_signal.notify_waiters();

    let mut cpu_samples = CpuSamples::default();
    for h in handles {
        if let Ok(samples) = h.await {
            cpu_samples.merge(samples);
        }
    }

    let elapsed = start_time.elapsed();
    let total = total_frames.load(Ordering::Relaxed);
    let total_lat_nanos = total_latency_nanos.load(Ordering::Relaxed);

    let fps = total as f64 / elapsed.as_secs_f64();
    let avg_latency_nanos = total_lat_nanos.checked_div(total).unwrap_or(0);

    println!("  -> Processed {} frames in {:.2?}", total, elapsed);
    println!("  -> Frames/s: {:.2}/sec", fps);
    println!("  -> Average latency: {:.2} ns/frame", avg_latency_nanos);
    if config.sample_cpus {
        cpu_samples.print();
    }
}
