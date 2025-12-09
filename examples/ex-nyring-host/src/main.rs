mod benchmark;

use nylon_ring_host::NylonRingHost;
use std::sync::Arc;

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

    // Demo 4: Fire-and-Forget Benchmark
    benchmark::run_fire_and_forget_benchmark(host.clone()).await;

    // Demo 5: Request-Response Benchmark
    benchmark::run_request_response_benchmark(host.clone()).await;

    println!("\n=== Demo Complete ===");
    Ok(())
}
