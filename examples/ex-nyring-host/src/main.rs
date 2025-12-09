use futures::future::join_all;
use nylon_ring_host::NylonRingHost;
use std::sync::Arc;
use std::{
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, Instant},
};

// --- CONFIGURATION ---
const DURATION_SECS: u64 = 10;
// BATCH_SIZE: used for Unary/Standard tests to create pipelining without spawn overhead
const BATCH_SIZE: usize = 100;

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
    let host = Arc::new(NylonRingHost::load(plugin_path).expect("Failed to load plugin"));

    // Demo 1: Echo
    println!("--- Demo 1: Echo ---");
    let message = b"Hello!";
    println!("Sending: {}", String::from_utf8_lossy(message));
    let now = std::time::Instant::now();
    let (status, response) = host.call_response("echo", message).await?;
    println!("round trip time: {:?}", now.elapsed());
    println!("Status: {:?}", status);
    println!(
        "Response: {}\n",
        String::from_utf8_lossy(response.as_slice())
    );

    // /*
    // Demo 2: Uppercase
    println!("--- Demo 2: Uppercase ---");
    let message = b"make me loud";
    println!("Sending: {}", String::from_utf8_lossy(message));

    let (status, response) = host.call_response("uppercase", message).await?;
    println!("Status: {:?}", status);
    println!(
        "Response: {}\n",
        String::from_utf8_lossy(response.as_slice())
    );

    // Demo 3: Multiple calls
    println!("--- Demo 3: Multiple Calls ---");
    for i in 1..=5 {
        let message = format!("Message #{}", i);
        let (status, _) = host.call_response("echo", message.as_bytes()).await?;
        println!("Call {}: {:?}", i, status);
    }

    // Demo 4: Benchmark
    println!("\n--- Demo 4: Benchmark ---");
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
    // let payload: &'static [u8] = b"66125646655438184824034357503490176636099264991633465762201498014519123891859268733983653039388726432642995143358504569007771
    // 58598693402496866943402835041634570224118066330404568236483221494076492917098844866249914290879929866424562331479470484929530
    // 47981071980750177177087538144356263522627349597567256092672809627220185268573884037546233149941048425721886017397002493771038
    // 59789493522946388742872159309483907924798646897590296799087138432035293041592297258616156208443607672462374144231313952523825
    // 41214722436789521357506910806784385239131212667915286065697223577192349536631069819291852420161751071280762096700317526464632
    // 90928765621229518421461199169418959317189370377096223039048075197848769839858594855143546758093458201630388955491473164903161
    // 19029733685356457419092050823362333977133993758927393621966880365414110809808625711116204972494708604941468381375412202718800
    // 30757276143464395289644876909915866493212206250053550400385293673376701537468360960764657913786708380781323834871961191069325
    // 5294339716425075";
    let payload: &'static [u8] = b"";
    println!("Payload Size: {}", payload.len());
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

    // Demo 5: Benchmark
    println!("\n--- Demo 5: Benchmark ---");
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
    println!("Payload Size: {}", payload.len());
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
    // */
    println!("\n=== Demo Complete ===");
    Ok(())
}
