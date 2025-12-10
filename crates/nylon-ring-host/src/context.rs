use crate::types::{
    FastPendingMap, FastStateMap, Pending, StreamReceiver, UnaryResultSlot, UnarySender,
};
use dashmap::DashMap;
use nylon_ring::NrHostExt;
use rustc_hash::FxBuildHasher;
use std::cell::Cell;
use std::sync::{Arc, Weak};

/// Number of shards for the pending requests.
const SHARD_COUNT: usize = 64;
const SHARD_MASK: usize = SHARD_COUNT - 1;

/// Host context shared with the plugin.
#[repr(C)]
pub(crate) struct HostContext {
    /// Sharded Pending Map Storage
    pub(crate) pending_shards: Box<[FastPendingMap]>,

    pub(crate) state_per_sid: FastStateMap,
    pub(crate) host_ext: NrHostExt,

    /// Weak reference to the plugin registry to avoid cycles.
    pub(crate) registry_weak: Weak<DashMap<String, Arc<crate::LoadedPlugin>>>,

    /// Active stream receivers (poll-based reading for plugins)
    pub(crate) stream_receivers: DashMap<u64, StreamReceiver, FxBuildHasher>,

    /// Map SID to Target Plugin (for stream writes/closes)
    pub(crate) stream_targets: DashMap<u64, Arc<crate::LoadedPlugin>, FxBuildHasher>,
}

impl HostContext {
    pub(crate) fn new(
        host_ext: NrHostExt,
        registry_weak: Weak<DashMap<String, Arc<crate::LoadedPlugin>>>,
    ) -> Self {
        let mut shards = Vec::with_capacity(SHARD_COUNT);
        for _ in 0..SHARD_COUNT {
            shards.push(FastPendingMap::with_hasher(FxBuildHasher));
        }

        Self {
            pending_shards: shards.into_boxed_slice(),
            state_per_sid: FastStateMap::with_hasher(FxBuildHasher),
            host_ext,
            registry_weak,
            stream_receivers: DashMap::with_hasher(FxBuildHasher),
            stream_targets: DashMap::with_hasher(FxBuildHasher),
        }
    }

    /// Resolve a plugin by name from the registry.
    pub(crate) fn get_plugin(&self, name: &str) -> Option<Arc<crate::LoadedPlugin>> {
        if let Some(registry) = self.registry_weak.upgrade() {
            registry.get(name).map(|p| p.clone())
        } else {
            None
        }
    }
}

// Safety: OK
unsafe impl Send for HostContext {}
unsafe impl Sync for HostContext {}

// --- Operations (Sharded Map) ---

// Thread-Local Shard Index (Sticky Sharding to avoid contention)
thread_local! {
    static LOCAL_SHARD_IDX: usize = {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};
        // Hash Thread ID to get a stable random shard for this thread
        let thread_id = std::thread::current().id();
        let mut hasher = DefaultHasher::new();
        thread_id.hash(&mut hasher);
        (hasher.finish() as usize) & (SHARD_COUNT - 1)
    };
}

#[inline(always)]
fn get_shard(ctx: &HostContext, sid: u64) -> &FastPendingMap {
    unsafe {
        ctx.pending_shards
            .get_unchecked((sid as usize) & SHARD_MASK)
    }
}

/// Insert a pending request.
pub(crate) fn insert_pending(ctx: &HostContext, sid: u64, pending: Pending) {
    get_shard(ctx, sid).insert(sid, pending);
}

/// Remove and return a pending request.
pub(crate) fn remove_pending(ctx: &HostContext, sid: u64) -> Option<Pending> {
    get_shard(ctx, sid).remove(&sid).map(|(_, v)| v)
}

/// Reinsert a pending request (used for streaming continuations).
pub(crate) fn reinsert_pending(ctx: &HostContext, sid: u64, pending: Pending) {
    // Always insert into Global Shard for continuations to support cross-thread access
    get_shard(ctx, sid).insert(sid, pending);
}

// --- Stream Management ---

pub(crate) fn insert_stream_receiver(ctx: &HostContext, sid: u64, rx: StreamReceiver) {
    ctx.stream_receivers.insert(sid, rx);
}

pub(crate) fn get_stream_receiver(
    ctx: &HostContext,
    sid: u64,
) -> Option<dashmap::mapref::one::RefMut<'_, u64, StreamReceiver>> {
    ctx.stream_receivers.get_mut(&sid)
}

pub(crate) fn insert_stream_target(ctx: &HostContext, sid: u64, plugin: Arc<crate::LoadedPlugin>) {
    ctx.stream_targets.insert(sid, plugin);
}

pub(crate) fn get_stream_target(ctx: &HostContext, sid: u64) -> Option<Arc<crate::LoadedPlugin>> {
    ctx.stream_targets.get(&sid).map(|p| p.clone())
}

// --- Thread Local Optimization for Unary Results ---
thread_local! {
    pub(crate) static CURRENT_UNARY_RESULT: Cell<*mut UnaryResultSlot> = const { Cell::new(std::ptr::null_mut()) };
    pub(crate) static CURRENT_UNARY_TX: Cell<*mut UnarySender> = const { Cell::new(std::ptr::null_mut()) };
}
