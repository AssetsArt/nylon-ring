use crate::types::{
    FastPendingMap, FastStateMap, Pending, StreamFrame, UnaryPending, UnaryResultSlot,
};
use dashmap::mapref::entry::Entry as DashEntry;
use nylon_ring::{NrHostExt, NrStatus};
use rustc_hash::FxBuildHasher;
use std::cell::Cell;
use std::collections::HashMap;
use std::future::poll_fn;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::task::Poll;

/// Number of shards for the pending requests.
const SHARD_COUNT: usize = 64;
const SHARD_MASK: usize = SHARD_COUNT - 1;

/// Host context shared with the plugin.
pub(crate) struct HostContext {
    /// Sharded pending storage is allocated only when a tracked call is made.
    pending_shards: OnceLock<Box<[FastPendingMap]>>,

    pub(crate) state_per_sid: FastStateMap,
    state_shard_counts: [AtomicUsize; SHARD_COUNT],
    pub(crate) host_ext: NrHostExt,
    stream_capacity: usize,
}

impl HostContext {
    pub(crate) fn new(host_ext: NrHostExt, stream_capacity: usize) -> Self {
        Self {
            pending_shards: OnceLock::new(),
            state_per_sid: FastStateMap::with_hasher(FxBuildHasher),
            state_shard_counts: std::array::from_fn(|_| AtomicUsize::new(0)),
            host_ext,
            stream_capacity,
        }
    }

    fn pending_shards(&self) -> &[FastPendingMap] {
        self.pending_shards.get_or_init(|| {
            let mut shards = Vec::with_capacity(SHARD_COUNT);
            for _ in 0..SHARD_COUNT {
                shards.push(FastPendingMap::with_hasher(FxBuildHasher));
            }
            shards.into_boxed_slice()
        })
    }

    pub(crate) fn pending_count(&self) -> usize {
        self.pending_shards
            .get()
            .map_or(0, |shards| shards.iter().map(FastPendingMap::len).sum())
    }

    pub(crate) fn stream_capacity(&self) -> usize {
        self.stream_capacity
    }

    pub(crate) fn set_state(&self, sid: u64, key: String, value: Vec<u8>) {
        match self.state_per_sid.entry(sid) {
            DashEntry::Occupied(mut entry) => {
                entry.get_mut().insert(key, value);
            }
            DashEntry::Vacant(entry) => {
                let mut state = HashMap::new();
                state.insert(key, value);
                self.state_shard_counts[(sid as usize) & SHARD_MASK]
                    .fetch_add(1, Ordering::Release);
                entry.insert(state);
            }
        }
    }

    /// Remove state only when the SID's occupancy shard can contain entries.
    /// The common no-state call path avoids locking the DashMap entirely.
    pub(crate) fn remove_state(&self, sid: u64) {
        let count = &self.state_shard_counts[(sid as usize) & SHARD_MASK];
        if count.load(Ordering::Acquire) == 0 {
            return;
        }
        if self.state_per_sid.remove(&sid).is_some() {
            count.fetch_sub(1, Ordering::AcqRel);
        }
    }

    pub(crate) fn state_count(&self) -> usize {
        self.state_shard_counts
            .iter()
            .map(|count| count.load(Ordering::Acquire))
            .sum()
    }
}

#[inline(always)]
fn get_shard(ctx: &HostContext, sid: u64) -> &FastPendingMap {
    // Shard by the SID's thread-local allocation block (blocks are 2^20 wide)
    // instead of the low bits. Consecutive SIDs from one thread then stay in
    // one shard, so a thread's insert/complete/remove traffic does not migrate
    // the same map cache lines across every core.
    &ctx.pending_shards()[((sid >> 20) as usize) & SHARD_MASK]
}

/// Insert a pending request.
pub(crate) fn insert_pending(ctx: &HostContext, sid: u64, pending: Pending) {
    get_shard(ctx, sid).insert(sid, pending);
}

/// Remove and return a pending request.
pub(crate) fn remove_pending(ctx: &HostContext, sid: u64) -> Option<Pending> {
    get_shard(ctx, sid).remove(&sid).map(|(_, pending)| pending)
}

/// Wait for a unary result stored in the pending map.
///
/// The synchronous response case reaches `Ready` before this future is first
/// polled, so it neither allocates a channel nor clones a waker. For a delayed
/// response, the callback takes and wakes the latest registered waker after
/// releasing the shard lock.
pub(crate) async fn wait_for_unary(ctx: &HostContext, sid: u64) -> Option<(NrStatus, Vec<u8>)> {
    poll_fn(|cx| {
        let mut replacement = None;
        loop {
            match get_shard(ctx, sid).entry(sid) {
                DashEntry::Vacant(entry) => {
                    drop(entry);
                    drop(replacement);
                    return Poll::Ready(None);
                }
                DashEntry::Occupied(mut entry) => match entry.get_mut() {
                    Pending::Unary(UnaryPending::Ready(_, _)) => {
                        let Pending::Unary(UnaryPending::Ready(status, data)) = entry.remove()
                        else {
                            unreachable!();
                        };
                        drop(replacement);
                        return Poll::Ready(Some((status, data)));
                    }
                    Pending::Unary(UnaryPending::Waiting(waker)) => {
                        if waker
                            .as_ref()
                            .is_some_and(|waker| waker.will_wake(cx.waker()))
                        {
                            drop(entry);
                            drop(replacement);
                            return Poll::Pending;
                        }
                        if let Some(replacement) = replacement.take() {
                            let previous = waker.replace(replacement);
                            drop(entry);
                            drop(previous);
                            return Poll::Pending;
                        }
                        drop(entry);
                        replacement = Some(cx.waker().clone());
                    }
                    Pending::Stream(_) => {
                        drop(entry);
                        drop(replacement);
                        return Poll::Ready(None);
                    }
                },
            }
        }
    })
    .await
}

/// Remove all host-owned state associated with a completed SID.
pub(crate) fn cleanup_sid(ctx: &HostContext, sid: u64) {
    remove_pending(ctx, sid);
    ctx.remove_state(sid);
}

/// Deliver a result while holding one shard entry lock for the whole state
/// transition. This prevents remove/reinsert and terminal-frame races.
pub(crate) fn dispatch_pending(ctx: &HostContext, sid: u64, frame: StreamFrame) -> NrStatus {
    match get_shard(ctx, sid).entry(sid) {
        DashEntry::Vacant(_) => NrStatus::Invalid,
        DashEntry::Occupied(mut entry) => {
            if let Pending::Unary(pending) = entry.get_mut() {
                let waker = match pending {
                    UnaryPending::Waiting(waker) => waker.take(),
                    UnaryPending::Ready(_, _) => return NrStatus::Invalid,
                };
                *pending = UnaryPending::Ready(frame.status, frame.data);
                drop(entry);
                ctx.remove_state(sid);
                if let Some(waker) = waker {
                    waker.wake();
                }
                return NrStatus::Ok;
            }

            let terminal = frame.status.is_terminal();
            let tx = match entry.get() {
                Pending::Stream(tx) => tx.clone(),
                Pending::Unary(_) => unreachable!(),
            };
            match tx.try_send(frame) {
                Ok(()) => {
                    if terminal {
                        entry.remove();
                        ctx.remove_state(sid);
                    }
                    NrStatus::Ok
                }
                Err(tokio::sync::mpsc::error::TrySendError::Full(_)) => NrStatus::Backpressure,
                Err(tokio::sync::mpsc::error::TrySendError::Closed(_)) => {
                    entry.remove();
                    ctx.remove_state(sid);
                    NrStatus::Invalid
                }
            }
        }
    }
}

// --- Thread Local Optimization for Unary Results ---
thread_local! {
    pub(crate) static CURRENT_UNARY_RESULT: Cell<*mut UnaryResultSlot> = const { Cell::new(std::ptr::null_mut()) };
}
