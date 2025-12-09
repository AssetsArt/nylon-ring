use crate::types::{FastPendingMap, FastStateMap, Pending, UnaryResultSlot, UnarySender};
use nylon_ring::NrHostExt;
use rustc_hash::FxBuildHasher;
use std::cell::Cell;

/// Number of shards for the pending requests map.
/// Must be a power of two for efficient masking (e.g., 64 = 2^6).
const SHARD_COUNT: usize = 64;

/// Bitmask for efficient shard indexing.
const SHARD_MASK: usize = SHARD_COUNT - 1;

/// Host context shared with the plugin (opaque on the plugin side).
///
/// Note:
/// - `host_ext` now lives **inside** `HostContext` → no dangling pointer when
///   `NylonRingHost` is dropped.
#[repr(C)]
pub(crate) struct HostContext {
    /// Sharded map of pending requests.
    /// Used `Box<[FastPendingMap]>` instead of Vec for potentially better pointer indirection
    /// and fixed size semantics.
    pub(crate) pending_shards: Box<[FastPendingMap]>,

    pub(crate) state_per_sid: FastStateMap,
    pub(crate) host_ext: NrHostExt,
}

impl HostContext {
    /// Create a new host context with the given extension callbacks.
    pub(crate) fn new(host_ext: NrHostExt) -> Self {
        // Manual initialization of shards because DashMap::new is not const.
        let mut shards = Vec::with_capacity(SHARD_COUNT);
        for _ in 0..SHARD_COUNT {
            shards.push(FastPendingMap::with_hasher(FxBuildHasher::default()));
        }

        Self {
            pending_shards: shards.into_boxed_slice(),
            state_per_sid: FastStateMap::with_hasher(FxBuildHasher::default()),
            host_ext,
        }
    }
}

// Safety: HostContext can be safely shared across threads because:
// - FastPendingMap and FastStateMap (DashMap) are thread-safe
// - NrHostExt only contains function pointers, which are Send + Sync
unsafe impl Send for HostContext {}
unsafe impl Sync for HostContext {}

// --- Helper: Shard Indexing ---

/// Get the pending request shard for a given Session ID.
#[inline(always)]
fn get_shard(ctx: &HostContext, sid: u64) -> &FastPendingMap {
    // Safety: we ensure SHARD_COUNT initialization in `new`, and the mask guarantees
    // the index is within bounds [0, SHARD_COUNT - 1].
    unsafe {
        ctx.pending_shards
            .get_unchecked((sid as usize) & SHARD_MASK)
    }
}

// --- Thread Locals (Fast Paths) ---

thread_local! {
    /// Fast-path for synchronous unary calls using oneshot channels.
    /// Used by `call_response` method.
    pub(crate) static CURRENT_UNARY_TX: Cell<*mut UnarySender> =
        const { Cell::new(std::ptr::null_mut()) };

    /// Ultra-fast path: plugin writes result directly to this slot.
    /// Used by `call_response_fast` method for synchronous plugins.
    pub(crate) static CURRENT_UNARY_RESULT: Cell<*mut UnaryResultSlot> =
        const { Cell::new(std::ptr::null_mut()) };
}

// --- Operations ---

/// Insert a pending request into the appropriate shard.
#[inline]
pub(crate) fn insert_pending(ctx: &HostContext, sid: u64, pending: Pending) {
    get_shard(ctx, sid).insert(sid, pending);
}

/// Remove and return a pending request from the appropriate shard.
#[inline]
pub(crate) fn remove_pending(ctx: &HostContext, sid: u64) -> Option<Pending> {
    get_shard(ctx, sid).remove(&sid).map(|(_, v)| v)
}

/// Insert a pending request back into the map (for streaming continuations).
#[inline]
pub(crate) fn reinsert_pending(ctx: &HostContext, sid: u64, pending: Pending) {
    get_shard(ctx, sid).insert(sid, pending);
}
