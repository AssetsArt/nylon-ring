use std::sync::atomic::{AtomicUsize, Ordering};

const COUNTER_SHARDS: usize = 64;
const COUNTER_SHARD_MASK: usize = COUNTER_SHARDS - 1;

static NEXT_THREAD_SHARD: AtomicUsize = AtomicUsize::new(0);

const CLOSED: usize = 1 << (usize::BITS - 1);
const ACTIVE_MASK: usize = CLOSED - 1;

thread_local! {
    static THREAD_SHARD: usize =
        NEXT_THREAD_SHARD.fetch_add(1, Ordering::Relaxed) & COUNTER_SHARD_MASK;
}

// Apple Silicon uses 128-byte cache lines. The larger alignment also keeps
// counters independent on targets with 64-byte cache lines.
#[repr(align(128))]
struct CallCounter(AtomicUsize);

pub(crate) struct CallTracker {
    counters: [CallCounter; COUNTER_SHARDS],
}

impl CallTracker {
    pub(crate) fn new() -> Self {
        Self {
            counters: std::array::from_fn(|_| CallCounter(AtomicUsize::new(0))),
        }
    }

    /// Admit a call and return the counter shard that must be released.
    pub(crate) fn try_begin(&self) -> Option<usize> {
        let shard = THREAD_SHARD.with(|shard| *shard);
        self.counters[shard]
            .0
            .fetch_update(Ordering::Acquire, Ordering::Relaxed, |state| {
                (state & CLOSED == 0 && state & ACTIVE_MASK != ACTIVE_MASK).then_some(state + 1)
            })
            .ok()
            .map(|_| shard)
    }

    pub(crate) fn finish(&self, shard: usize) {
        self.counters[shard].0.fetch_sub(1, Ordering::Release);
    }

    pub(crate) fn stop(&self) {
        for counter in &self.counters {
            counter.0.fetch_or(CLOSED, Ordering::AcqRel);
        }
    }

    pub(crate) fn active_calls(&self) -> usize {
        self.counters
            .iter()
            .map(|counter| counter.0.load(Ordering::Acquire) & ACTIVE_MASK)
            .sum()
    }
}

#[cfg(test)]
mod tests {
    use super::CallTracker;

    #[test]
    fn tracks_nested_calls_and_rejects_calls_after_stop() {
        let tracker = CallTracker::new();
        let first = tracker.try_begin().unwrap();
        let second = tracker.try_begin().unwrap();
        assert_eq!(tracker.active_calls(), 2);

        tracker.finish(first);
        tracker.finish(second);
        assert_eq!(tracker.active_calls(), 0);

        tracker.stop();
        assert!(tracker.try_begin().is_none());
    }
}
