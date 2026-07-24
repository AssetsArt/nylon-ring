# Performance investigation

This report records the measurement-driven performance work for ABI v1 on an
Apple M1 Pro (8 performance cores and 2 efficiency cores). ABI layouts,
function signatures, ownership rules, lifecycle guarantees, and status values
were kept unchanged.

## Result

The retained optimization removes Tokio's per-call oneshot allocation from the
standard unary path. Unary completion now lives in the existing pending-map
entry as `Waiting(Option<Waker>)` or `Ready(status, data)`.

- Steady-state host allocations: 1 allocation + 1 free (88 bytes) to 0 per call.
- Single-stream unary: 139.12 ns to 104.71 ns in the paired run (-24.7%).
- 10-worker unary median: 16.27M to 21.72M calls/s (+33.5%).
- Delayed and cross-thread replies still register and wake the latest executor
  waker. Synchronous replies never clone a waker.

The absolute multi-core result is sensitive to concurrent system load. Repeated
direct-terminal runs were materially faster than runs launched while Codex was
compiling and profiling. The README therefore uses the direct-terminal snapshot
(126.01M fire-and-forget, 99.18M fast, and 25.34M unary calls/s), while the
controlled before/after curve below uses one consistent managed-session method.
The final direct-terminal Criterion estimates were 40.610 ns for
fire-and-forget, 59.121 ns for fast unary, and 95.797 ns for standard unary.

## Measurement controls

- Release profile: `opt-level=3`, fat LTO, one codegen unit.
- Criterion ABI inputs and results are both passed through `black_box`.
- Ownership benchmarks use `iter_batched`, excluding setup allocation and
  returned-value destruction from the timed region.
- Scaling runs accept `NYRING_BENCH_WORKERS`, `NYRING_BENCH_SECONDS`, and
  `NYRING_BENCH_BATCH_SIZE`; each point below is the median of three 10-second
  runs with batches of 100.
- `NYRING_BENCH_OPERATION=fire|fast|unary` selects one operation without adding
  a branch to its hot loop.
- `NYRING_BENCH_CPU_SAMPLES=1` samples the current macOS CPU every 1,024 batches.

CPU IDs 0-1 are efficiency cores and 2-9 are performance cores on this machine.
Normal runs used all cores but placed roughly 92-96% of samples on performance
cores. An explicit background-QoS run changed fire-and-forget from 42.32 ns to
265.50 ns, confirming that placement/QoS can dominate variance. One 10-worker
sample recorded 434 observations on efficiency cores and 5,329 on performance
cores.

The exact published executable was also rebuilt from commit `9abb32e`. Under
the managed workload its three-run medians were 62.23M, 50.29M, and 14.48M
calls/s, so the failure to reproduce the earlier 125.12M/88.67M/19.72M figures
was not caused by later source or harness changes.

## Experiments

| Change | Bench | Before | After | Delta | Soundness risk | Kept? |
|---|---|---:|---:|---:|---|:---:|
| Correct ABI `black_box` barriers | `NrBytes::as_slice` | 0.318 ns | 0.491 ns | +54.4% reported time | None; benchmark-only | Yes |
| Correct ownership timing scope | `NrVec::from_vec` | 22.89 ns | 3.86 ns | -83.1% reported time | None; benchmark-only | Yes |
| Pending outer shards: 64 to 32 | 10-worker unary | 11.81-13.95M/s control | 11.23-11.55M/s | Regression | None | No |
| Pending outer shards: 64 to 128 | 30-second unary | 11.39/12.66M/s controls | 11.64M/s | Inside control drift | None | No |
| Rust two-copy to C one-copy probe | Unary, 128 B / 1 KiB / 4 KiB | 197.83 / 251.92 / 345.39 ns | 174.38 / 198.30 / 269.11 ns | -11.9% / -21.3% / -22.1% | Changed ownership semantics | Probe removed |
| Inline unary completion state | Single-stream unary | 139.12 ns | 104.71 ns | -24.7% | Waker/cancellation races | Yes, with tests |
| Inline unary completion state | 10-worker unary | 16.27M/s | 21.72M/s | +33.5% | Same as above | Yes |
| Opportunistic standard TLS slot | Single-stream unary | 104.71 ns | 114.63 ns | +9.5% | Extra first-response race surface | No |

## Multi-core scaling

Values are median calls/s from three runs; brackets show the observed minimum
and maximum. Fire-and-forget and fast-path code did not change, so they are
reference curves rather than artificial duplicated “after” columns.

| Workers | Fire-and-forget, M/s | Fast, M/s | Unary before, M/s | Unary after, M/s | Unary delta |
|---:|---:|---:|---:|---:|---:|
| 1 | 15.19 [15.15-15.37] | 11.94 [11.75-12.10] | 4.82 [4.82-5.13] | 7.84 [7.80-7.96] | +62.6% |
| 2 | 20.51 [20.51-24.24] | 18.10 [16.61-18.22] | 7.16 [6.23-7.50] | 8.54 [8.41-8.60] | +19.2% |
| 4 | 47.77 [44.74-48.25] | 39.96 [35.05-41.56] | 13.31 [12.53-14.33] | 17.03 [13.59-19.30] | +28.0% |
| 8 | 57.26 [54.48-61.46] | 55.72 [51.20-60.36] | 15.97 [12.73-16.43] | 20.28 [20.15-21.72] | +27.0% |
| 10 | 63.60 [56.81-67.30] | 60.38 [55.48-64.65] | 16.27 [15.85-17.28] | 21.72 [21.16-22.24] | +33.5% |

## Hypothesis results

### H1: async router contention — falsified

The pending router uses 64 outer shards selected by `sid & 63`; every outer
map is itself a 64-shard DashMap on this machine. DashMap's crossbeam storage is
128-byte aligned on AArch64. Session IDs come from thread-local blocks of
1,000,003, so the global atomic is touched once per block, not once per call.
The Fx hash and sequential IDs did not produce shard imbalance: sampled shard
counts differed by at most one.

Explicit `try_entry` instrumentation measured the percentage of contended map
operations:

| Workers | Insert contention | Callback contention |
|---:|---:|---:|
| 2 | 0.0446% | 0.0487% |
| 4 | 0.3255% | 0.3201% |
| 8 | 0.1145% | 0.0596% |
| 10 | about 0.13% | about 0.09% |

Contention neither grew superlinearly nor explained the scaling gap. Both 32-
and 128-shard experiments failed their predicted improvement and were reverted.

### H2: avoidable payload copy — confirmed, but ABI-v1 constrained

The request remains borrowed from host to plugin; the host does not
defensively copy `NrBytes`. The Rust benchmark plugin copies the request into a
producer-owned `NrVec` and the host then copies that foreign allocation into a
host-owned `Vec`: two copies. A temporary C response used a borrowed `NrVec`
and isolated the cost of one copy, saving 23-76 ns as payload size increased.

The host cannot replace its copy with `Vec::from_raw_parts`: the response was
allocated inside another dynamic library and freeing or growing it with the
host allocator is a cross-allocator bug. The existing `call_response -> Vec<u8>`
contract therefore retains the safe copy.

### H3: `NrVec::into_vec` indirection — copy/free confirmed, fast take blocked

For a five-byte vector, the measured components were:

- complete `into_vec`: 25.14 ns;
- destination allocation and copy: 16.85 ns;
- producer drop callback and source free: 9.25 ns.

Capacity equalled length; no shrink occurred. Comparing the producer drop
function address with the host's `drop_vec::<T>` is not a sound allocator
identity test: Rust permits equivalent functions to share addresses and one
generic function to have multiple addresses. It would also miss the real Rust
plugin, whose callback is private code in a different Mach-O image. The copy
fallback remains unchanged.

### H4: unary setup — confirmed and improved

A symbolized 15-second `sample` profile collected 12,158 active-thread samples.
`oneshot::channel()` accounted for 8.6%; channel destruction/free accounted for
32.7%. DashMap insertion was 4.2%, total callback/map work was about 6%, locks
were about 1%, and the synchronous benchmark had no meaningful waker/scheduler
hotspot. A counting allocator independently found exactly one 88-byte allocation
and one free per unary call.

The retained state machine removes that allocation. Waiter registration uses a
two-phase algorithm so arbitrary `RawWaker` clone/drop callbacks never run under
a DashMap lock. The callback performs `Waiting -> Ready`, releases the shard,
then wakes. Tests cover cross-thread completion, waker replacement, re-entrant
RawWaker clone/drop, duplicate responses, cancellation cleanup, timeouts,
stream backpressure, and unload call tracking.

An additional same-thread TLS shortcut was tested only after this change. Its
TLS bind/clear and first-response claim cost exceeded the map operation it
removed (114.63 ns versus 104.71 ns), so it was fully reverted.

## Deliberately unchanged

- Lock and waker pooling: the profile showed neither as a primary hotspot after
  removing the channel allocation.
- More router shards or manual cache padding: direct experiments falsified the
  contention hypothesis, and DashMap is already cache-padded on AArch64.
- Request copying: the host already preserves the `NrBytes` borrow through the
  synchronous handler invocation.
- `NrVec` allocator shortcuts: no allocator provenance can be proven from ABI
  v1 fields.

## ABI-v2 proposal

Two complementary interfaces can remove the remaining response copy safely:

1. `NrOwnedBytesV2 { ptr, len, owner_ctx, release_fn }` returned as a host
   `ResponseBytes` view. Its drop calls the producer release function and it
   retains a library guard so the callback code cannot be unloaded early.
2. A host-owned output protocol:
   `acquire_result_buffer(capacity) -> { ptr, cap, token }`, followed by
   `commit_result_buffer(token, initialized_len)`. The host can then return its
   original `Vec<u8>` without guessing allocator provenance.

The one-copy probe suggests an end-to-end gain of roughly 23 ns at 128 bytes and
76 ns at 4 KiB on this machine. This requires ABI v2 and was intentionally not
implemented in ABI v1.

## Verification

The final retained code passed:

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-targets --all-features
```

The workspace test run includes the C plugin ABI round-trip. The release Rust
plugin and host demo were also built and run; fire-and-forget, fast unary,
standard unary, bounded streaming, and rapid sequential calls all completed
successfully. The demo reports `DashMap + inline completion` for standard unary.

## Reproduction

```bash
cargo bench --package nylon-ring --bench abi_types
cargo bench --package nylon-ring-host --bench host_overhead

cargo build --release --package ex-nyring-host --package ex-nyring-plugin
NYRING_BENCH_WORKERS=10 \
NYRING_BENCH_SECONDS=10 \
NYRING_BENCH_CPU_SAMPLES=1 \
./target/release/ex-nyring-host
```

Instruments/xctrace was unavailable because the installed Devices plugin failed
to load; the investigation used `/usr/bin/sample` as the profiler fallback.
