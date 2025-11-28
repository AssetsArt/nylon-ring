use nylon_ring_host::{HighLevelRequest, NylonRingHost, NylonRingHostError};
use std::env;
use std::path::PathBuf;

#[tokio::main]
async fn main() -> Result<(), NylonRingHostError> {
    // Determine the path to the plugin library
    let mut plugin_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    plugin_path.pop(); // Go up to workspace root
    plugin_path.push("target");
    plugin_path.push("debug");

    #[cfg(target_os = "macos")]
    plugin_path.push("libnylon_ring_plugin_example.dylib");
    #[cfg(target_os = "linux")]
    plugin_path.push("libnylon_ring_plugin_example.so");
    #[cfg(target_os = "windows")]
    plugin_path.push("nylon_ring_plugin_example.dll");

    println!("Loading plugin from: {:?}", plugin_path);

    let plugin_path_str = plugin_path.to_str().ok_or_else(|| {
        NylonRingHostError::InvalidPluginPath(format!(
            "Path contains invalid UTF-8: {:?}",
            plugin_path
        ))
    })?;
    let host = NylonRingHost::load(plugin_path_str)?;
    println!("Plugin loaded successfully!");

    let req = HighLevelRequest {
        method: "GET".to_string(),
        path: "/hello".to_string(),
        query: "".to_string(),
        headers: vec![("User-Agent".to_string(), "NylonHost/1.0".to_string())],
        body: vec![],
        extensions: std::collections::HashMap::new(),
    };

    println!("Sending request...");
    let (status, payload) = host.call(req).await?;

    println!("Response received!");
    println!("Status: {:?}", status);
    println!("Payload: {:?}", String::from_utf8_lossy(&payload));

    Ok(())
}
