use criterion::{black_box, criterion_group, criterion_main, Criterion};
use nylon_ring::{NrBytes, NrHeader, NrRequest, NrStr};

fn bench_nrstr_from_str(c: &mut Criterion) {
    let test_string = "Hello, World! This is a test string for benchmarking.";
    c.bench_function("NrStr::from_str", |b| {
        b.iter(|| {
            black_box(NrStr::from_str(black_box(test_string)));
        })
    });
}

fn bench_nrstr_as_str(c: &mut Criterion) {
    let nr_str = NrStr::from_str("Hello, World! This is a test string for benchmarking.");
    c.bench_function("NrStr::as_str", |b| {
        b.iter(|| {
            black_box(black_box(&nr_str).as_str());
        })
    });
}

fn bench_nrbytes_from_slice(c: &mut Criterion) {
    let test_data = b"Hello, World! This is test data for benchmarking byte operations.";
    c.bench_function("NrBytes::from_slice", |b| {
        b.iter(|| {
            black_box(NrBytes::from_slice(black_box(test_data)));
        })
    });
}

fn bench_nrbytes_as_slice(c: &mut Criterion) {
    let nr_bytes =
        NrBytes::from_slice(b"Hello, World! This is test data for benchmarking byte operations.");
    c.bench_function("NrBytes::as_slice", |b| {
        b.iter(|| {
            black_box(black_box(&nr_bytes).as_slice());
        })
    });
}

fn bench_nrheader_new(c: &mut Criterion) {
    c.bench_function("NrHeader::new", |b| {
        b.iter(|| {
            black_box(NrHeader::new(
                black_box("Content-Type"),
                black_box("application/json"),
            ));
        })
    });
}

fn bench_nrrequest_build(c: &mut Criterion) {
    let path_str = "/api/v1/users";
    let method_str = "GET";
    let query_str = "page=1&limit=10";

    let headers: Vec<NrHeader> = vec![
        NrHeader::new("Content-Type", "application/json"),
        NrHeader::new("Authorization", "Bearer token123"),
        NrHeader::new("User-Agent", "NylonRing/1.0"),
    ];

    c.bench_function("NrRequest::build", |b| {
        b.iter(|| {
            black_box(NrRequest {
                path: NrStr::from_str(black_box(path_str)),
                method: NrStr::from_str(black_box(method_str)),
                query: NrStr::from_str(black_box(query_str)),
                headers: black_box(headers.as_ptr()),
                headers_len: black_box(headers.len() as u32),
                _reserved0: 0,
                _reserved1: 0,
            });
        })
    });
}

criterion_group!(
    benches,
    bench_nrstr_from_str,
    bench_nrstr_as_str,
    bench_nrbytes_from_slice,
    bench_nrbytes_as_slice,
    bench_nrheader_new,
    bench_nrrequest_build
);
criterion_main!(benches);
