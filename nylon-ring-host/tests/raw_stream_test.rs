use nylon_ring_host::NrStatus;
use nylon_ring_host::NylonRingHost;
use std::path::PathBuf;

#[tokio::test]
async fn test_call_raw_stream() -> Result<(), Box<dyn std::error::Error>> {
    // Locate the plugin
    let mut dylib_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    dylib_path.pop(); // Go up to workspace root
    dylib_path.push("target");
    dylib_path.push("debug");
    dylib_path.push("libnylon_ring_plugin_example.dylib");

    if !dylib_path.exists() {
        // Try .so for Linux if .dylib doesn't exist (though we are on mac)
        dylib_path.set_extension("so");
    }

    // On mac it might be .dylib, on linux .so.
    // For this environment (mac), .dylib is correct.
    // If running in a different env, might need adjustment, but user env is mac.

    println!("Loading plugin from: {:?}", dylib_path);
    let host = NylonRingHost::load(dylib_path.to_str().unwrap())?;

    let payload = b"test_payload";
    let (sid, mut rx) = host.call_raw_stream("stream", payload).await?;

    println!("Stream started with SID: {}", sid);

    let mut frame_count = 0;
    while let Some(frame) = rx.recv().await {
        println!("Received frame: {:?}", frame);
        if frame.status == NrStatus::StreamEnd {
            break;
        }
        frame_count += 1;
        let msg = String::from_utf8(frame.data)?;
        assert!(msg.contains("Stream frame"));
        assert!(msg.contains("test_payload"));
    }

    assert_eq!(frame_count, 3);
    Ok(())
}
