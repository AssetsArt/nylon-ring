//! Host context and thread-local state management.

use crate::types::{FastPendingMap, FastStateMap, UnaryResultSlot, UnarySender};
use nylon_ring::NrHostExt;
use rustc_hash::FxBuildHasher;
use std::cell::Cell;

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
}
