use crate::types::{
    FastPendingMap, FastStateMap, Pending, ResponsePayload, StreamFrame, UnaryPending,
    UnaryResultSlot,
};
use dashmap::mapref::entry::Entry as DashEntry;
use nylon_ring::{NrHostExt, NrStatus};
use rustc_hash::FxBuildHasher;
use std::cell::Cell;
use std::collections::HashMap;
use std::future::poll_fn;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::task::Poll;

/// Number of shards for the pending requests.
const SHARD_COUNT: usize = 64;
const SHARD_MASK: usize = SHARD_COUNT - 1;

/// Host context shared with the plugin.
pub(crate) struct HostContext {
    /// Sharded pending storage is allocated only when a tracked call is made.
    pending_shards: OnceLock<Box<[FastPendingMap]>>,

    pub(crate) state_per_sid: FastStateMap,
    state_shard_counts: [AtomicUsize; SHARD_COUNT],
    pub(crate) host_ext: NrHostExt,
    stream_capacity: usize,

    /// Leased response buffers whose call the host abandoned (timeout,
    /// cancel, receiver drop) while the plugin may still write into them.
    /// They are freed on a late commit or when this context drops — by
    /// which point the plugin's shutdown contract forbids further writes.
    /// Only cold paths touch this lock.
    orphaned_leases: std::sync::Mutex<Vec<Vec<u8>>>,
}

impl HostContext {
    pub(crate) fn new(host_ext: NrHostExt, stream_capacity: usize) -> Self {
        Self {
            pending_shards: OnceLock::new(),
            state_per_sid: FastStateMap::with_hasher(FxBuildHasher),
            state_shard_counts: std::array::from_fn(|_| AtomicUsize::new(0)),
            host_ext,
            stream_capacity,
            orphaned_leases: std::sync::Mutex::new(Vec::new()),
        }
    }

    /// Keep an abandoned lease alive instead of freeing memory the plugin
    /// may still be writing to.
    pub(crate) fn park_orphan_lease(&self, lease: Vec<u8>) {
        self.orphaned_leases.lock().unwrap().push(lease);
    }

    /// Free the orphaned lease identified by `token` (its buffer address),
    /// if present. Called on a late commit — the plugin's commit is its
    /// promise that it will not touch the buffer again.
    pub(crate) fn release_orphan_lease(&self, token: u64) -> bool {
        let mut orphans = self.orphaned_leases.lock().unwrap();
        if let Some(index) = orphans
            .iter()
            .position(|lease| lease.as_ptr() as u64 == token)
        {
            orphans.swap_remove(index);
            true
        } else {
            false
        }
    }

    #[cfg(test)]
    pub(crate) fn orphaned_lease_count(&self) -> usize {
        self.orphaned_leases.lock().unwrap().len()
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

    pub(crate) fn set_state(&self, sid: u64, key: String, value: Vec<u8>) {
        match self.state_per_sid.entry(sid) {
            DashEntry::Occupied(mut entry) => {
                entry.get_mut().insert(key, value);
            }
            DashEntry::Vacant(entry) => {
                let mut state = HashMap::new();
                state.insert(key, value);
                self.state_shard_counts[(sid as usize) & SHARD_MASK]
                    .fetch_add(1, Ordering::Release);
                entry.insert(state);
            }
        }
    }

    /// Remove state only when the SID's occupancy shard can contain entries.
    /// The common no-state call path avoids locking the DashMap entirely.
    pub(crate) fn remove_state(&self, sid: u64) {
        let count = &self.state_shard_counts[(sid as usize) & SHARD_MASK];
        if count.load(Ordering::Acquire) == 0 {
            return;
        }
        if self.state_per_sid.remove(&sid).is_some() {
            count.fetch_sub(1, Ordering::AcqRel);
        }
    }

    pub(crate) fn state_count(&self) -> usize {
        self.state_shard_counts
            .iter()
            .map(|count| count.load(Ordering::Acquire))
            .sum()
    }
}

#[inline(always)]
fn get_shard(ctx: &HostContext, sid: u64) -> &FastPendingMap {
    // Shard by the SID's thread-local allocation block (blocks are 2^20 wide)
    // instead of the low bits. Consecutive SIDs from one thread then stay in
    // one shard, so a thread's insert/complete/remove traffic does not migrate
    // the same map cache lines across every core.
    &ctx.pending_shards()[((sid >> 20) as usize) & SHARD_MASK]
}

/// Insert a pending request.
pub(crate) fn insert_pending(ctx: &HostContext, sid: u64, pending: Pending) {
    get_shard(ctx, sid).insert(sid, pending);
}

/// Remove and return a pending request.
pub(crate) fn remove_pending(ctx: &HostContext, sid: u64) -> Option<Pending> {
    get_shard(ctx, sid).remove(&sid).map(|(_, pending)| pending)
}

/// Wait for a unary result stored in the pending map.
///
/// The synchronous response case reaches `Ready` before this future is first
/// polled, so it neither allocates a channel nor clones a waker. For a delayed
/// response, the callback takes and wakes the latest registered waker after
/// releasing the shard lock.
pub(crate) async fn wait_for_unary(
    ctx: &HostContext,
    sid: u64,
) -> Option<(NrStatus, ResponsePayload)> {
    poll_fn(|cx| {
        let mut replacement = None;
        loop {
            match get_shard(ctx, sid).entry(sid) {
                DashEntry::Vacant(entry) => {
                    drop(entry);
                    drop(replacement);
                    return Poll::Ready(None);
                }
                DashEntry::Occupied(mut entry) => match entry.get_mut() {
                    Pending::Unary(UnaryPending::Ready(_, _)) => {
                        let Pending::Unary(UnaryPending::Ready(status, data)) = entry.remove()
                        else {
                            unreachable!();
                        };
                        drop(replacement);
                        return Poll::Ready(Some((status, data)));
                    }
                    Pending::Unary(UnaryPending::Waiting { waker, .. }) => {
                        if waker
                            .as_ref()
                            .is_some_and(|waker| waker.will_wake(cx.waker()))
                        {
                            drop(entry);
                            drop(replacement);
                            return Poll::Pending;
                        }
                        if let Some(replacement) = replacement.take() {
                            let previous = waker.replace(replacement);
                            drop(entry);
                            drop(previous);
                            return Poll::Pending;
                        }
                        drop(entry);
                        replacement = Some(cx.waker().clone());
                    }
                    Pending::Stream(_) => {
                        drop(entry);
                        drop(replacement);
                        return Poll::Ready(None);
                    }
                },
            }
        }
    })
    .await
}

/// Remove all host-owned state associated with a completed SID.
///
/// An outstanding lease in the removed entry is parked, not freed: the
/// plugin may still write into it (see `HostContext::orphaned_leases`).
pub(crate) fn cleanup_sid(ctx: &HostContext, sid: u64) {
    if let Some(Pending::Unary(UnaryPending::Waiting {
        lease: Some(lease), ..
    })) = remove_pending(ctx, sid)
    {
        ctx.park_orphan_lease(lease);
    }
    ctx.remove_state(sid);
}

/// Deliver a result while holding one shard entry lock for the whole state
/// transition. This prevents remove/reinsert and terminal-frame races.
///
/// A foreign (plugin-owned) payload is stored as-is for unary requests; for
/// streams it is copied into the frame and its release runs after the shard
/// lock is dropped (release is plugin code and must not run under a lock).
pub(crate) fn dispatch_pending(
    ctx: &HostContext,
    sid: u64,
    status: NrStatus,
    payload: ResponsePayload,
) -> NrStatus {
    // Holds a foreign payload consumed by the stream arm so its release
    // callback runs after the entry lock below is released.
    let mut deferred_release = None;
    let dispatch_status = match get_shard(ctx, sid).entry(sid) {
        DashEntry::Vacant(_) => {
            deferred_release = Some(payload);
            NrStatus::Invalid
        }
        DashEntry::Occupied(mut entry) => {
            if let Pending::Unary(pending) = entry.get_mut() {
                let (waker, stale_lease) = match pending {
                    UnaryPending::Waiting { waker, lease } => (waker.take(), lease.take()),
                    UnaryPending::Ready(_, _) => {
                        drop(entry);
                        drop(payload);
                        return NrStatus::Invalid;
                    }
                };
                *pending = UnaryPending::Ready(status, payload);
                drop(entry);
                // A response through this path supersedes any outstanding
                // lease; park it in case the plugin still holds the pointer.
                if let Some(lease) = stale_lease {
                    ctx.park_orphan_lease(lease);
                }
                ctx.remove_state(sid);
                if let Some(waker) = waker {
                    waker.wake();
                }
                return NrStatus::Ok;
            }

            let terminal = status.is_terminal();
            let frame = StreamFrame {
                status,
                data: match payload {
                    ResponsePayload::Owned(data) => data,
                    ResponsePayload::Foreign(foreign) => {
                        let data = foreign.as_slice().to_vec();
                        deferred_release = Some(ResponsePayload::Foreign(foreign));
                        data
                    }
                },
            };
            // try_send borrows the sender in place; the borrow ends before
            // entry.remove(), so no per-frame Sender clone is needed. A live
            // pending entry implies a live receiver (its Drop removes the
            // entry first), so there is no closed-channel case here.
            let send_result = match entry.get() {
                Pending::Stream(tx) => tx.try_send(frame),
                Pending::Unary(_) => unreachable!(),
            };
            match send_result {
                Ok(()) => {
                    if terminal {
                        entry.remove();
                        ctx.remove_state(sid);
                    }
                    NrStatus::Ok
                }
                Err(_rejected_frame) => NrStatus::Backpressure,
            }
        }
    };
    drop(deferred_release);
    dispatch_status
}

/// Lease a host-owned response buffer for a pending unary request.
///
/// The buffer is allocated before the shard lock is taken and stored inside
/// the pending entry, so the same cleanup that removes the entry also
/// reclaims the lease. Fails for unknown sids, streaming sids, completed
/// requests, and sids that already hold an outstanding lease.
pub(crate) fn acquire_pending_lease(
    ctx: &HostContext,
    sid: u64,
    capacity: u64,
) -> nylon_ring::NrBufferLease {
    let Ok(capacity) = usize::try_from(capacity) else {
        return nylon_ring::NrBufferLease::failed();
    };
    let mut buffer: Vec<u8> = Vec::with_capacity(capacity);
    let granted = nylon_ring::NrBufferLease {
        ptr: buffer.as_mut_ptr(),
        cap: buffer.capacity() as u64,
        token: buffer.as_ptr() as u64,
    };
    match get_shard(ctx, sid).entry(sid) {
        DashEntry::Occupied(mut entry) => match entry.get_mut() {
            Pending::Unary(UnaryPending::Waiting {
                lease: lease @ None,
                ..
            }) => {
                *lease = Some(buffer);
                granted
            }
            _ => nylon_ring::NrBufferLease::failed(),
        },
        DashEntry::Vacant(_) => nylon_ring::NrBufferLease::failed(),
    }
}

/// Commit a leased buffer as the response to a pending unary request.
///
/// On a bad token or oversized `initialized_len` the lease stays stored (and
/// valid for a corrected retry); a commit for a sid the host has already
/// abandoned frees the matching orphaned lease and reports `Invalid`.
pub(crate) fn commit_pending_lease(
    ctx: &HostContext,
    sid: u64,
    status: NrStatus,
    token: u64,
    initialized_len: u64,
) -> NrStatus {
    let waker = match get_shard(ctx, sid).entry(sid) {
        DashEntry::Vacant(entry) => {
            drop(entry);
            ctx.release_orphan_lease(token);
            return NrStatus::Invalid;
        }
        DashEntry::Occupied(mut entry) => {
            let Pending::Unary(pending) = entry.get_mut() else {
                return NrStatus::Invalid;
            };
            let UnaryPending::Waiting { waker, lease } = pending else {
                return NrStatus::Invalid;
            };
            let Some(mut buffer) = lease.take() else {
                return NrStatus::Invalid;
            };
            if buffer.as_ptr() as u64 != token || initialized_len > buffer.capacity() as u64 {
                *lease = Some(buffer);
                return NrStatus::Invalid;
            }
            // SAFETY: initialized_len is within the buffer's capacity and
            // the plugin's commit asserts it wrote that prefix (ABI
            // contract; u8 needs no validity beyond initialization).
            unsafe { buffer.set_len(initialized_len as usize) };
            let waker = waker.take();
            *pending = UnaryPending::Ready(status, ResponsePayload::Owned(buffer));
            waker
        }
    };
    ctx.remove_state(sid);
    if let Some(waker) = waker {
        waker.wake();
    }
    NrStatus::Ok
}

// --- Thread Local Optimization for Unary Results ---
thread_local! {
    pub(crate) static CURRENT_UNARY_RESULT: Cell<*mut UnaryResultSlot> = const { Cell::new(std::ptr::null_mut()) };
}

/// Thread-local target for stream frames sent synchronously inside the
/// plugin's `handle` call: frames land directly in the channel without the
/// per-frame pending-map lookup. Frames sent from other threads (or after
/// `handle` returns) still route through the map.
pub(crate) struct StreamFrameSlot {
    pub(crate) sid: u64,
    pub(crate) chan: std::sync::Arc<crate::stream_channel::StreamChannel>,
    /// Set when a terminal frame went through this slot; the caller removes
    /// the pending entry after `handle` returns (the map path removes it
    /// inline instead).
    pub(crate) terminal_seen: bool,
}

thread_local! {
    pub(crate) static CURRENT_STREAM_FRAME: Cell<*mut StreamFrameSlot> = const { Cell::new(std::ptr::null_mut()) };
}
