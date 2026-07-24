//! Type definitions and aliases for the nylon-ring-host crate.

use crate::error::NylonRingHostError;
use crate::stream_channel::{StreamChannelReceiver, StreamSender};
use crate::{PluginCallGuard, context, context::HostContext};
use dashmap::DashMap;
use nylon_ring::NrStatus;
use rustc_hash::FxBuildHasher;
use std::collections::HashMap;
use std::sync::Arc;
use std::task::Waker;

/// Result type alias for this crate.
pub type Result<T> = std::result::Result<T, NylonRingHostError>;

/// Pending request state.
#[derive(Debug)]
pub(crate) enum Pending {
    Unary(UnaryPending),
    Stream(StreamSender),
}

/// Completion state stored inline in the pending-request map.
#[derive(Debug)]
pub(crate) enum UnaryPending {
    Waiting(Option<Waker>),
    Ready(NrStatus, ResponsePayload),
}

/// A plugin-owned buffer received over ABI v2; releases it on drop.
///
/// v2 contract (`NrOwnedBytesV2`): the bytes stay valid and immutable until
/// the consumer calls `release` exactly once, from any thread — which is
/// what makes this Send + Sync. Dropping it runs plugin code, so the holder
/// must guarantee the plugin library is still loaded (via a call guard or by
/// being inside a plugin callback).
#[derive(Debug)]
pub(crate) struct ForeignBytes {
    ptr: *const u8,
    len: usize,
    owner_ctx: *mut std::ffi::c_void,
    release: Option<unsafe extern "C" fn(*mut std::ffi::c_void, *const u8, u64)>,
}

// SAFETY: see the type documentation — immutability until release plus a
// thread-agnostic release callback are part of the v2 ABI contract.
unsafe impl Send for ForeignBytes {}
unsafe impl Sync for ForeignBytes {}

impl ForeignBytes {
    /// Takes ownership of a payload received from a plugin.
    pub(crate) fn from_abi(payload: nylon_ring::NrOwnedBytesV2) -> Self {
        Self {
            ptr: payload.ptr,
            len: payload.len as usize,
            owner_ctx: payload.owner_ctx,
            release: payload.release,
        }
    }

    pub(crate) fn as_slice(&self) -> &[u8] {
        if self.ptr.is_null() || self.len == 0 {
            return &[];
        }
        // SAFETY: the producer guarantees ptr..ptr+len stays valid and
        // immutable until release (v2 ABI contract).
        unsafe { std::slice::from_raw_parts(self.ptr, self.len) }
    }
}

impl Drop for ForeignBytes {
    fn drop(&mut self) {
        if let Some(release) = self.release {
            // SAFETY: called exactly once with the values the producer
            // handed over; Drop runs at most once.
            unsafe { release(self.owner_ctx, self.ptr, self.len as u64) };
        }
    }
}

/// A response payload that is either host-owned or plugin-owned.
#[derive(Debug)]
pub(crate) enum ResponsePayload {
    Owned(Vec<u8>),
    Foreign(ForeignBytes),
}

impl ResponsePayload {
    pub(crate) fn as_slice(&self) -> &[u8] {
        match self {
            Self::Owned(data) => data,
            Self::Foreign(foreign) => foreign.as_slice(),
        }
    }

    /// Converts into a host-owned `Vec`, copying (and releasing) a foreign
    /// payload. Callers must hold the plugin alive across this call.
    pub(crate) fn into_vec(self) -> Vec<u8> {
        match self {
            Self::Owned(data) => data,
            Self::Foreign(foreign) => foreign.as_slice().to_vec(),
        }
    }
}

/// A unary response as returned by [`crate::PluginHandle::call_response_bytes`].
///
/// For a v2 plugin that responded with plugin-owned bytes this is a
/// zero-copy view: the buffer is released back to the plugin when the value
/// drops, and an in-flight call guard held inside keeps the plugin library
/// loaded until then.
pub struct ResponseBytes {
    payload: ResponsePayload,
    /// Keeps the plugin loaded while a foreign payload is alive; dropped
    /// after `payload` would be a bug, so field order matters (payload
    /// first).
    _guard: Option<PluginCallGuard>,
}

impl ResponseBytes {
    pub(crate) fn new(payload: ResponsePayload, guard: Option<PluginCallGuard>) -> Self {
        Self {
            payload,
            _guard: guard,
        }
    }

    /// Converts into a host-owned `Vec<u8>` (copies only if the payload is
    /// still plugin-owned).
    pub fn into_vec(self) -> Vec<u8> {
        // Field-by-field move keeps the guard alive until after the copy.
        let Self { payload, _guard } = self;
        payload.into_vec()
    }
}

impl std::ops::Deref for ResponseBytes {
    type Target = [u8];

    fn deref(&self) -> &[u8] {
        self.payload.as_slice()
    }
}

impl AsRef<[u8]> for ResponseBytes {
    fn as_ref(&self) -> &[u8] {
        self.payload.as_slice()
    }
}

impl std::fmt::Debug for ResponseBytes {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ResponseBytes")
            .field("len", &self.payload.as_slice().len())
            .field(
                "foreign",
                &matches!(self.payload, ResponsePayload::Foreign(_)),
            )
            .finish()
    }
}

impl UnaryPending {
    pub(crate) const fn waiting() -> Self {
        Self::Waiting(None)
    }
}

/// A frame in a streaming response.
#[derive(Debug)]
pub struct StreamFrame {
    pub status: NrStatus,
    pub data: Vec<u8>,
}

/// A receiver for streaming responses.
///
/// Dropping the receiver unregisters its pending stream from the host.
pub struct StreamReceiver {
    inner: StreamChannelReceiver,
    /// Populated only when no call guard is held (host-internal uses); with a
    /// guard the context is reached through the guarded plugin instead of
    /// paying a second shared refcount round-trip per stream.
    host_ctx: Option<Arc<HostContext>>,
    sid: u64,
    call_guard: Option<PluginCallGuard>,
}

impl StreamReceiver {
    pub(crate) fn new(
        inner: StreamChannelReceiver,
        host_ctx: Option<Arc<HostContext>>,
        sid: u64,
        call_guard: Option<PluginCallGuard>,
    ) -> Self {
        debug_assert!(
            host_ctx.is_some() || call_guard.is_some(),
            "stream receiver needs a context source"
        );
        Self {
            inner,
            host_ctx,
            sid,
            call_guard,
        }
    }

    fn host_ctx(&self) -> &HostContext {
        match (&self.call_guard, &self.host_ctx) {
            (Some(guard), _) => &guard.plugin().host_ctx,
            (None, Some(host_ctx)) => host_ctx,
            (None, None) => unreachable!("checked at construction"),
        }
    }

    /// Waits for the next stream frame.
    pub async fn recv(&mut self) -> Option<StreamFrame> {
        self.inner.recv().await
    }
}

impl Drop for StreamReceiver {
    fn drop(&mut self) {
        // The call guard (if any) outlives this cleanup: Drop::drop runs
        // before the struct's fields are dropped. Removing the pending entry
        // drops the channel's sender under the shard lock, which is what
        // makes recycling the receiving half sound.
        context::cleanup_sid(self.host_ctx(), self.sid);
        self.inner.recycle();
    }
}

/// Fast hash map for pending requests using FxHash.
pub(crate) type FastPendingMap = DashMap<u64, Pending, FxBuildHasher>;

/// Fast hash map for per-SID state using FxHash.
pub(crate) type FastStateMap = DashMap<u64, HashMap<String, Vec<u8>>, FxBuildHasher>;

/// Result slot for an in-progress synchronous fast-path call.
pub(crate) struct UnaryResultSlot {
    pub(crate) sid: u64,
    pub(crate) result: Option<(NrStatus, Vec<u8>)>,
}
