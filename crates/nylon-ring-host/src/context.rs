//! Host context and thread-local state management.

use crate::types::{FastStateMap, StreamFrame, UnaryResultSlot, UnarySender};
use crossbeam_utils::CachePadded;
use nylon_ring::{NrHostExt, NrStatus};
use parking_lot::Mutex;
use rustc_hash::FxBuildHasher;
use slab::Slab;
use std::cell::Cell;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::task::{Context, Poll, Waker};
use tokio::sync::mpsc::UnboundedSender;

/// Number of shards for the pending requests.
/// Must be a power of two (64 = 2^6).
const SHARD_COUNT: usize = 64;
const SHARD_BITS: usize = 6;
const SLOT_MASK: usize = (1 << (64 - SHARD_BITS)) - 1; // 58 bits for slot index

/// A slot in the slab designed for zero-allocation reuse.
pub(crate) struct RequestSlot {
    pub(crate) waker: Option<Waker>,
    pub(crate) stream_tx: Option<UnboundedSender<StreamFrame>>,
    pub(crate) data: Option<Vec<u8>>,
    pub(crate) status: NrStatus,
}

struct Shard {
    requests: Mutex<Slab<RequestSlot>>,
}

/// Host context shared with the plugin.
#[repr(C)]
pub(crate) struct HostContext {
    /// Sharded Slab Storage (Index-based, No Hashing).
    shards: Box<[CachePadded<Shard>]>,

    /// Round-robin counter for shard selection.
    next_shard: AtomicUsize,

    pub(crate) state_per_sid: FastStateMap,
    pub(crate) host_ext: NrHostExt,
}

impl HostContext {
    pub(crate) fn new(host_ext: NrHostExt) -> Self {
        let mut shards = Vec::with_capacity(SHARD_COUNT);
        for _ in 0..SHARD_COUNT {
            shards.push(CachePadded::new(Shard {
                requests: Mutex::new(Slab::with_capacity(1024)), // Pre-allocate some capacity
            }));
        }

        Self {
            shards: shards.into_boxed_slice(),
            next_shard: AtomicUsize::new(0),
            state_per_sid: FastStateMap::with_hasher(FxBuildHasher::default()),
            host_ext,
        }
    }
}

// Safety: OK
unsafe impl Send for HostContext {}
unsafe impl Sync for HostContext {}

// --- Operations (God Mode) ---

/// Insert a new request and return the encoded SID.
/// SID = [ Shard Index (6 bits) | Slot Index (58 bits) ]
pub(crate) fn insert_request(ctx: &HostContext) -> u64 {
    // 1. Select Shard (Round-robin)
    // Relaxed is fine for load balancing
    let shard_idx = ctx.next_shard.fetch_add(1, Ordering::Relaxed) & (SHARD_COUNT - 1);

    // 2. Lock Shard (Low contention due to 64 shards)
    let mut slab = ctx.shards[shard_idx].requests.lock();

    // 3. Insert into Slab (O(1), reuses memory)
    let entry = slab.vacant_entry();
    let slot_idx = entry.key();

    entry.insert(RequestSlot {
        waker: None,
        stream_tx: None,
        data: None,
        status: NrStatus::Ok, //Default
    });

    // 4. Encode SID
    ((shard_idx as u64) << (64 - SHARD_BITS)) | (slot_idx as u64)
}

/// Poll a pending request.
pub(crate) fn poll_request(
    ctx: &HostContext,
    sid: u64,
    cx: &mut Context<'_>,
) -> Poll<(NrStatus, Vec<u8>)> {
    let shard_idx = (sid >> (64 - SHARD_BITS)) as usize;
    let slot_idx = (sid as usize) & SLOT_MASK;

    if shard_idx >= SHARD_COUNT {
        return Poll::Ready((NrStatus::Invalid, Vec::new()));
    }

    let mut slab = ctx.shards[shard_idx].requests.lock();
    if let Some(slot) = slab.get_mut(slot_idx) {
        if let Some(data) = slot.data.take() {
            let status = slot.status;
            slab.remove(slot_idx); // Complete!
            return Poll::Ready((status, data));
        } else {
            // Register waker
            slot.waker = Some(cx.waker().clone());
            return Poll::Pending;
        }
    }
    Poll::Ready((NrStatus::Invalid, Vec::new())) // Slot disappeared?
}

/// Register a stream channel for a request.
pub(crate) fn register_stream(ctx: &HostContext, sid: u64, tx: UnboundedSender<StreamFrame>) {
    let shard_idx = (sid >> (64 - SHARD_BITS)) as usize;
    let slot_idx = (sid as usize) & SLOT_MASK;

    if shard_idx >= SHARD_COUNT {
        return;
    }

    let mut slab = ctx.shards[shard_idx].requests.lock();
    if let Some(slot) = slab.get_mut(slot_idx) {
        slot.stream_tx = Some(tx);
    }
}

/// Get access to a request slot if it exists.
/// Used by callbacks to complete the request.
pub(crate) fn with_request_slot<F>(ctx: &HostContext, sid: u64, f: F)
where
    F: FnOnce(&mut RequestSlot),
{
    // 1. Decode SID (O(1) Arithmetic)
    let shard_idx = (sid >> (64 - SHARD_BITS)) as usize;
    let slot_idx = (sid as usize) & SLOT_MASK;

    // Safety check: Shard index logic guarantees bounds, but being safe is good.
    if shard_idx >= SHARD_COUNT {
        return;
    }

    // 2. Lock Shard
    let mut slab = ctx.shards[shard_idx].requests.lock();

    // 3. Access Slot
    if let Some(slot) = slab.get_mut(slot_idx) {
        f(slot);
    }
}

/// Remove a request slot.
/// Used when the Future is dropped or completed.
pub(crate) fn remove_request(ctx: &HostContext, sid: u64) {
    let shard_idx = (sid >> (64 - SHARD_BITS)) as usize;
    let slot_idx = (sid as usize) & SLOT_MASK;

    if shard_idx >= SHARD_COUNT {
        return;
    }

    let mut slab = ctx.shards[shard_idx].requests.lock();
    if slab.contains(slot_idx) {
        slab.remove(slot_idx);
    }
}

// --- Thread Locals (Preserved Fast Paths) ---
thread_local! {
    pub(crate) static CURRENT_UNARY_TX: Cell<*mut UnarySender> =
        const { Cell::new(std::ptr::null_mut()) };

    pub(crate) static CURRENT_UNARY_RESULT: Cell<*mut UnaryResultSlot> =
        const { Cell::new(std::ptr::null_mut()) };
}
