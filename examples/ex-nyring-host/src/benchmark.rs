use nylon_ring_host::{NrStatus, NylonRingHost, PluginHandle};
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
    FireById,
    Fast,
    FastById,
    Unary,
    UnaryById,
    OwnedUnary,
    LeaseUnary,
    Stream,
    AsyncUnary,
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

    pub fn runs_fire_by_id(self) -> bool {
        matches!(
            self.operation,
            BenchmarkOperation::All | BenchmarkOperation::FireById
        )
    }

    pub fn runs_fast(self) -> bool {
        matches!(
            self.operation,
            BenchmarkOperation::All | BenchmarkOperation::Fast
        )
    }

    pub fn runs_fast_by_id(self) -> bool {
        matches!(
            self.operation,
            BenchmarkOperation::All | BenchmarkOperation::FastById
        )
    }

    pub fn runs_unary(self) -> bool {
        matches!(
            self.operation,
            BenchmarkOperation::All | BenchmarkOperation::Unary
        )
    }

    pub fn runs_unary_by_id(self) -> bool {
        matches!(
            self.operation,
            BenchmarkOperation::All | BenchmarkOperation::UnaryById
        )
    }

    pub fn runs_owned(self) -> bool {
        matches!(
            self.operation,
            BenchmarkOperation::All | BenchmarkOperation::OwnedUnary
        )
    }

    pub fn runs_lease(self) -> bool {
        matches!(
            self.operation,
            BenchmarkOperation::All | BenchmarkOperation::LeaseUnary
        )
    }

    pub fn runs_stream(self) -> bool {
        matches!(
            self.operation,
            BenchmarkOperation::All | BenchmarkOperation::Stream
        )
    }

    pub fn runs_async_unary(self) -> bool {
        matches!(
            self.operation,
            BenchmarkOperation::All | BenchmarkOperation::AsyncUnary
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
        "fireid" => Ok(BenchmarkOperation::FireById),
        "fast" => Ok(BenchmarkOperation::Fast),
        "fastid" => Ok(BenchmarkOperation::FastById),
        "unary" => Ok(BenchmarkOperation::Unary),
        "unaryid" => Ok(BenchmarkOperation::UnaryById),
        "owned" => Ok(BenchmarkOperation::OwnedUnary),
        "lease" => Ok(BenchmarkOperation::LeaseUnary),
        "stream" => Ok(BenchmarkOperation::Stream),
        "async" => Ok(BenchmarkOperation::AsyncUnary),
        _ => Err(
            "NYRING_BENCH_OPERATION must be one of: all, fire, fireid, fast, fastid, \
                  unary, unaryid, owned, lease, stream, async"
                .into(),
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

/// Run a fire-and-forget benchmark (calls without waiting for response).
/// With `by_id`, calls dispatch through a pre-resolved integer entry id.
pub async fn run_fire_and_forget_benchmark(
    plugin: PluginHandle,
    config: BenchmarkConfig,
    by_id: bool,
) {
    if by_id {
        println!("\n--- Benchmark: Fire-and-Forget (by entry id) ---");
    } else {
        println!("\n--- Benchmark: Fire-and-Forget ---");
    }

    let mut handles = Vec::with_capacity(config.workers);
    let total_requests = Arc::new(AtomicU64::new(0));
    let total_latency_nanos = Arc::new(AtomicU64::new(0));
    let start_signal = Arc::new(tokio::sync::Notify::new());

    println!("  -> Using {} threads", config.workers);
    println!("  -> Using {} requests per batch", config.batch_size);
    println!("  -> Using {} seconds for benchmark", config.duration_secs);

    let payload = config.payload();
    println!("  -> Payload Size: {}", payload.len());

    let entry = by_id.then(|| {
        plugin
            .entry("benchmark_without_response")
            .expect("entry resolution failed")
    });

    for _ in 0..config.workers {
        let plugin = plugin.clone();
        let entry = entry.clone();
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
            let mut local_requests = 0u64;
            let mut local_latency_nanos = 0u64;

            while start_time.elapsed() < bench_duration {
                let batch_start = Instant::now();
                // Await each call directly: these plugin ops complete
                // synchronously, so batching futures through join_all only
                // added a large per-batch allocation to the measurement.
                // Sequential awaits also pin the in-flight depth per worker
                // to exactly one, and every counted call must have succeeded.
                if let Some(entry) = &entry {
                    for _ in 0..config.batch_size {
                        let status = entry.call(&payload).await.expect("benchmarked call failed");
                        assert_eq!(status, NrStatus::Ok, "benchmarked call was not Ok");
                    }
                } else {
                    for _ in 0..config.batch_size {
                        let status = plugin
                            .call("benchmark_without_response", &payload)
                            .await
                            .expect("benchmarked call failed");
                        assert_eq!(status, NrStatus::Ok, "benchmarked call was not Ok");
                    }
                }
                let batch_elapsed = batch_start.elapsed();

                local_requests += config.batch_size as u64;
                local_latency_nanos += batch_elapsed.as_nanos() as u64;
                if config.sample_cpus && completed_batches % CPU_SAMPLE_BATCH_INTERVAL == 0 {
                    cpu_samples.record_current();
                }
                completed_batches += 1;
            }
            // Flush once per worker: per-batch RMWs on the shared counters
            // serialize workers once batches drop below a microsecond.
            counter.fetch_add(local_requests, Ordering::Relaxed);
            latency_counter.fetch_add(local_latency_nanos, Ordering::Relaxed);
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

/// Run a request-response benchmark.
/// With `by_id`, calls dispatch through a pre-resolved integer entry id.
pub async fn run_request_response_benchmark(
    plugin: PluginHandle,
    config: BenchmarkConfig,
    by_id: bool,
) {
    if by_id {
        println!("\n--- Benchmark: Request-Response (by entry id) ---");
    } else {
        println!("\n--- Benchmark: Request-Response ---");
    }

    let mut handles = Vec::with_capacity(config.workers);
    let total_requests = Arc::new(AtomicU64::new(0));
    let total_latency_nanos = Arc::new(AtomicU64::new(0));
    let start_signal = Arc::new(tokio::sync::Notify::new());

    println!("  -> Using {} threads", config.workers);
    println!("  -> Using {} requests per batch", config.batch_size);
    println!("  -> Using {} seconds for benchmark", config.duration_secs);

    let payload = config.payload();
    println!("  -> Payload Size: {}", payload.len());

    let entry = by_id.then(|| plugin.entry("benchmark").expect("entry resolution failed"));

    for _ in 0..config.workers {
        let plugin = plugin.clone();
        let entry = entry.clone();
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
            let mut local_requests = 0u64;
            let mut local_latency_nanos = 0u64;

            while start_time.elapsed() < bench_duration {
                let batch_start = Instant::now();
                // Await each call directly; see run_fire_and_forget_benchmark.
                if let Some(entry) = &entry {
                    for _ in 0..config.batch_size {
                        let (status, _data) = entry
                            .call_response(&payload)
                            .await
                            .expect("benchmarked call failed");
                        assert_eq!(status, NrStatus::Ok, "benchmarked call was not Ok");
                    }
                } else {
                    for _ in 0..config.batch_size {
                        let (status, _data) = plugin
                            .call_response("benchmark", &payload)
                            .await
                            .expect("benchmarked call failed");
                        assert_eq!(status, NrStatus::Ok, "benchmarked call was not Ok");
                    }
                }
                let batch_elapsed = batch_start.elapsed();

                local_requests += config.batch_size as u64;
                local_latency_nanos += batch_elapsed.as_nanos() as u64;
                if config.sample_cpus && completed_batches % CPU_SAMPLE_BATCH_INTERVAL == 0 {
                    cpu_samples.record_current();
                }
                completed_batches += 1;
            }
            // Flush once per worker: per-batch RMWs on the shared counters
            // serialize workers once batches drop below a microsecond.
            counter.fetch_add(local_requests, Ordering::Relaxed);
            latency_counter.fetch_add(local_latency_nanos, Ordering::Relaxed);
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

/// Run a request-response fast benchmark.
/// With `by_id`, calls dispatch through a pre-resolved integer entry id.
pub async fn run_request_response_fast_benchmark(
    plugin: PluginHandle,
    config: BenchmarkConfig,
    by_id: bool,
) {
    if by_id {
        println!("\n--- Benchmark: Request-Response Fast (by entry id) ---");
    } else {
        println!("\n--- Benchmark: Request-Response Fast ---");
    }

    let mut handles = Vec::with_capacity(config.workers);
    let total_requests = Arc::new(AtomicU64::new(0));
    let total_latency_nanos = Arc::new(AtomicU64::new(0));
    let start_signal = Arc::new(tokio::sync::Notify::new());

    println!("  -> Using {} threads", config.workers);
    println!("  -> Using {} requests per batch", config.batch_size);
    println!("  -> Using {} seconds for benchmark", config.duration_secs);

    let payload = config.payload();
    println!("  -> Payload Size: {}", payload.len());

    let entry = by_id.then(|| plugin.entry("benchmark").expect("entry resolution failed"));

    for _ in 0..config.workers {
        let plugin = plugin.clone();
        let entry = entry.clone();
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
            let mut local_requests = 0u64;
            let mut local_latency_nanos = 0u64;

            while start_time.elapsed() < bench_duration {
                let batch_start = Instant::now();
                // Await each call directly; see run_fire_and_forget_benchmark.
                if let Some(entry) = &entry {
                    for _ in 0..config.batch_size {
                        let (status, _data) = entry
                            .call_response_fast(&payload)
                            .await
                            .expect("benchmarked call failed");
                        assert_eq!(status, NrStatus::Ok, "benchmarked call was not Ok");
                    }
                } else {
                    for _ in 0..config.batch_size {
                        let (status, _data) = plugin
                            .call_response_fast("benchmark", &payload)
                            .await
                            .expect("benchmarked call failed");
                        assert_eq!(status, NrStatus::Ok, "benchmarked call was not Ok");
                    }
                }
                let batch_elapsed = batch_start.elapsed();

                local_requests += config.batch_size as u64;
                local_latency_nanos += batch_elapsed.as_nanos() as u64;
                if config.sample_cpus && completed_batches % CPU_SAMPLE_BATCH_INTERVAL == 0 {
                    cpu_samples.record_current();
                }
                completed_batches += 1;
            }
            // Flush once per worker: per-batch RMWs on the shared counters
            // serialize workers once batches drop below a microsecond.
            counter.fetch_add(local_requests, Ordering::Relaxed);
            latency_counter.fetch_add(local_latency_nanos, Ordering::Relaxed);
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

/// Run an owned-response benchmark: the plugin answers with
/// `payload.len()` bytes borrowed from a static slab and the host consumes
/// them zero-copy through `call_response_bytes`.
pub async fn run_owned_response_benchmark(plugin: PluginHandle, config: BenchmarkConfig) {
    println!("\n--- Benchmark: Request-Response Owned ---");

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
            let mut local_requests = 0u64;
            let mut local_latency_nanos = 0u64;

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

                local_requests += config.batch_size as u64;
                local_latency_nanos += batch_elapsed.as_nanos() as u64;
                if config.sample_cpus && completed_batches % CPU_SAMPLE_BATCH_INTERVAL == 0 {
                    cpu_samples.record_current();
                }
                completed_batches += 1;
            }
            // Flush once per worker: per-batch RMWs on the shared counters
            // serialize workers once batches drop below a microsecond.
            counter.fetch_add(local_requests, Ordering::Relaxed);
            latency_counter.fetch_add(local_latency_nanos, Ordering::Relaxed);
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

/// Run a lease benchmark: the plugin echoes the payload by writing
/// straight into a host-leased buffer, so the response reaches the plain
/// `call_response` Vec API with no extra alloc/copy pair.
pub async fn run_lease_response_benchmark(plugin: PluginHandle, config: BenchmarkConfig) {
    println!("\n--- Benchmark: Request-Response Lease ---");

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
            let mut local_requests = 0u64;
            let mut local_latency_nanos = 0u64;

            while start_time.elapsed() < bench_duration {
                let batch_start = Instant::now();
                // Await each call directly; see run_fire_and_forget_benchmark.
                for _ in 0..config.batch_size {
                    let (status, data) = plugin
                        .call_response("echo_lease", &payload)
                        .await
                        .expect("benchmarked call failed");
                    assert_eq!(status, NrStatus::Ok, "benchmarked call was not Ok");
                    assert_eq!(data.len(), payload.len(), "lease response length mismatch");
                }
                let batch_elapsed = batch_start.elapsed();

                local_requests += config.batch_size as u64;
                local_latency_nanos += batch_elapsed.as_nanos() as u64;
                if config.sample_cpus && completed_batches % CPU_SAMPLE_BATCH_INTERVAL == 0 {
                    cpu_samples.record_current();
                }
                completed_batches += 1;
            }
            // Flush once per worker: per-batch RMWs on the shared counters
            // serialize workers once batches drop below a microsecond.
            counter.fetch_add(local_requests, Ordering::Relaxed);
            latency_counter.fetch_add(local_latency_nanos, Ordering::Relaxed);
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
            let mut local_requests = 0u64;
            let mut local_latency_nanos = 0u64;

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

                local_requests += config.batch_size as u64 * frames_per_stream;
                local_latency_nanos += batch_elapsed.as_nanos() as u64;
                if config.sample_cpus && completed_batches % CPU_SAMPLE_BATCH_INTERVAL == 0 {
                    cpu_samples.record_current();
                }
                completed_batches += 1;
            }
            // Flush once per worker: per-batch RMWs on the shared counters
            // serialize workers once batches drop below a microsecond.
            counter.fetch_add(local_requests, Ordering::Relaxed);
            latency_counter.fetch_add(local_latency_nanos, Ordering::Relaxed);
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

/// Builds the trait example plugin and returns its dylib path (relative to
/// the workspace root, like the paths in `main.rs`).
fn trait_plugin_path() -> Option<&'static str> {
    let status = std::process::Command::new("cargo")
        .args([
            "build",
            "--manifest-path",
            "examples/ex-nyring-trait-plugin/Cargo.toml",
            "-r",
        ])
        .status()
        .ok()?;
    if !status.success() {
        return None;
    }
    Some(if cfg!(target_os = "macos") {
        "target/release/libex_nyring_trait_plugin.dylib"
    } else if cfg!(target_os = "windows") {
        "target/release/ex_nyring_trait_plugin.dll"
    } else {
        "target/release/libex_nyring_trait_plugin.so"
    })
}

/// Async-unary benchmark: drives the trait example's `async_echo` entry.
/// The handler is an `async fn`; its future is ready on the first poll, so
/// this measures the async machinery itself (session payload copy, future
/// allocation, one poll, inline delivery) on top of the standard unary
/// path — not the executor. Loads the trait plugin on demand, honoring
/// `NYRING_BENCH_PINNED` like the main plugin.
pub async fn run_async_echo_benchmark(host: &mut NylonRingHost, config: BenchmarkConfig) {
    println!("\n--- Benchmark: Async Unary (trait plugin async_echo) ---");

    let Some(path) = trait_plugin_path() else {
        println!("  -> skipped: ex-nyring-trait-plugin failed to build");
        return;
    };
    let name = "async-bench";
    if host.plugin(name).is_none() {
        let loaded = if std::env::var("NYRING_BENCH_PINNED").is_ok_and(|value| value == "1") {
            host.load_pinned(name, path)
        } else {
            host.load(name, path)
        };
        if let Err(error) = loaded {
            println!("  -> skipped: load failed ({error})");
            return;
        }
    }
    let plugin = host.plugin(name).expect("trait plugin was loaded");

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
            let mut local_requests = 0u64;
            let mut local_latency_nanos = 0u64;

            while start_time.elapsed() < bench_duration {
                let batch_start = Instant::now();
                for _ in 0..config.batch_size {
                    let (status, _data) = plugin
                        .call_response("async_echo", &payload)
                        .await
                        .expect("benchmarked call failed");
                    assert_eq!(status, NrStatus::Ok, "benchmarked call was not Ok");
                }
                local_requests += config.batch_size as u64;
                local_latency_nanos += batch_start.elapsed().as_nanos() as u64;
            }
            counter.fetch_add(local_requests, Ordering::Relaxed);
            latency_counter.fetch_add(local_latency_nanos, Ordering::Relaxed);
        });
        handles.push(handle);
    }

    tokio::time::sleep(Duration::from_millis(100)).await;
    let start_time = Instant::now();
    start_signal.notify_waiters();
    for handle in handles {
        let _ = handle.await;
    }

    let elapsed = start_time.elapsed();
    let total = total_requests.load(Ordering::Relaxed);
    let total_lat_nanos = total_latency_nanos.load(Ordering::Relaxed);
    let rps = total as f64 / elapsed.as_secs_f64();
    let avg_latency_nanos = total_lat_nanos.checked_div(total).unwrap_or(0);

    println!("  -> Processed {} requests in {:.2?}", total, elapsed);
    println!("  -> RPS: {:.2}/sec", rps);
    println!("  -> Average latency: {} ns/request", avg_latency_nanos);
}
