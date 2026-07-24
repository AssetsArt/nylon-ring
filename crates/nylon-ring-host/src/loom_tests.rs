use loom::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use loom::sync::{Arc, Mutex};
use loom::thread;

struct CallGate {
    accepting: AtomicBool,
    in_flight: AtomicUsize,
}

impl CallGate {
    fn try_begin(&self) -> bool {
        if !self.accepting.load(Ordering::Acquire) {
            return false;
        }
        self.in_flight.fetch_add(1, Ordering::AcqRel);
        if !self.accepting.load(Ordering::Acquire) {
            self.in_flight.fetch_sub(1, Ordering::AcqRel);
            return false;
        }
        true
    }
}

#[test]
fn loom_call_gate_drains_across_unload_race() {
    loom::model(|| {
        let gate = Arc::new(CallGate {
            accepting: AtomicBool::new(true),
            in_flight: AtomicUsize::new(0),
        });
        let caller_gate = gate.clone();
        let caller = thread::spawn(move || caller_gate.try_begin());
        let unload_gate = gate.clone();
        let unload = thread::spawn(move || {
            unload_gate.accepting.store(false, Ordering::Release);
        });

        let admitted = caller.join().unwrap();
        unload.join().unwrap();
        if admitted {
            gate.in_flight.fetch_sub(1, Ordering::AcqRel);
        }
        assert!(!gate.accepting.load(Ordering::Acquire));
        assert_eq!(gate.in_flight.load(Ordering::Acquire), 0);
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
