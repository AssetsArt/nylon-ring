use nylon_ring::NrStatus;
use nylon_ring_host::{Extensions, HighLevelRequest, NylonRingHost};
use std::env;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::time::Duration;

fn get_plugin_path() -> PathBuf {
    let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    path.pop(); // Go up to workspace root
    path.push("target");
    path.push("go");

    #[cfg(target_os = "macos")]
    path.push("nylon_ring_go_plugin_simple.dylib");
    #[cfg(target_os = "linux")]
    path.push("nylon_ring_go_plugin_simple.so");
    #[cfg(target_os = "windows")]
    path.push("nylon_ring_go_plugin_simple.dll");

    path
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let plugin_path = get_plugin_path();
    println!("Loading Go plugin from: {:?}", plugin_path);

    let host = Arc::new(NylonRingHost::load(plugin_path.to_str().unwrap())?);

    let req = HighLevelRequest {
        method: "GET".to_string(),
        path: "/chat".to_string(),
        query: "".to_string(),
        headers: vec![],
        body: vec![],
        extensions: Extensions::new(),
    };

    println!("Starting bidirectional stream with Go plugin...");
    // Go plugin example registers "stream" handler which sends 5 frames
    // And we registered HandleStream to handle data/close
    let (sid, mut rx) = host.call_stream("stream", req).await?;

    // Spawn a task to handle incoming messages from plugin
    let rx_handle = tokio::spawn(async move {
        while let Some(frame) = rx.recv().await {
            match frame.status {
                NrStatus::Ok => {
                    if let Ok(msg) = String::from_utf8(frame.data) {
                        println!("Received from plugin: {}", msg);
                    }
                }
                NrStatus::StreamEnd => {
                    println!("Stream ended by plugin");
                    if let Ok(msg) = String::from_utf8(frame.data) {
                        println!("End message: {}", msg);
                    }
                    break;
                }
                _ => {
                    println!("Stream error: {:?}", frame.status);
                    break;
                }
            }
        }
    });

    // Send messages to plugin
    let messages = vec!["Hello Go", "How are you Go?", "Bye Go"];
    for msg in messages {
        println!("Sending to plugin: {}", msg);
        host.send_stream_data(sid, msg.as_bytes())?;
        tokio::time::sleep(Duration::from_secs(1)).await;
    }

    // Close stream
    println!("Closing stream from host...");
    host.close_stream(sid)?;

    // Wait for receiver to finish
    rx_handle.await?;

    Ok(())
}
