use crate::types::{FastPendingMap, FastStateMap, Pending, UnaryResultSlot, UnarySender};
use nylon_ring::NrHostExt;
use rustc_hash::FxBuildHasher;
use std::cell::Cell;

/// จำนวน shard ต้องเป็น power-of-two (64 = 2^6)
const SHARD_COUNT: usize = 64;
const SHARD_MASK: usize = SHARD_COUNT - 1;

#[repr(C)]
pub(crate) struct HostContext {
    // ย้าย Shards เข้ามาอยู่ในนี้ (Box<[]> เพื่อให้ขนาด Struct เล็กและ Pointer indirection เร็ว)
    pub(crate) pending_shards: Box<[FastPendingMap]>,

    pub(crate) state_per_sid: FastStateMap,
    pub(crate) host_ext: NrHostExt,
}

impl HostContext {
    pub(crate) fn new(host_ext: NrHostExt) -> Self {
        // Init shards แบบ Manual (เพราะ DashMap::new ไม่ใช่ const)
        let mut shards = Vec::with_capacity(SHARD_COUNT);
        for _ in 0..SHARD_COUNT {
            shards.push(FastPendingMap::with_hasher(FxBuildHasher::default()));
        }

        Self {
            // Convert Vec -> Box<[T]> (Fixed size slice)
            pending_shards: shards.into_boxed_slice(),
            state_per_sid: FastStateMap::with_hasher(FxBuildHasher::default()),
            host_ext,
        }
    }
}

// Safety: OK
unsafe impl Send for HostContext {}
unsafe impl Sync for HostContext {}

// --- Helper: Shard Indexing ---
// Inline(always) เพื่อให้ Compiler เปลี่ยนเป็น Bitmask instruction ตัวเดียว
#[inline(always)]
fn get_shard(ctx: &HostContext, sid: u64) -> &FastPendingMap {
    // Safety: เรา init มาครบ SHARD_COUNT แน่นอน และ mask รับประกันว่า index ไม่เกิน
    unsafe {
        ctx.pending_shards
            .get_unchecked((sid as usize) & SHARD_MASK)
    }
}

// --- Thread Locals (Fast Paths) ---
thread_local! {
    pub(crate) static CURRENT_UNARY_TX: Cell<*mut UnarySender> =
        const { Cell::new(std::ptr::null_mut()) };

    pub(crate) static CURRENT_UNARY_RESULT: Cell<*mut UnaryResultSlot> =
        const { Cell::new(std::ptr::null_mut()) };
}

// --- Operations ---

#[inline]
pub(crate) fn insert_pending(ctx: &HostContext, sid: u64, pending: Pending) {
    get_shard(ctx, sid).insert(sid, pending);
}

#[inline]
pub(crate) fn remove_pending(ctx: &HostContext, sid: u64) -> Option<Pending> {
    // ไม่ต้องมี Fallback แล้วครับ Shard อย่างเดียว เร็วและชัวร์กว่า
    get_shard(ctx, sid).remove(&sid).map(|(_, v)| v)
}

#[inline]
pub(crate) fn reinsert_pending(ctx: &HostContext, sid: u64, pending: Pending) {
    get_shard(ctx, sid).insert(sid, pending);
}
