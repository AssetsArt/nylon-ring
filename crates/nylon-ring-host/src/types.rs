//! Type definitions and aliases for the nylon-ring-host crate.

use crate::error::NylonRingHostError;
use dashmap::DashMap;
use nylon_ring::NrStatus;
use rustc_hash::FxBuildHasher;
use std::collections::HashMap;
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
pub type StreamReceiver = mpsc::UnboundedReceiver<StreamFrame>;

/// Fast hash map for pending requests using FxHash.
pub(crate) type FastPendingMap = DashMap<u64, Pending, FxBuildHasher>;

/// Fast hash map for per-SID state using FxHash.
pub(crate) type FastStateMap = DashMap<u64, HashMap<String, Vec<u8>>, FxBuildHasher>;

/// Optional oneshot sender for unary responses.
pub(crate) type UnarySender = Option<oneshot::Sender<(NrStatus, Vec<u8>)>>;

/// Optional result slot for ultra-fast unary responses.
pub(crate) type UnaryResultSlot = Option<(NrStatus, Vec<u8>)>;
