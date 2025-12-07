use std::hint::black_box;

use criterion::{Criterion, criterion_group, criterion_main};
use nylon_ring::{NrBytes, NrKV, NrStr, NrVec};

fn bench_nr_str(c: &mut Criterion) {
    let s = "Hello, World!";

    c.bench_function("NrStr::new", |b| {
        b.iter(|| {
            black_box(NrStr::new(black_box(s)));
        })
    });

    let nr_str = NrStr::new(s);
    c.bench_function("NrStr::as_str", |b| {
        b.iter(|| {
            black_box(nr_str.as_str());
        })
    });
}

fn bench_nr_bytes(c: &mut Criterion) {
    let bytes = b"Hello, World!";

    c.bench_function("NrBytes::from_slice", |b| {
        b.iter(|| {
            black_box(NrBytes::from_slice(black_box(bytes)));
        })
    });

    let nr_bytes = NrBytes::from_slice(bytes);
    c.bench_function("NrBytes::as_slice", |b| {
        b.iter(|| {
            black_box(nr_bytes.as_slice());
        })
    });
}

fn bench_nr_kv(c: &mut Criterion) {
    c.bench_function("NrKV::new", |b| {
        b.iter(|| {
            black_box(NrKV::new(black_box("key"), black_box("value")));
        })
    });
}

fn bench_nr_vec(c: &mut Criterion) {
    c.bench_function("NrVec::from_vec", |b| {
        b.iter(|| {
            let v = vec![1u8, 2, 3, 4, 5];
            black_box(NrVec::from_vec(black_box(v)));
        })
    });

    c.bench_function("NrVec::into_vec", |b| {
        b.iter_batched(
            || {
                let v = vec![1u8, 2, 3, 4, 5];
                NrVec::from_vec(v)
            },
            |nr_vec| {
                black_box(nr_vec.into_vec());
            },
            criterion::BatchSize::SmallInput,
        )
    });

    c.bench_function("NrVec::push", |b| {
        b.iter_batched(
            || NrVec::<u32>::default(),
            |mut nr_vec| {
                for i in 0..100 {
                    nr_vec.push(black_box(i));
                }
                black_box(nr_vec);
            },
            criterion::BatchSize::SmallInput,
        )
    });

    c.bench_function("NrVec::as_slice", |b| {
        let mut nr_vec = NrVec::<u32>::default();
        for i in 0..100 {
            nr_vec.push(i);
        }

        b.iter(|| {
            black_box(nr_vec.as_slice());
        })
    });
}

criterion_group!(
    benches,
    bench_nr_str,
    bench_nr_bytes,
    bench_nr_kv,
    bench_nr_vec
);
criterion_main!(benches);
