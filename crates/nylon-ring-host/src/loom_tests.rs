use loom::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use loom::sync::{Arc, Mutex};
use loom::thread;

struct CallGate {
    in_flight: [AtomicUsize; 2],
}

impl CallGate {
    fn try_begin(&self, shard: usize) -> bool {
        const CLOSED: usize = 1 << (usize::BITS - 1);
        self.in_flight[shard]
            .fetch_update(Ordering::Acquire, Ordering::Relaxed, |state| {
                (state & CLOSED == 0).then_some(state + 1)
            })
            .is_ok()
    }

    fn active_calls(&self) -> usize {
        const ACTIVE_MASK: usize = (1 << (usize::BITS - 1)) - 1;
        self.in_flight
            .iter()
            .map(|counter| counter.load(Ordering::Acquire) & ACTIVE_MASK)
            .sum()
    }

    fn stop(&self) {
        const CLOSED: usize = 1 << (usize::BITS - 1);
        for counter in &self.in_flight {
            counter.fetch_or(CLOSED, Ordering::AcqRel);
        }
    }
}

#[test]
fn loom_call_gate_drains_across_unload_race() {
    loom::model(|| {
        let gate = Arc::new(CallGate {
            in_flight: [AtomicUsize::new(0), AtomicUsize::new(0)],
        });
        let caller_gate = gate.clone();
        let caller = thread::spawn(move || caller_gate.try_begin(1));
        let unload_gate = gate.clone();
        let unload = thread::spawn(move || unload_gate.stop());

        let admitted = caller.join().unwrap();
        unload.join().unwrap();
        if admitted {
            gate.in_flight[1].fetch_sub(1, Ordering::Release);
        }
        assert!(!gate.try_begin(0));
        assert!(!gate.try_begin(1));
        assert_eq!(gate.active_calls(), 0);
    });
}

#[test]
fn loom_callback_router_completes_terminal_stream_once() {
    loom::model(|| {
        let pending = Arc::new(Mutex::new(Some(())));
        let completions = Arc::new(AtomicUsize::new(0));
        let handles: Vec<_> = (0..2)
            .map(|_| {
                let pending = pending.clone();
                let completions = completions.clone();
                thread::spawn(move || {
                    if pending.lock().unwrap().take().is_some() {
                        completions.fetch_add(1, Ordering::AcqRel);
                    }
                })
            })
            .collect();
        for handle in handles {
            handle.join().unwrap();
        }
        assert_eq!(completions.load(Ordering::Acquire), 1);
    });
}

/// Mirror of the retire/sweep protocol in `lib.rs`: guards decrement padded
/// tracker shards with AcqRel and sweep when they observe CLOSED; retiring
/// sets CLOSED on every shard, parks the plugin in a mutex-guarded registry,
/// and sweeps. Frees must happen exactly once, with shard counts always read
/// inside the registry mutex (the mutex hand-off is what guarantees the last
/// finisher or the retirer observes every shard drained).
#[test]
fn loom_retired_plugin_frees_exactly_once_across_finish_stop_races() {
    const CLOSED: usize = 1 << (usize::BITS - 1);
    const ACTIVE_MASK: usize = CLOSED - 1;

    fn sweep(shards: &[AtomicUsize; 2], graveyard: &Mutex<Option<()>>, freed: &AtomicUsize) {
        loom::sync::atomic::fence(Ordering::SeqCst);
        let mut parked = graveyard.lock().unwrap();
        if parked.is_some() {
            let active: usize = shards
                .iter()
                .map(|shard| shard.load(Ordering::Acquire) & ACTIVE_MASK)
                .sum();
            if active == 0 {
                parked.take();
                freed.fetch_add(1, Ordering::Relaxed);
            }
        }
    }

    // Three threads over several atomics and a mutex: unbounded exploration
    // does not finish in reasonable time, so bound preemptions (loom's
    // recommended practice; bound 3 still catches the store-buffering and
    // mutex-handoff interleavings this protocol depends on).
    let mut model = loom::model::Builder::new();
    model.preemption_bound = Some(3);
    model.check(|| {
        // One in-flight call on each of two tracker shards.
        let shards = Arc::new([AtomicUsize::new(1), AtomicUsize::new(1)]);
        let graveyard = Arc::new(Mutex::new(None::<()>));
        let freed = Arc::new(AtomicUsize::new(0));

        let finishers: Vec<_> = (0..2)
            .map(|index| {
                let shards = shards.clone();
                let graveyard = graveyard.clone();
                let freed = freed.clone();
                thread::spawn(move || {
                    let previous = shards[index].fetch_sub(1, Ordering::Release);
                    if previous & CLOSED != 0 {
                        sweep(&shards, &graveyard, &freed);
                    }
                })
            })
            .collect();

        let retirer = {
            let shards = shards.clone();
            let graveyard = graveyard.clone();
            let freed = freed.clone();
            thread::spawn(move || {
                for shard in shards.iter() {
                    shard.fetch_or(CLOSED, Ordering::AcqRel);
                }
                *graveyard.lock().unwrap() = Some(());
                sweep(&shards, &graveyard, &freed);
            })
        };

        for finisher in finishers {
            finisher.join().unwrap();
        }
        retirer.join().unwrap();

        assert_eq!(
            freed.load(Ordering::Relaxed),
            1,
            "retired plugin must be freed exactly once"
        );
        assert!(graveyard.lock().unwrap().is_none());
    });
}

#[test]
fn loom_sid_blocks_do_not_overlap() {
    loom::model(|| {
        const BLOCK: u64 = 8;
        let next = Arc::new(AtomicU64::new(1));
        let first_counter = next.clone();
        let first = thread::spawn(move || first_counter.fetch_add(BLOCK, Ordering::Relaxed));
        let second_counter = next.clone();
        let second = thread::spawn(move || second_counter.fetch_add(BLOCK, Ordering::Relaxed));
        let first = first.join().unwrap();
        let second = second.join().unwrap();
        assert_ne!(first, second);
        assert!(first + BLOCK <= second || second + BLOCK <= first);
    });
}
