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
