use nylon_ring::NrStatus;
use nylon_ring_host::{HighLevelRequest, NylonRingHost};
use std::sync::Arc;

#[tokio::test]
async fn test_bidirectional_streaming() -> Result<(), Box<dyn std::error::Error>> {
    // Compile the bench plugin first to ensure it's up to date
    let status = std::process::Command::new("cargo")
        .args(&["build", "--package", "nylon-ring-bench-plugin", "--lib"])
        .status()?;
    assert!(status.success());

    let dylib_path = if cfg!(target_os = "macos") {
        "../target/debug/libnylon_ring_bench_plugin.dylib"
    } else {
        "../target/debug/libnylon_ring_bench_plugin.so"
    };

    let host = Arc::new(NylonRingHost::load(dylib_path)?);

    // Start a stream
    let req = HighLevelRequest {
        method: "GET".to_string(),
        path: "/stream".to_string(),
        query: "".to_string(),
        headers: vec![],
        body: vec![],
        extensions: Default::default(),
    };

    let (sid, mut rx) = host.call_stream("bidi_stream", req).await?;

    // Receive initial frames (bench plugin sends 5 frames)
    for i in 1..=5 {
        let frame = rx.recv().await.expect("Should receive frame");
        assert_eq!(frame.status, NrStatus::Ok);
        let msg = String::from_utf8(frame.data)?;
        assert_eq!(msg, format!("Frame {}", i));
    }

    // Now send data to the plugin
    let data = b"Hello from host";
    host.send_stream_data(sid, data)?;

    // Expect echo response
    let frame = rx.recv().await.expect("Should receive echo frame");
    assert_eq!(frame.status, NrStatus::Ok);
    let msg = String::from_utf8(frame.data)?;
    assert_eq!(msg, format!("Received data: {}", data.len()));

    // Close stream
    host.close_stream(sid)?;

    // Expect stream end
    let frame = rx.recv().await.expect("Should receive stream end");
    assert_eq!(frame.status, NrStatus::StreamEnd);

    Ok(())
}
