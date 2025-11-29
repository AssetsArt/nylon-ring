// Example demonstrating loading and using a Go plugin with NylonRingHost
use nylon_ring::NrStatus;
use nylon_ring_host::{Extensions, HighLevelRequest, NylonRingHost, NylonRingHostError};
use std::env;
use std::path::PathBuf;

#[tokio::main]
async fn main() -> Result<(), NylonRingHostError> {
    // Determine the path to the Go plugin library
    let mut plugin_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    plugin_path.pop(); // Go up to workspace root
    plugin_path.push("target");
    plugin_path.push("go");
    
    // Try different extensions based on platform
    let extensions = if cfg!(target_os = "macos") {
        vec!["dylib", "so"]
    } else if cfg!(target_os = "linux") {
        vec!["so"]
    } else {
        vec!["dll", "so"]
    };
    
    let mut found = false;
    for ext in &extensions {
        let mut test_path = plugin_path.clone();
        test_path.push(format!("nylon_ring_go_plugin_simple.{}", ext));
        if test_path.exists() {
            plugin_path = test_path;
            found = true;
            break;
        }
    }
    
    if !found {
        // Fallback: try with first extension
        plugin_path.push(format!("nylon_ring_go_plugin_simple.{}", extensions[0]));
    }

    println!("Loading Go plugin from: {:?}", plugin_path);

    let plugin_path_str = plugin_path.to_str().ok_or_else(|| {
        NylonRingHostError::InvalidPluginPath(format!(
            "Path contains invalid UTF-8: {:?}",
            plugin_path
        ))
    })?;
    
    let host = NylonRingHost::load(plugin_path_str)?;
    println!("✓ Go plugin loaded successfully!");

    // Test unary call
    println!("\n=== Testing Unary Call ===");
    let req = HighLevelRequest {
        method: "GET".to_string(),
        path: "/hello".to_string(),
        query: "".to_string(),
        headers: vec![("User-Agent".to_string(), "NylonHost/1.0".to_string())],
        body: vec![],
        extensions: Extensions::new(),
    };

    println!("Sending unary request...");
    let (status, payload) = host.call("unary", req).await?;

    println!("Response received!");
    println!("Status: {:?}", status);
    println!("Payload: {}", String::from_utf8_lossy(&payload));

    // Test streaming call
    println!("\n=== Testing Streaming Call ===");
    let req = HighLevelRequest {
        method: "GET".to_string(),
        path: "/stream".to_string(),
        query: "".to_string(),
        headers: vec![("X-Stream-Type".to_string(), "websocket".to_string())],
        body: vec![],
        extensions: Extensions::new(),
    };

    println!("Starting streaming request...");
    let mut rx = host.call_stream("stream", req).await?;

    println!("Receiving stream frames:");
    let mut frame_count = 0;
    while let Some(frame) = rx.recv().await {
        frame_count += 1;
        println!(
            "Frame {} - Status: {:?}, Data: {}",
            frame_count,
            frame.status,
            String::from_utf8_lossy(&frame.data)
        );

        // Stream ends when we receive StreamEnd, Err, Invalid, or Unsupported
        if matches!(
            frame.status,
            NrStatus::StreamEnd | NrStatus::Err | NrStatus::Invalid | NrStatus::Unsupported
        ) {
            println!("Stream ended with status: {:?}", frame.status);
            break;
        }
    }

    println!("\n✓ All tests completed successfully!");
    Ok(())
}

