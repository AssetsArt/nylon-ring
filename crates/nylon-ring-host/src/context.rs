use crate::types::{FastPendingMap, FastStateMap, Pending, UnaryResultSlot, UnarySender};
use nylon_ring::NrHostExt;
use rustc_hash::FxBuildHasher;
use std::cell::Cell;

/// Number of shards for the pending requests.
const SHARD_COUNT: usize = 64;
const SHARD_MASK: usize = SHARD_COUNT - 1;

/// Host context shared with the plugin.
#[repr(C)]
pub(crate) struct HostContext {
    /// Sharded Pending Map Storage
    pub(crate) pending_shards: Box<[FastPendingMap]>,

    pub(crate) state_per_sid: FastStateMap,
    pub(crate) host_ext: NrHostExt,
}

impl HostContext {
    pub(crate) fn new(host_ext: NrHostExt) -> Self {
        let mut shards = Vec::with_capacity(SHARD_COUNT);
        for _ in 0..SHARD_COUNT {
            shards.push(FastPendingMap::with_hasher(FxBuildHasher));
        }

        Self {
            pending_shards: shards.into_boxed_slice(),
            state_per_sid: FastStateMap::with_hasher(FxBuildHasher),
            host_ext,
        }
    }
}

// Safety: OK
unsafe impl Send for HostContext {}
unsafe impl Sync for HostContext {}

#[inline(always)]
fn get_shard(ctx: &HostContext, sid: u64) -> &FastPendingMap {
    unsafe {
        ctx.pending_shards
            .get_unchecked((sid as usize) & SHARD_MASK)
    }
}

/// Insert a pending request.
pub(crate) fn insert_pending(ctx: &HostContext, sid: u64, pending: Pending) {
    get_shard(ctx, sid).insert(sid, pending);
}

/// Remove and return a pending request.
pub(crate) fn remove_pending(ctx: &HostContext, sid: u64) -> Option<Pending> {
    get_shard(ctx, sid).remove(&sid).map(|(_, v)| v)
}

/// Reinsert a pending request (used for streaming continuations).
pub(crate) fn reinsert_pending(ctx: &HostContext, sid: u64, pending: Pending) {
    // Always insert into Global Shard for continuations to support cross-thread access
    get_shard(ctx, sid).insert(sid, pending);
}

// --- Thread Local Optimization for Unary Results ---
thread_local! {
    pub(crate) static CURRENT_UNARY_RESULT: Cell<*mut UnaryResultSlot> = const { Cell::new(std::ptr::null_mut()) };
    pub(crate) static CURRENT_UNARY_TX: Cell<*mut UnarySender> = const { Cell::new(std::ptr::null_mut()) };
}
