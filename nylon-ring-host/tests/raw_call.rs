use nylon_ring::NrStatus;
use nylon_ring_host::NylonRingHost;
use std::path::PathBuf;

#[tokio::test]
async fn test_call_raw() {
    let mut dylib_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    dylib_path.pop(); // Go up to workspace root
    dylib_path.push("target");
    dylib_path.push("debug");
    dylib_path.push("libnylon_ring_plugin_example.dylib");

    if !dylib_path.exists() {
        // Try .so for Linux if .dylib doesn't exist (though we are on mac)
        dylib_path.set_extension("so");
    }

    // If running in a different profile, might need adjustment, but assuming debug for now.
    // A more robust way is to use cargo env vars if available or just assume standard layout.

    println!("Loading plugin from: {:?}", dylib_path);

    let host = NylonRingHost::load(dylib_path.to_str().unwrap()).expect("Failed to load plugin");

    let payload = b"Hello, Raw World!";
    let (status, response) = host
        .call_raw("echo", payload)
        .await
        .expect("call_raw failed");

    assert_eq!(status, NrStatus::Ok);
    assert_eq!(response, payload);
}
