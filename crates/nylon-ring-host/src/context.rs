//! Host context and thread-local state management.

use crate::types::{FastPendingMap, FastStateMap, Pending, UnaryResultSlot, UnarySender};
use nylon_ring::NrHostExt;
use rustc_hash::FxBuildHasher;
use std::cell::{Cell, RefCell};
use std::collections::HashMap;

/// Host context shared with the plugin (opaque on the plugin side).
///
/// Note:
/// - `host_ext` now lives **inside** `HostContext` → no dangling pointer when
///   `NylonRingHost` is dropped.
#[repr(C)]
pub(crate) struct HostContext {
    pub(crate) pending_requests: FastPendingMap,
    pub(crate) state_per_sid: FastStateMap,
    pub(crate) host_ext: NrHostExt,
}

impl HostContext {
    /// Create a new host context with the given extension callbacks.
    pub(crate) fn new(host_ext: NrHostExt) -> Self {
        Self {
            pending_requests: FastPendingMap::with_hasher(FxBuildHasher),
            state_per_sid: FastStateMap::with_hasher(FxBuildHasher),
            host_ext,
        }
    }
}

// Safety: HostContext can be safely shared across threads because:
// - FastPendingMap and FastStateMap (DashMap) are thread-safe
// - NrHostExt only contains function pointers, which are Send + Sync
unsafe impl Send for HostContext {}
unsafe impl Sync for HostContext {}

thread_local! {
    /// Fast-path for synchronous unary calls using oneshot channels.
    /// Used by `call_response` method.
    pub(crate) static CURRENT_UNARY_TX: Cell<*mut UnarySender> =
        const { Cell::new(std::ptr::null_mut()) };

    /// Ultra-fast path: plugin writes result directly to this slot.
    /// Used by `call_response_fast` method for synchronous plugins.
    pub(crate) static CURRENT_UNARY_RESULT: Cell<*mut UnaryResultSlot> =
        const { Cell::new(std::ptr::null_mut()) };

    /// Thread-local pending requests for same-thread optimization.
    /// Reduces contention on the global DashMap for same-thread request-response patterns.
    pub(crate) static THREAD_LOCAL_PENDING: RefCell<HashMap<u64, Pending, FxBuildHasher>> =
        const { RefCell::new(HashMap::with_hasher(FxBuildHasher)) };
}

/// Insert a pending request, preferring thread-local storage.
///
/// This reduces contention on the global DashMap when the request and callback
/// happen on the same thread.
pub(crate) fn insert_pending(_ctx: &HostContext, sid: u64, pending: Pending) {
    THREAD_LOCAL_PENDING.with(|map| {
        map.borrow_mut().insert(sid, pending);
    });
}

/// Remove and return a pending request.
///
/// Checks thread-local first (zero contention), then falls back to global map
/// for cross-thread cases.
pub(crate) fn remove_pending(ctx: &HostContext, sid: u64) -> Option<Pending> {
    // Try thread-local first
    let local_result = THREAD_LOCAL_PENDING.with(|map| map.borrow_mut().remove(&sid));

    if local_result.is_some() {
        return local_result;
    }

    // Fall back to global map for cross-thread cases
    ctx.pending_requests.remove(&sid).map(|(_, v)| v)
}

/// Insert a pending request back into the map (for streaming continuations).
///
/// Uses the global map since streaming responses may come from different threads.
pub(crate) fn reinsert_pending(ctx: &HostContext, sid: u64, pending: Pending) {
    ctx.pending_requests.insert(sid, pending);
}
