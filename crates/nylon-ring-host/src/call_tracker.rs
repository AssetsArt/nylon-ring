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

    /// Release a call slot. Returns `true` when the tracker had already been
    /// stopped: the caller may have been the last in-flight call of a retired
    /// plugin and must trigger a graveyard sweep. Release suffices (no
    /// Acquire) because every sweep reads the shard counts inside the
    /// graveyard mutex: each finisher's decrement is sequenced before its own
    /// sweep's lock, so the mutex hand-off publishes it to every later
    /// sweeper, and `stop`'s AcqRel RMW on the same counter covers the
    /// decrement-before-stop case by coherence. Modeled in
    /// `loom_retired_plugin_frees_exactly_once_across_finish_stop_races`.
    #[must_use]
    pub(crate) fn finish(&self, shard: usize) -> bool {
        let previous = self.counters[shard].0.fetch_sub(1, Ordering::Release);
        previous & CLOSED != 0
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

        assert!(!tracker.finish(first));
        assert!(!tracker.finish(second));
        assert_eq!(tracker.active_calls(), 0);

        tracker.stop();
        assert!(tracker.try_begin().is_none());
    }
}
