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
    Ready(NrStatus, Vec<u8>),
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
            (Some(guard), _) => &guard.plugin.host_ctx,
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
