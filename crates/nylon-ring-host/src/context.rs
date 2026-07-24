use crate::types::{FastPendingMap, FastStateMap, Pending, StreamFrame, UnaryResultSlot};
use dashmap::mapref::entry::Entry;
use nylon_ring::{NrHostExt, NrStatus};
use rustc_hash::FxBuildHasher;
use std::cell::Cell;
use std::sync::OnceLock;

/// Number of shards for the pending requests.
const SHARD_COUNT: usize = 64;
const SHARD_MASK: usize = SHARD_COUNT - 1;

/// Host context shared with the plugin.
pub(crate) struct HostContext {
    /// Sharded pending storage is allocated only when a tracked call is made.
    pending_shards: OnceLock<Box<[FastPendingMap]>>,

    pub(crate) state_per_sid: FastStateMap,
    pub(crate) host_ext: NrHostExt,
    stream_capacity: usize,
}

impl HostContext {
    pub(crate) fn new(host_ext: NrHostExt, stream_capacity: usize) -> Self {
        Self {
            pending_shards: OnceLock::new(),
            state_per_sid: FastStateMap::with_hasher(FxBuildHasher),
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
}

#[inline(always)]
fn get_shard(ctx: &HostContext, sid: u64) -> &FastPendingMap {
    &ctx.pending_shards()[(sid as usize) & SHARD_MASK]
}

/// Insert a pending request.
pub(crate) fn insert_pending(ctx: &HostContext, sid: u64, pending: Pending) {
    get_shard(ctx, sid).insert(sid, pending);
}

/// Remove and return a pending request.
pub(crate) fn remove_pending(ctx: &HostContext, sid: u64) -> Option<Pending> {
    get_shard(ctx, sid).remove(&sid).map(|(_, v)| v)
}

/// Remove all host-owned state associated with a completed SID.
pub(crate) fn cleanup_sid(ctx: &HostContext, sid: u64) {
    remove_pending(ctx, sid);
    ctx.state_per_sid.remove(&sid);
}

/// Deliver a result while holding one shard entry lock for the whole state
/// transition. This prevents remove/reinsert and terminal-frame races.
pub(crate) fn dispatch_pending(ctx: &HostContext, sid: u64, frame: StreamFrame) -> NrStatus {
    match get_shard(ctx, sid).entry(sid) {
        Entry::Vacant(_) => NrStatus::Invalid,
        Entry::Occupied(entry) => {
            if matches!(entry.get(), Pending::Unary(_)) {
                let Pending::Unary(tx) = entry.remove() else {
                    unreachable!();
                };
                ctx.state_per_sid.remove(&sid);
                return if tx.send((frame.status, frame.data)).is_ok() {
                    NrStatus::Ok
                } else {
                    NrStatus::Invalid
                };
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
                        ctx.state_per_sid.remove(&sid);
                    }
                    NrStatus::Ok
                }
                Err(tokio::sync::mpsc::error::TrySendError::Full(_)) => NrStatus::Backpressure,
                Err(tokio::sync::mpsc::error::TrySendError::Closed(_)) => {
                    entry.remove();
                    ctx.state_per_sid.remove(&sid);
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
