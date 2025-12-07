use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion};
use nylon_ring_host::NylonRingHost;
use std::{hint::black_box, sync::Arc};

fn get_plugin_path() -> String {
    #[cfg(target_os = "macos")]
    let path = "target/release/libex_nyring_plugin.dylib";
    #[cfg(target_os = "windows")]
    let path = "target/release/ex_nyring_plugin.dll";
    #[cfg(target_os = "linux")]
    let path = "target/release/libex_nyring_plugin.so";

    path.to_string()
}

fn setup_host() -> Arc<NylonRingHost> {
    // Get the workspace root directory
    let workspace_root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf();

    // Build the plugin first
    let plugin_manifest = workspace_root.join("examples/ex-nyring-plugin/Cargo.toml");
    let _ = std::process::Command::new("cargo")
        .args([
            "build",
            "--manifest-path",
            plugin_manifest.to_str().unwrap(),
            "--release",
        ])
        .status()
        .expect("Failed to build plugin");

    let plugin_path = workspace_root.join(get_plugin_path());
    Arc::new(NylonRingHost::load(plugin_path.to_str().unwrap()).expect("Failed to load plugin"))
}

fn bench_call_response(c: &mut Criterion) {
    let host = setup_host();
    let runtime = tokio::runtime::Runtime::new().unwrap();

    c.bench_function("call_response", |b| {
        b.iter(|| {
            runtime.block_on(async {
                let payload = b"Hello";
                let result = host.call_response("benchmark", black_box(payload)).await;
                black_box(result).unwrap();
            })
        })
    });
}

fn bench_call_response_with_payload(c: &mut Criterion) {
    let host = setup_host();
    let runtime = tokio::runtime::Runtime::new().unwrap();

    let mut group = c.benchmark_group("call_response_with_payload");

    for size in [128, 1024, 4096].iter() {
        let payload: Vec<u8> = vec![42u8; *size];

        group.bench_with_input(BenchmarkId::from_parameter(size), &payload, |b, payload| {
            b.iter(|| {
                runtime.block_on(async {
                    let result = host.call_response("benchmark", black_box(payload)).await;
                    black_box(result).unwrap();
                })
            })
        });
    }

    group.finish();
}

fn bench_call_response_fast(c: &mut Criterion) {
    let host = setup_host();
    let runtime = tokio::runtime::Runtime::new().unwrap();

    c.bench_function("call_response_fast", |b| {
        b.iter(|| {
            runtime.block_on(async {
                let payload = b"Hello";
                let result = host
                    .call_response_fast("benchmark", black_box(payload))
                    .await;
                black_box(result).unwrap();
            })
        })
    });
}

fn bench_call_without_response(c: &mut Criterion) {
    let host = setup_host();
    let runtime = tokio::runtime::Runtime::new().unwrap();

    c.bench_function("call_without_response", |b| {
        b.iter(|| {
            runtime.block_on(async {
                let payload = b"Hello";
                let result = host
                    .call("benchmark_without_response", black_box(payload))
                    .await;
                black_box(result).unwrap();
            })
        })
    });
}

criterion_group!(
    benches,
    bench_call_response,
    bench_call_response_with_payload,
    bench_call_response_fast,
    bench_call_without_response
);
criterion_main!(benches);
