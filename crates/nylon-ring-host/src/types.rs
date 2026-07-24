//! Type definitions and aliases for the nylon-ring-host crate.

use crate::error::NylonRingHostError;
use crate::{context, context::HostContext};
use dashmap::DashMap;
use nylon_ring::NrStatus;
use rustc_hash::FxBuildHasher;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{mpsc, oneshot};

/// Result type alias for this crate.
pub type Result<T> = std::result::Result<T, NylonRingHostError>;

/// Pending request state.
#[derive(Debug)]
pub(crate) enum Pending {
    #[allow(dead_code)]
    Unary(oneshot::Sender<(NrStatus, Vec<u8>)>),
    Stream(mpsc::UnboundedSender<StreamFrame>),
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
    inner: mpsc::UnboundedReceiver<StreamFrame>,
    host_ctx: Arc<HostContext>,
    sid: u64,
}

impl StreamReceiver {
    pub(crate) fn new(
        inner: mpsc::UnboundedReceiver<StreamFrame>,
        host_ctx: Arc<HostContext>,
        sid: u64,
    ) -> Self {
        Self {
            inner,
            host_ctx,
            sid,
        }
    }

    /// Waits for the next stream frame.
    pub async fn recv(&mut self) -> Option<StreamFrame> {
        self.inner.recv().await
    }
}

impl Drop for StreamReceiver {
    fn drop(&mut self) {
        context::cleanup_sid(&self.host_ctx, self.sid);
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
