use nylon_ring_host::NylonRingHost;

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
        ])
        .status()?;

    if !build_status.success() {
        eprintln!("Failed to build plugin");
        return Err("Plugin build failed".into());
    }

    // Load the plugin
    let plugin_path = if cfg!(target_os = "macos") {
        "target/debug/libex_nyring_plugin.dylib"
    } else if cfg!(target_os = "windows") {
        "target/debug/ex_nyring_plugin.dll"
    } else {
        "target/debug/libex_nyring_plugin.so"
    };

    println!("Loading plugin from: {}\n", plugin_path);
    let host = NylonRingHost::load(plugin_path)?;

    // Demo 1: Echo
    println!("--- Demo 1: Echo ---");
    let message = b"Hello, Nylon Ring!";
    println!("Sending: {}", String::from_utf8_lossy(message));

    let (status, response) = host.call("echo", message).await?;
    println!("Status: {:?}", status);
    println!("Response: {}\n", String::from_utf8_lossy(&response));

    // Demo 2: Uppercase
    println!("--- Demo 2: Uppercase ---");
    let message = b"make me loud";
    println!("Sending: {}", String::from_utf8_lossy(message));

    let (status, response) = host.call("uppercase", message).await?;
    println!("Status: {:?}", status);
    println!("Response: {}\n", String::from_utf8_lossy(&response));

    // Demo 3: Multiple calls
    println!("--- Demo 3: Multiple Calls ---");
    for i in 1..=5 {
        let message = format!("Message #{}", i);
        let (status, _) = host.call("echo", message.as_bytes()).await?;
        println!("Call {}: {:?}", i, status);
    }

    println!("\n=== Demo Complete ===");
    Ok(())
}
