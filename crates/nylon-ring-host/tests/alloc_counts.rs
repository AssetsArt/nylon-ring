//! Counts host-side heap activity per call with a counting global allocator.
//!
//! Only allocations made by this test binary (the host side) are counted;
//! the example plugin is a separate image with its own allocator. The plugin
//! side of the response path is documented in docs/PERFORMANCE_INVESTIGATION.md.

use nylon_ring_host::{NrStatus, NylonRingHost};
use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicU64, Ordering};

struct CountingAllocator;

static ALLOCS: AtomicU64 = AtomicU64::new(0);
static DEALLOCS: AtomicU64 = AtomicU64::new(0);

unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        ALLOCS.fetch_add(1, Ordering::Relaxed);
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        DEALLOCS.fetch_add(1, Ordering::Relaxed);
        unsafe { System.dealloc(ptr, layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        ALLOCS.fetch_add(1, Ordering::Relaxed);
        DEALLOCS.fetch_add(1, Ordering::Relaxed);
        unsafe { System.realloc(ptr, layout, new_size) }
    }
}

#[global_allocator]
static GLOBAL: CountingAllocator = CountingAllocator;

fn example_plugin_path() -> Option<std::path::PathBuf> {
    let workspace_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()?
        .parent()?;
    let manifest = workspace_root.join("examples/ex-nyring-plugin/Cargo.toml");
    if !manifest.is_file() {
        return None;
    }
    let status = std::process::Command::new("cargo")
        .args(["build", "--release", "--manifest-path"])
        .arg(&manifest)
        .status()
        .ok()?;
    if !status.success() {
        return None;
    }
    let filename = if cfg!(target_os = "macos") {
        "libex_nyring_plugin.dylib"
    } else if cfg!(target_os = "windows") {
        "ex_nyring_plugin.dll"
    } else {
        "libex_nyring_plugin.so"
    };
    Some(workspace_root.join("target/release").join(filename))
}

/// Host-side allocations and deallocations per call in steady state.
fn measure<F: FnMut()>(calls: u64, mut op: F) -> (f64, f64) {
    let allocs_before = ALLOCS.load(Ordering::Relaxed);
    let deallocs_before = DEALLOCS.load(Ordering::Relaxed);
    for _ in 0..calls {
        op();
    }
    let allocs = ALLOCS.load(Ordering::Relaxed) - allocs_before;
    let deallocs = DEALLOCS.load(Ordering::Relaxed) - deallocs_before;
    (allocs as f64 / calls as f64, deallocs as f64 / calls as f64)
}

#[test]
fn steady_state_host_allocations_per_call() {
    let Some(path) = example_plugin_path() else {
        return;
    };
    let mut host = NylonRingHost::new();
    host.load("example", path.to_str().unwrap()).unwrap();
    let plugin = host.plugin("example").unwrap();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();

    let payload_128 = vec![42u8; 128];

    // Warm up every path so lazy structures (shard maps, hash tables,
    // runtime internals) reach steady state before counting.
    runtime.block_on(async {
        for _ in 0..1_000 {
            plugin
                .call("benchmark_without_response", b"")
                .await
                .unwrap();
            plugin.call_response("benchmark", b"").await.unwrap();
            plugin.call_response_fast("benchmark", b"").await.unwrap();
            plugin
                .call_response("benchmark", &payload_128)
                .await
                .unwrap();
        }
    });

    const CALLS: u64 = 10_000;

    let (a, d) = measure(CALLS, || {
        let status = runtime
            .block_on(plugin.call("benchmark_without_response", b""))
            .unwrap();
        assert_eq!(status, NrStatus::Ok);
    });
    assert_eq!((a, d), (0.0, 0.0), "fire-and-forget must not allocate");

    let (a, d) = measure(CALLS, || {
        let (status, data) = runtime
            .block_on(plugin.call_response("benchmark", b""))
            .unwrap();
        assert_eq!(status, NrStatus::Ok);
        assert!(data.is_empty());
    });
    assert_eq!(
        (a, d),
        (0.0, 0.0),
        "empty unary must not allocate host-side"
    );

    let (a, d) = measure(CALLS, || {
        let (status, data) = runtime
            .block_on(plugin.call_response_fast("benchmark", b""))
            .unwrap();
        assert_eq!(status, NrStatus::Ok);
        assert!(data.is_empty());
    });
    assert_eq!((a, d), (0.0, 0.0), "empty fast path must not allocate");

    // Non-empty responses copy the foreign allocation into a host-owned
    // Vec (ABI v1 cannot prove allocator provenance), so exactly one
    // host-side allocation and one deallocation per call are expected.
    let (a, d) = measure(CALLS, || {
        let (status, data) = runtime
            .block_on(plugin.call_response("benchmark", &payload_128))
            .unwrap();
        assert_eq!(status, NrStatus::Ok);
        assert_eq!(data.len(), 128);
    });
    assert_eq!(
        (a, d),
        (1.0, 1.0),
        "128-byte unary must cost exactly one host alloc/free pair"
    );

    let (a, d) = measure(CALLS, || {
        let (status, data) = runtime
            .block_on(plugin.call_response("benchmark", &[1u8]))
            .unwrap();
        assert_eq!(status, NrStatus::Ok);
        assert_eq!(data.len(), 1);
    });
    assert_eq!(
        (a, d),
        (1.0, 1.0),
        "1-byte unary must cost exactly one host alloc/free pair"
    );
}
