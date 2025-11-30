use criterion::{criterion_group, criterion_main, Criterion, Throughput};
use nylon_ring_host::{Extensions, HighLevelRequest, NylonRingHost};
use std::env;
use std::hint::black_box;
use std::path::PathBuf;

fn get_plugin_path() -> PathBuf {
    let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    path.pop(); // Go up to workspace root
    path.push("target");
    path.push("release"); // Use release build for benchmarks

    #[cfg(target_os = "macos")]
    path.push("libnylon_ring_bench_plugin.dylib");
    #[cfg(target_os = "linux")]
    path.push("libnylon_ring_bench_plugin.so");
    #[cfg(target_os = "windows")]
    path.push("nylon_ring_bench_plugin.dll");

    path
}

fn get_go_plugin_path() -> PathBuf {
    let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    path.pop(); // Go up to workspace root
    path.push("target");
    path.push("go");
    #[cfg(target_os = "macos")]
    path.push("plugin-bench.dylib");
    #[cfg(target_os = "linux")]
    path.push("plugin-bench.so");
    #[cfg(target_os = "windows")]
    path.push("plugin-bench.dll");
    path
}

fn bench_host_call_overhead(c: &mut Criterion) {
    let plugin_path = get_plugin_path();

    if !plugin_path.exists() {
        eprintln!(
            "Plugin not found at {:?}, skipping host benchmarks",
            plugin_path
        );
        return;
    }

    let rt = tokio::runtime::Runtime::new().unwrap();
    let host = rt.block_on(async {
        NylonRingHost::load(plugin_path.to_str().unwrap()).expect("Failed to load plugin")
    });

    let mut group = c.benchmark_group("host_overhead");
    group.throughput(Throughput::Elements(1));

    group.bench_function("fast_raw_unary_call", |b| {
        b.iter(|| {
            rt.block_on(async {
                let _result = host.call_raw_unary_fast("echo", b"bench").await;
            });
        });
    });

    group.bench_function("raw_unary_call", |b| {
        b.iter(|| {
            rt.block_on(async {
                let _result = host.call_raw("echo", b"bench").await;
            });
        });
    });

    group.bench_function("unary_call", |b| {
        b.iter(|| {
            rt.block_on(async {
                let req = HighLevelRequest {
                    method: black_box("GET".to_string()),
                    path: black_box("/benchmark".to_string()),
                    query: black_box("".to_string()),
                    headers: black_box(vec![(
                        "User-Agent".to_string(),
                        "Benchmark/1.0".to_string(),
                    )]),
                    body: black_box(vec![]),
                    extensions: Extensions::new(),
                };

                let _result = host.call("unary", req).await;
            });
        });
    });

    group.bench_function("unary_call_with_body", |b| {
        b.iter(|| {
            rt.block_on(async {
                let body = vec![0u8; 1024]; // 1KB body
                let req = HighLevelRequest {
                    method: black_box("POST".to_string()),
                    path: black_box("/benchmark".to_string()),
                    query: black_box("".to_string()),
                    headers: black_box(vec![(
                        "Content-Type".to_string(),
                        "application/json".to_string(),
                    )]),
                    body: black_box(body),
                    extensions: Extensions::new(),
                };

                let _result = host.call("unary", req).await;
            });
        });
    });

    group.bench_function("stream_call", |b| {
        b.iter(|| {
            rt.block_on(async {
                let req = HighLevelRequest {
                    method: black_box("GET".to_string()),
                    path: black_box("/stream".to_string()),
                    query: black_box("".to_string()),
                    headers: black_box(vec![]),
                    body: black_box(vec![]),
                    extensions: Extensions::new(),
                };

                let mut stream = host.call_stream("stream", req).await.unwrap();
                // Consume all frames
                while let Some(_frame) = stream.recv().await {
                    // Consume frame
                }
            });
        });
    });

    group.finish();
}

fn bench_go_host_call_overhead(c: &mut Criterion) {
    let plugin_path = get_go_plugin_path();

    if !plugin_path.exists() {
        eprintln!(
            "Go plugin not found at {:?}, skipping Go host benchmarks",
            plugin_path
        );
        return;
    }

    let rt = tokio::runtime::Runtime::new().unwrap();
    let host = rt.block_on(async {
        NylonRingHost::load(plugin_path.to_str().unwrap()).expect("Failed to load Go plugin")
    });

    let mut group = c.benchmark_group("go_host_overhead");
    group.throughput(Throughput::Elements(1));

    group.bench_function("go_fast_raw_unary_call", |b| {
        b.iter(|| {
            rt.block_on(async {
                let _result = host.call_raw_unary_fast("echo", b"bench").await;
            });
        });
    });

    group.bench_function("go_raw_unary_call", |b| {
        b.iter(|| {
            rt.block_on(async {
                let _result = host.call_raw("echo", b"bench").await;
            });
        });
    });

    group.bench_function("unary_call", |b| {
        b.iter(|| {
            rt.block_on(async {
                let req = HighLevelRequest {
                    method: black_box("GET".to_string()),
                    path: black_box("/benchmark".to_string()),
                    query: black_box("".to_string()),
                    headers: black_box(vec![]),
                    body: black_box(vec![]),
                    extensions: Extensions::new(),
                };

                let _result = host.call("unary", req).await;
            });
        });
    });

    group.bench_function("unary_call_with_body", |b| {
        b.iter(|| {
            rt.block_on(async {
                let body = vec![0u8; 1024]; // 1KB body
                let req = HighLevelRequest {
                    method: black_box("POST".to_string()),
                    path: black_box("/benchmark".to_string()),
                    query: black_box("".to_string()),
                    headers: black_box(vec![]),
                    body: black_box(body),
                    extensions: Extensions::new(),
                };

                let _result = host.call("unary", req).await;
            });
        });
    });

    group.bench_function("stream_call", |b| {
        b.iter(|| {
            rt.block_on(async {
                let req = HighLevelRequest {
                    method: black_box("GET".to_string()),
                    path: black_box("/stream".to_string()),
                    query: black_box("".to_string()),
                    headers: black_box(vec![]),
                    body: black_box(vec![]),
                    extensions: Extensions::new(),
                };

                let mut stream = host.call_stream("stream", req).await.unwrap();
                while let Some(_frame) = stream.recv().await {
                    // Consume frame
                }
            });
        });
    });

    group.finish();
}

fn bench_request_building(c: &mut Criterion) {
    c.bench_function("build_high_level_request", |b| {
        b.iter(|| {
            black_box(HighLevelRequest {
                method: black_box("GET".to_string()),
                path: black_box("/api/v1/users".to_string()),
                query: black_box("page=1&limit=10".to_string()),
                headers: black_box(vec![
                    ("Content-Type".to_string(), "application/json".to_string()),
                    ("Authorization".to_string(), "Bearer token123".to_string()),
                    ("User-Agent".to_string(), "NylonRing/1.0".to_string()),
                ]),
                body: black_box(vec![0u8; 512]),
                extensions: Extensions::new(),
            });
        })
    });
}

criterion_group!(
    benches,
    bench_request_building,
    bench_host_call_overhead,
    bench_go_host_call_overhead
);
criterion_main!(benches);
