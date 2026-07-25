//! Nylon Ring Host - A high-performance plugin host for the nylon-ring ABI.
//!
//! This crate provides the host-side implementation for loading and managing
//! plugins that conform to the nylon-ring ABI. It supports multiple execution
//! modes including fire-and-forget calls, request-response patterns, and
//! bidirectional streaming.

mod call_tracker;
mod callbacks;
mod context;
mod error;
mod extensions;
#[cfg(test)]
mod loom_tests;
mod sid;
mod stream_channel;
mod types;

use call_tracker::CallTracker;
use callbacks::{
    acquire_result_buffer_callback, commit_result_buffer_callback, get_state_callback,
    send_result_owned_callback, send_result_vec_callback, set_state_callback,
};
use context::{CURRENT_UNARY_RESULT, HostContext};
use libloading::{Library, Symbol};
use nylon_ring::{ABI_VERSION, NrBytes, NrHostExt, NrHostVTable, NrPluginInfo, NrStr};
use sid::next_sid;
use std::collections::HashMap;
use std::ffi::c_void;
use std::sync::Arc;
use std::time::Duration;

pub use error::NylonRingHostError;
pub use extensions::Extensions;
pub use nylon_ring::NrStatus;
pub use types::{ResponseBytes, Result, StreamFrame, StreamReceiver};

/// Default number of frames buffered for each response stream.
pub const DEFAULT_STREAM_CAPACITY: usize = 64;

/// Point-in-time host metrics suitable for application monitoring hooks.
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub struct HostMetrics {
    pub loaded_plugins: usize,
    pub pending_requests: usize,
    pub state_sessions: usize,
    pub in_flight_calls: usize,
}

struct PendingGuard<'a> {
    host_ctx: &'a HostContext,
    sid: u64,
    armed: bool,
}

impl<'a> PendingGuard<'a> {
    fn new(host_ctx: &'a HostContext, sid: u64) -> Self {
        Self {
            host_ctx,
            sid,
            armed: true,
        }
    }

    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for PendingGuard<'_> {
    fn drop(&mut self) {
        if self.armed {
            context::cleanup_sid(self.host_ctx, self.sid);
        }
    }
}

struct FastSlotBinding;

impl FastSlotBinding {
    fn bind(slot: &mut types::UnaryResultSlot) -> Result<Self> {
        let bound = CURRENT_UNARY_RESULT.with(|cell| {
            if !cell.get().is_null() {
                return false;
            }
            cell.set(slot as *mut _);
            true
        });
        if bound {
            Ok(Self)
        } else {
            Err(NylonRingHostError::FastPathReentrant)
        }
    }
}

impl Drop for FastSlotBinding {
    fn drop(&mut self) {
        CURRENT_UNARY_RESULT.with(|cell| cell.set(std::ptr::null_mut()));
    }
}

/// Binds the thread-local stream-frame slot for the duration of a plugin
/// `handle` call. Binding is best-effort: a reentrant `call_stream` on the
/// same thread simply falls back to the pending-map frame path.
struct StreamSlotBinding;

impl StreamSlotBinding {
    fn bind(slot: &mut context::StreamFrameSlot) -> Option<Self> {
        let bound = context::CURRENT_STREAM_FRAME.with(|cell| {
            if !cell.get().is_null() {
                return false;
            }
            cell.set(slot as *mut _);
            true
        });
        bound.then_some(Self)
    }
}

impl Drop for StreamSlotBinding {
    fn drop(&mut self) {
        context::CURRENT_STREAM_FRAME.with(|cell| cell.set(std::ptr::null_mut()));
    }
}

/// The plugin's dispatch surface, copied out of its vtable at load time
/// (`init` is only used during load and not retained).
#[derive(Copy, Clone)]
struct PluginDispatch {
    handle: Option<unsafe extern "C" fn(NrStr, u64, NrBytes) -> NrStatus>,
    shutdown: Option<unsafe extern "C" fn()>,
    stream_data: Option<unsafe extern "C" fn(u64, NrBytes) -> NrStatus>,
    stream_close: Option<unsafe extern "C" fn(u64) -> NrStatus>,
    resolve_entry: Option<unsafe extern "C" fn(NrStr) -> u32>,
    handle_by_id: Option<unsafe extern "C" fn(u32, u64, NrBytes) -> NrStatus>,
}

impl PluginDispatch {
    fn from_vtable(vtable: &nylon_ring::NrPluginVTable) -> Self {
        Self {
            handle: vtable.handle,
            shutdown: vtable.shutdown,
            stream_data: vtable.stream_data,
            stream_close: vtable.stream_close,
            resolve_entry: vtable.resolve_entry,
            handle_by_id: vtable.handle_by_id,
        }
    }
}

/// A loaded plugin instance.
pub struct LoadedPlugin {
    _lib: std::mem::ManuallyDrop<Library>,
    dispatch: PluginDispatch,
    host_ctx: Arc<HostContext>,
    path: String,
    call_tracker: CallTracker,
    /// Pinned plugins opt out of unload/reload entirely: per-call guards
    /// skip the tracker RMWs, and the library is leaked on drop instead of
    /// running shutdown/dlclose that can no longer be proven quiescent.
    /// Streams still take real tracker slots (their guards hold raw plugin
    /// pointers whose validity relies on the tracker).
    pinned: bool,
}

/// Sentinel shard for calls on pinned plugins: no tracker slot was taken
/// and `finish` must not run.
const PINNED_SHARD: usize = usize::MAX;

/// Plugins removed from a host while calls may still be in flight.
///
/// Owned call guards do not clone `Arc<LoadedPlugin>` (a per-stream RMW on
/// one shared cache line capped multi-core streaming); they hold a raw
/// pointer plus a tracker slot instead. That is sound only if the plugin
/// allocation outlives every tracker slot, so an Arc leaving a host's map
/// is never dropped directly: [`retire_plugin`] parks it here and the last
/// `finish` observing the stopped tracker sweeps it out.
static RETIRED_PLUGINS: std::sync::Mutex<Vec<Arc<LoadedPlugin>>> =
    std::sync::Mutex::new(Vec::new());

/// Stop a plugin removed from a host's map and keep it alive until every
/// tracked call has finished. Every code path where a host-map
/// `Arc<LoadedPlugin>` leaves the map must go through here (a plain drop
/// could free the plugin while a call guard still points at it).
fn retire_plugin(plugin: Arc<LoadedPlugin>) {
    plugin.stop_accepting_calls();
    RETIRED_PLUGINS.lock().unwrap().push(plugin);
    // The plugin may already be idle; free it now instead of waiting for
    // an unrelated future sweep.
    sweep_retired_plugins();
}

/// Drop every retired plugin whose tracked calls have drained.
///
/// Memory-ordering argument for "the last finisher always frees": `finish`
/// is an AcqRel RMW and `stop` is an AcqRel RMW on the same shard counters,
/// so for each shard either the finish precedes the stop (the stop/retire
/// thread's sweep then reads the drained value) or it follows it (the
/// finisher sees CLOSED and sweeps itself, happening-after all finishes the
/// stop observed). Two concurrent last finishers on different shards are the
/// store-buffering case; the SeqCst fence below pairs across sweeps so at
/// least one of them observes every shard drained.
fn sweep_retired_plugins() {
    std::sync::atomic::fence(std::sync::atomic::Ordering::SeqCst);
    let drained: Vec<Arc<LoadedPlugin>> = {
        let mut retired = RETIRED_PLUGINS.lock().unwrap();
        let (drained, live) = std::mem::take(&mut *retired)
            .into_iter()
            .partition(|plugin| plugin.call_tracker.active_calls() == 0);
        *retired = live;
        drained
    };
    // Dropping may run the plugin's shutdown callback and unload the
    // library; keep that outside the registry lock.
    drop(drained);
}

impl LoadedPlugin {
    fn begin_call(&self) -> Result<BorrowedPluginCallGuard<'_>> {
        if self.pinned {
            // The borrow ties the guard to a live `Arc<LoadedPlugin>` and a
            // pinned library is never unloaded, so no tracker slot is needed.
            return Ok(BorrowedPluginCallGuard {
                tracker: &self.call_tracker,
                shard: PINNED_SHARD,
            });
        }
        let shard = self
            .call_tracker
            .try_begin()
            .ok_or(NylonRingHostError::PluginUnloaded)?;
        Ok(BorrowedPluginCallGuard {
            tracker: &self.call_tracker,
            shard,
        })
    }

    fn begin_owned_call(self: &Arc<Self>) -> Result<PluginCallGuard> {
        let shard = self
            .call_tracker
            .try_begin()
            .ok_or(NylonRingHostError::PluginUnloaded)?;
        Ok(PluginCallGuard {
            plugin: Arc::as_ptr(self),
            shard,
        })
    }

    fn stop_accepting_calls(&self) {
        self.call_tracker.stop();
    }
}

struct BorrowedPluginCallGuard<'a> {
    tracker: &'a CallTracker,
    shard: usize,
}

impl Drop for BorrowedPluginCallGuard<'_> {
    fn drop(&mut self) {
        if self.shard != PINNED_SHARD && self.tracker.finish(self.shard) {
            sweep_retired_plugins();
        }
    }
}

/// Owned in-flight-call token, held by values that outlive their call
/// (currently [`StreamReceiver`] via `call_stream`).
///
/// Holds a raw plugin pointer instead of an `Arc` clone: every stream
/// cloning the same Arc made its refcount line the multi-core streaming
/// bottleneck. Validity: the tracker slot held below is released only in
/// this guard's `Drop`, and a `LoadedPlugin` is freed only when its host-map
/// Arc has been retired through [`retire_plugin`] *and* its tracker has
/// drained — so the pointee outlives the guard.
pub(crate) struct PluginCallGuard {
    plugin: *const LoadedPlugin,
    shard: usize,
}

// SAFETY: the guard behaves like a `&LoadedPlugin` with liveness (see the
// type docs); `LoadedPlugin` is Send + Sync (it is shared via Arc across
// threads today).
unsafe impl Send for PluginCallGuard {}
unsafe impl Sync for PluginCallGuard {}

impl PluginCallGuard {
    pub(crate) fn plugin(&self) -> &LoadedPlugin {
        // SAFETY: see the type documentation — the held tracker slot keeps
        // the plugin allocation alive for the guard's lifetime.
        unsafe { &*self.plugin }
    }
}

impl Drop for PluginCallGuard {
    fn drop(&mut self) {
        // After this `finish` the plugin pointer must not be dereferenced:
        // releasing the slot may allow a concurrent sweep to free it.
        if self.plugin().call_tracker.finish(self.shard) {
            sweep_retired_plugins();
        }
    }
}

impl Drop for LoadedPlugin {
    fn drop(&mut self) {
        if self.pinned {
            // Untracked borrowed calls may still be executing plugin code;
            // leak the library and skip shutdown rather than pull code pages
            // out from under them.
            return;
        }
        if let Some(shutdown_fn) = self.dispatch.shutdown {
            unsafe {
                shutdown_fn();
            }
        }
        // SAFETY: dropped exactly once here; the pinned branch above returns
        // without dropping, which is the intended leak.
        unsafe { std::mem::ManuallyDrop::drop(&mut self._lib) };
    }
}

/// A handle to a specific plugin for making calls.
#[derive(Clone)]
pub struct PluginHandle {
    plugin: Arc<LoadedPlugin>,
}

impl PluginHandle {
    fn status_error(status: NrStatus) -> NylonRingHostError {
        if status == NrStatus::Panic {
            NylonRingHostError::PluginPanicked
        } else {
            NylonRingHostError::PluginHandleFailed(status)
        }
    }

    /// Call a plugin entry point with a request-response pattern.
    pub async fn call_response(&self, entry: &str, payload: &[u8]) -> Result<(NrStatus, Vec<u8>)> {
        let _call_guard = self.plugin.begin_call()?;
        let sid = next_sid();

        context::insert_pending(
            &self.plugin.host_ctx,
            sid,
            types::Pending::Unary(types::UnaryPending::waiting()),
        );
        let mut pending_guard = PendingGuard::new(&self.plugin.host_ctx, sid);

        // Synchronous responses land in the thread-local slot without touching
        // the pending map again; the map entry (inserted above, before the
        // plugin runs) still catches responses from other threads. A reentrant
        // caller already owns the slot, so binding is best-effort.
        let mut slot = types::UnaryResultSlot {
            sid,
            result: None,
            lease: None,
        };
        let binding = FastSlotBinding::bind(&mut slot).ok();

        let payload_bytes = NrBytes::from_slice(payload);
        let handle_raw_fn = match self.plugin.dispatch.handle {
            Some(f) => f,
            None => return Err(NylonRingHostError::MissingRequiredFunctions),
        };

        let status = unsafe { handle_raw_fn(NrStr::new(entry), sid, payload_bytes) };

        drop(binding);
        // A lease the plugin acquired but never committed may still be
        // written to by misbehaving plugin threads; park it instead of
        // freeing stack-adjacent heap memory (see HostContext orphans).
        if let Some(lease) = slot.lease.take() {
            self.plugin.host_ctx.park_orphan_lease(lease);
        }

        if status != NrStatus::Ok {
            return Err(Self::status_error(status));
        }

        if let Some((status, data)) = slot.result.take() {
            // The pending guard removes the unused map entry on drop.
            return Ok((status, data));
        }

        match context::wait_for_unary(&self.plugin.host_ctx, sid).await {
            Some((status, payload)) => {
                pending_guard.disarm();
                // Conversion happens while the call guard is still held, so
                // a foreign payload's release cannot outlive the library.
                Ok((status, payload.into_vec()))
            }
            None => Err(NylonRingHostError::PluginUnloaded),
        }
    }

    /// Like [`PluginHandle::call_response`], but returns the response as a
    /// [`ResponseBytes`] view instead of a `Vec<u8>`.
    ///
    /// For a plugin that responds with plugin-owned bytes this is
    /// zero-copy: the buffer is only released back to the plugin when the
    /// view drops, and the view holds an in-flight call guard so the plugin
    /// cannot be unloaded from under it.
    pub async fn call_response_bytes(
        &self,
        entry: &str,
        payload: &[u8],
    ) -> Result<(NrStatus, ResponseBytes)> {
        let call_guard = self.plugin.begin_owned_call()?;
        let sid = next_sid();

        context::insert_pending(
            &self.plugin.host_ctx,
            sid,
            types::Pending::Unary(types::UnaryPending::waiting()),
        );
        let mut pending_guard = PendingGuard::new(&self.plugin.host_ctx, sid);

        let payload_bytes = NrBytes::from_slice(payload);
        let handle_raw_fn = match self.plugin.dispatch.handle {
            Some(f) => f,
            None => return Err(NylonRingHostError::MissingRequiredFunctions),
        };

        let status = unsafe { handle_raw_fn(NrStr::new(entry), sid, payload_bytes) };

        if status != NrStatus::Ok {
            return Err(Self::status_error(status));
        }

        match context::wait_for_unary(&self.plugin.host_ctx, sid).await {
            Some((status, payload)) => {
                pending_guard.disarm();
                // Host-owned payloads need no guard; foreign ones keep the
                // plugin loaded until the view is dropped.
                let guard =
                    matches!(payload, types::ResponsePayload::Foreign(_)).then_some(call_guard);
                Ok((status, ResponseBytes::new(payload, guard)))
            }
            None => Err(NylonRingHostError::PluginUnloaded),
        }
    }

    /// Like [`PluginHandle::call_response`] but bounded by a timeout.
    ///
    /// If the plugin does not deliver a response within `timeout`, the pending
    /// entry is removed from the host's tracking map and `Err(Timeout)` is
    /// returned. Use this for any production caller that cannot afford to
    /// hang indefinitely on a misbehaving plugin.
    pub async fn call_response_timeout(
        &self,
        entry: &str,
        payload: &[u8],
        timeout: std::time::Duration,
    ) -> Result<(NrStatus, Vec<u8>)> {
        let _call_guard = self.plugin.begin_call()?;
        let sid = next_sid();

        context::insert_pending(
            &self.plugin.host_ctx,
            sid,
            types::Pending::Unary(types::UnaryPending::waiting()),
        );
        let mut pending_guard = PendingGuard::new(&self.plugin.host_ctx, sid);

        // Same synchronous-response slot as `call_response`.
        let mut slot = types::UnaryResultSlot {
            sid,
            result: None,
            lease: None,
        };
        let binding = FastSlotBinding::bind(&mut slot).ok();

        let payload_bytes = NrBytes::from_slice(payload);
        let handle_raw_fn = match self.plugin.dispatch.handle {
            Some(f) => f,
            None => return Err(NylonRingHostError::MissingRequiredFunctions),
        };

        let status = unsafe { handle_raw_fn(NrStr::new(entry), sid, payload_bytes) };

        drop(binding);
        if let Some(lease) = slot.lease.take() {
            self.plugin.host_ctx.park_orphan_lease(lease);
        }

        if status != NrStatus::Ok {
            return Err(Self::status_error(status));
        }

        if let Some((status, data)) = slot.result.take() {
            return Ok((status, data));
        }

        match tokio::time::timeout(timeout, context::wait_for_unary(&self.plugin.host_ctx, sid))
            .await
        {
            Ok(Some((status, payload))) => {
                pending_guard.disarm();
                Ok((status, payload.into_vec()))
            }
            Ok(None) => Err(NylonRingHostError::PluginUnloaded),
            Err(_) => Err(NylonRingHostError::Timeout),
        }
    }

    /// Ultra-fast unary call for synchronous plugins.
    pub async fn call_response_fast(
        &self,
        entry: &str,
        payload: &[u8],
    ) -> Result<(NrStatus, Vec<u8>)> {
        let _call_guard = self.plugin.begin_call()?;
        let sid = next_sid();
        let mut slot = types::UnaryResultSlot {
            sid,
            result: None,
            lease: None,
        };

        let binding = FastSlotBinding::bind(&mut slot)?;

        let payload_bytes = NrBytes::from_slice(payload);

        let handle_raw_fn = match self.plugin.dispatch.handle {
            Some(f) => f,
            None => return Err(NylonRingHostError::MissingRequiredFunctions),
        };

        let status = unsafe { handle_raw_fn(NrStr::new(entry), sid, payload_bytes) };

        drop(binding);
        // A lease the plugin acquired but never committed may still be
        // written to by misbehaving plugin threads; park it instead of
        // freeing stack-adjacent heap memory (see HostContext orphans).
        if let Some(lease) = slot.lease.take() {
            self.plugin.host_ctx.park_orphan_lease(lease);
        }
        self.plugin.host_ctx.remove_state(sid);

        if status != NrStatus::Ok {
            return Err(Self::status_error(status));
        }

        match slot.result {
            Some((st, data)) => Ok((st, data)),
            None => Err(NylonRingHostError::MissingSynchronousResponse),
        }
    }

    /// Fire-and-forget call to a plugin entry point.
    pub async fn call(&self, entry: &str, payload: &[u8]) -> Result<NrStatus> {
        let _call_guard = self.plugin.begin_call()?;
        // Use Fast SID
        let sid = next_sid();

        let payload_bytes = NrBytes::from_slice(payload);
        let handle_raw_fn = match self.plugin.dispatch.handle {
            Some(f) => f,
            None => {
                return Err(NylonRingHostError::MissingRequiredFunctions);
            }
        };

        let status = unsafe { handle_raw_fn(NrStr::new(entry), sid, payload_bytes) };

        // Fire-and-forget calls have no later response lifecycle in which to
        // clean up extension state.
        self.plugin.host_ctx.remove_state(sid);

        if status != NrStatus::Ok {
            return Err(Self::status_error(status));
        }
        Ok(status)
    }

    /// Call a plugin entry point with a streaming response pattern.
    pub async fn call_stream(&self, entry: &str, payload: &[u8]) -> Result<(u64, StreamReceiver)> {
        let call_guard = self.plugin.begin_owned_call()?;
        let sid = next_sid();

        let (tx, rx) = stream_channel::acquire(self.plugin.host_ctx.stream_capacity());

        // Frames the plugin sends synchronously inside `handle` land in the
        // channel through this slot without a per-frame map lookup.
        let mut frame_slot = context::StreamFrameSlot {
            sid,
            chan: tx.channel(),
            terminal_seen: false,
        };

        // Register the stream channel (Map)
        context::insert_pending(&self.plugin.host_ctx, sid, types::Pending::Stream(tx));

        let payload_bytes = NrBytes::from_slice(payload);

        let handle_raw_fn = match self.plugin.dispatch.handle {
            Some(f) => f,
            None => {
                context::cleanup_sid(&self.plugin.host_ctx, sid);
                return Err(NylonRingHostError::MissingRequiredFunctions);
            }
        };

        let binding = StreamSlotBinding::bind(&mut frame_slot);
        let status = unsafe { handle_raw_fn(NrStr::new(entry), sid, payload_bytes) };
        drop(binding);

        if frame_slot.terminal_seen {
            // The stream completed inside `handle`; drop the map entry (and
            // with it the sender) so the receiver observes end-of-stream.
            context::cleanup_sid(&self.plugin.host_ctx, sid);
        }

        if status != NrStatus::Ok {
            context::cleanup_sid(&self.plugin.host_ctx, sid);
            return Err(Self::status_error(status));
        }

        Ok((sid, StreamReceiver::new(rx, None, sid, Some(call_guard))))
    }

    /// Send data to an active stream.
    pub fn send_stream_data(&self, sid: u64, data: &[u8]) -> Result<NrStatus> {
        let stream_data_fn = match self.plugin.dispatch.stream_data {
            Some(f) => f,
            None => return Err(NylonRingHostError::MissingRequiredFunctions),
        };
        let payload = NrBytes::from_slice(data);
        Ok(unsafe { stream_data_fn(sid, payload) })
    }

    /// Close an active stream from the host side.
    pub fn close_stream(&self, sid: u64) -> Result<NrStatus> {
        let stream_close_fn = match self.plugin.dispatch.stream_close {
            Some(f) => f,
            None => return Err(NylonRingHostError::MissingRequiredFunctions),
        };
        Ok(unsafe { stream_close_fn(sid) })
    }

    /// Resolve an entry name to a [`PluginEntry`] that dispatches by
    /// integer id, skipping the per-call name comparison.
    ///
    /// The id stays valid for this loaded plugin instance; a handle created
    /// before a reload keeps calling the instance it resolved against, the
    /// same snapshot semantics as `PluginHandle` itself. Fails with
    /// [`NylonRingHostError::EntryDispatchUnsupported`] for v1 plugins and
    /// [`NylonRingHostError::EntryNotFound`] for names the plugin does not
    /// export.
    pub fn entry(&self, entry: &str) -> Result<PluginEntry> {
        let (Some(resolve_fn), Some(_)) = (
            self.plugin.dispatch.resolve_entry,
            self.plugin.dispatch.handle_by_id,
        ) else {
            return Err(NylonRingHostError::EntryDispatchUnsupported);
        };
        // Resolution runs plugin code, so it counts as an in-flight call.
        let _call_guard = self.plugin.begin_call()?;
        let id = unsafe { resolve_fn(NrStr::new(entry)) };
        if id == nylon_ring::NR_ENTRY_UNKNOWN {
            return Err(NylonRingHostError::EntryNotFound(entry.to_string()));
        }
        Ok(PluginEntry {
            plugin: self.plugin.clone(),
            id,
        })
    }
}

/// A pre-resolved plugin entry point that dispatches by integer id
///: the per-call entry-name comparison and `NrStr` construction
/// are paid once, in [`PluginHandle::entry`], instead of on every call.
#[derive(Clone)]
pub struct PluginEntry {
    plugin: Arc<LoadedPlugin>,
    id: u32,
}

impl PluginEntry {
    fn handle_by_id_fn(&self) -> Result<unsafe extern "C" fn(u32, u64, NrBytes) -> NrStatus> {
        self.plugin
            .dispatch
            .handle_by_id
            .ok_or(NylonRingHostError::EntryDispatchUnsupported)
    }

    /// Fire-and-forget call; see [`PluginHandle::call`].
    pub async fn call(&self, payload: &[u8]) -> Result<NrStatus> {
        let _call_guard = self.plugin.begin_call()?;
        let sid = next_sid();

        let handle_by_id_fn = self.handle_by_id_fn()?;
        let status = unsafe { handle_by_id_fn(self.id, sid, NrBytes::from_slice(payload)) };

        self.plugin.host_ctx.remove_state(sid);

        if status != NrStatus::Ok {
            return Err(PluginHandle::status_error(status));
        }
        Ok(status)
    }

    /// Request-response call; see [`PluginHandle::call_response`].
    pub async fn call_response(&self, payload: &[u8]) -> Result<(NrStatus, Vec<u8>)> {
        let _call_guard = self.plugin.begin_call()?;
        let sid = next_sid();

        context::insert_pending(
            &self.plugin.host_ctx,
            sid,
            types::Pending::Unary(types::UnaryPending::waiting()),
        );
        let mut pending_guard = PendingGuard::new(&self.plugin.host_ctx, sid);

        // Same synchronous-response slot as `PluginHandle::call_response`.
        let mut slot = types::UnaryResultSlot {
            sid,
            result: None,
            lease: None,
        };
        let binding = FastSlotBinding::bind(&mut slot).ok();

        let handle_by_id_fn = self.handle_by_id_fn()?;
        let status = unsafe { handle_by_id_fn(self.id, sid, NrBytes::from_slice(payload)) };

        drop(binding);
        if let Some(lease) = slot.lease.take() {
            self.plugin.host_ctx.park_orphan_lease(lease);
        }

        if status != NrStatus::Ok {
            return Err(PluginHandle::status_error(status));
        }

        if let Some((status, data)) = slot.result.take() {
            return Ok((status, data));
        }

        match context::wait_for_unary(&self.plugin.host_ctx, sid).await {
            Some((status, payload)) => {
                pending_guard.disarm();
                Ok((status, payload.into_vec()))
            }
            None => Err(NylonRingHostError::PluginUnloaded),
        }
    }

    /// Synchronous fast-path call; see [`PluginHandle::call_response_fast`].
    pub async fn call_response_fast(&self, payload: &[u8]) -> Result<(NrStatus, Vec<u8>)> {
        let _call_guard = self.plugin.begin_call()?;
        let sid = next_sid();
        let mut slot = types::UnaryResultSlot {
            sid,
            result: None,
            lease: None,
        };

        let binding = FastSlotBinding::bind(&mut slot)?;

        let handle_by_id_fn = self.handle_by_id_fn()?;
        let status = unsafe { handle_by_id_fn(self.id, sid, NrBytes::from_slice(payload)) };

        drop(binding);
        if let Some(lease) = slot.lease.take() {
            self.plugin.host_ctx.park_orphan_lease(lease);
        }
        self.plugin.host_ctx.remove_state(sid);

        if status != NrStatus::Ok {
            return Err(PluginHandle::status_error(status));
        }

        match slot.result {
            Some((st, data)) => Ok((st, data)),
            None => Err(NylonRingHostError::MissingSynchronousResponse),
        }
    }

    /// Streaming call; see [`PluginHandle::call_stream`].
    pub async fn call_stream(&self, payload: &[u8]) -> Result<(u64, StreamReceiver)> {
        let call_guard = self.plugin.begin_owned_call()?;
        let sid = next_sid();

        let (tx, rx) = stream_channel::acquire(self.plugin.host_ctx.stream_capacity());

        // Same synchronous-frame slot as `PluginHandle::call_stream`.
        let mut frame_slot = context::StreamFrameSlot {
            sid,
            chan: tx.channel(),
            terminal_seen: false,
        };

        context::insert_pending(&self.plugin.host_ctx, sid, types::Pending::Stream(tx));

        let handle_by_id_fn = match self.handle_by_id_fn() {
            Ok(f) => f,
            Err(error) => {
                context::cleanup_sid(&self.plugin.host_ctx, sid);
                return Err(error);
            }
        };
        let binding = StreamSlotBinding::bind(&mut frame_slot);
        let status = unsafe { handle_by_id_fn(self.id, sid, NrBytes::from_slice(payload)) };
        drop(binding);

        if frame_slot.terminal_seen {
            context::cleanup_sid(&self.plugin.host_ctx, sid);
        }

        if status != NrStatus::Ok {
            context::cleanup_sid(&self.plugin.host_ctx, sid);
            return Err(PluginHandle::status_error(status));
        }

        Ok((sid, StreamReceiver::new(rx, None, sid, Some(call_guard))))
    }
}

/// The main host for loading and managing nylon-ring plugins.
pub struct NylonRingHost {
    plugins: HashMap<String, Arc<LoadedPlugin>>,
    host_ctx: Arc<HostContext>,
    host_vtable: Box<NrHostVTable>,
}

impl Default for NylonRingHost {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for NylonRingHost {
    fn drop(&mut self) {
        // Call guards hold raw plugin pointers, so the map's Arcs must be
        // retired — never dropped directly — even when the host itself goes
        // away while streams are still in flight.
        for (_, plugin) in self.plugins.drain() {
            retire_plugin(plugin);
        }
    }
}

impl NylonRingHost {
    /// Create a new empty host.
    pub fn new() -> Self {
        Self::with_stream_capacity(DEFAULT_STREAM_CAPACITY)
    }

    /// Create a host with a bounded per-stream frame capacity.
    pub fn with_stream_capacity(stream_capacity: usize) -> Self {
        assert!(
            stream_capacity > 0,
            "stream capacity must be greater than zero"
        );
        let host_ctx = Arc::new(HostContext::new(
            NrHostExt {
                set_state: set_state_callback,
                get_state: get_state_callback,
            },
            stream_capacity,
        ));

        let host_vtable = Box::new(NrHostVTable {
            send_result: send_result_vec_callback,
            send_result_owned: send_result_owned_callback,
            acquire_result_buffer: acquire_result_buffer_callback,
            commit_result_buffer: commit_result_buffer_callback,
        });

        Self {
            plugins: HashMap::new(),
            host_ctx,
            host_vtable,
        }
    }

    /// Load a plugin from the specified path with a given name.
    ///
    /// The plugin must export `nylon_ring_get_plugin` and match this host's
    /// `ABI_VERSION` exactly.
    pub fn load(&mut self, name: &str, path: &str) -> Result<()> {
        self.load_inner(name, path, false)
    }

    /// Load a plugin that opts out of unload/reload for the life of the
    /// process.
    ///
    /// Per-call guards on a pinned plugin skip the in-flight tracker RMWs
    /// (the dominant per-call cost on short paths); in exchange the plugin
    /// can never be unloaded, reloaded, or replaced, its `shutdown` callback
    /// never runs, and its library handle is intentionally leaked at host
    /// drop. Streams still take tracker slots.
    pub fn load_pinned(&mut self, name: &str, path: &str) -> Result<()> {
        self.load_inner(name, path, true)
    }

    fn load_inner(&mut self, name: &str, path: &str, pinned: bool) -> Result<()> {
        if let Some(existing) = self.plugins.get(name)
            && existing.pinned
        {
            return Err(NylonRingHostError::PluginPinned(name.to_owned()));
        }
        unsafe {
            let lib = Library::new(path).map_err(NylonRingHostError::FailedToLoadLibrary)?;

            let get_plugin: Symbol<extern "C" fn() -> *const NrPluginInfo> =
                lib.get(b"nylon_ring_get_plugin\0").map_err(|_| {
                    NylonRingHostError::MissingSymbol("nylon_ring_get_plugin".to_string())
                })?;
            let info_ptr = get_plugin();
            Self::validate_plugin_info::<NrPluginInfo>(info_ptr.cast(), ABI_VERSION)?;
            let info = &*info_ptr;
            if info.vtable.is_null() {
                return Err(NylonRingHostError::NullPluginVTable);
            }
            let plugin_vtable = &*info.vtable;
            if plugin_vtable.init.is_none() || plugin_vtable.handle.is_none() {
                return Err(NylonRingHostError::MissingRequiredFunctions);
            }
            if let Some(init_fn) = plugin_vtable.init {
                let status = init_fn(
                    Arc::as_ptr(&self.host_ctx) as *mut c_void,
                    &*self.host_vtable,
                );
                if status != NrStatus::Ok {
                    return Err(NylonRingHostError::PluginInitFailed(status));
                }
            }
            let dispatch = PluginDispatch::from_vtable(plugin_vtable);

            let loaded = LoadedPlugin {
                _lib: std::mem::ManuallyDrop::new(lib),
                dispatch,
                host_ctx: self.host_ctx.clone(),
                path: path.to_string(),
                call_tracker: CallTracker::new(),
                pinned,
            };

            if let Some(previous) = self.plugins.insert(name.to_string(), Arc::new(loaded)) {
                retire_plugin(previous);
            }
            Ok(())
        }
    }

    /// Validates the version-independent `abi_version`/`struct_size` header
    /// shared by every plugin-info generation.
    ///
    /// # Safety
    ///
    /// `info_ptr` must be null or point to a struct starting with two `u32`
    /// fields (`abi_version`, `struct_size`).
    unsafe fn validate_plugin_info<T>(info_ptr: *const u32, expected_version: u32) -> Result<()> {
        if info_ptr.is_null() {
            return Err(NylonRingHostError::NullPluginInfo);
        }
        let abi_version = unsafe { std::ptr::read_unaligned(info_ptr) };
        let struct_size = unsafe { std::ptr::read_unaligned(info_ptr.add(1)) };
        if abi_version != expected_version {
            return Err(NylonRingHostError::IncompatibleAbiVersion {
                expected: expected_version,
                actual: abi_version,
            });
        }
        let expected_size = std::mem::size_of::<T>() as u32;
        if struct_size < expected_size {
            return Err(NylonRingHostError::IncompatiblePluginInfoSize {
                expected: expected_size,
                actual: struct_size,
            });
        }
        Ok(())
    }

    /// Unload a plugin by name.
    pub fn unload(&mut self, name: &str) -> Result<()> {
        self.refuse_pinned(name)?;
        let plugin = self
            .plugins
            .remove(name)
            .ok_or_else(|| NylonRingHostError::PluginNotFound(name.to_owned()))?;
        retire_plugin(plugin);
        Ok(())
    }

    /// Pinned plugins hold untracked in-flight calls, so retiring them is
    /// never provably safe; every unload/reload entry point refuses first.
    fn refuse_pinned(&self, name: &str) -> Result<()> {
        if let Some(plugin) = self.plugins.get(name)
            && plugin.pinned
        {
            return Err(NylonRingHostError::PluginPinned(name.to_owned()));
        }
        Ok(())
    }

    /// Stop accepting new calls, then wait for tracked calls to drain.
    pub async fn unload_with_grace(&mut self, name: &str, grace: Duration) -> Result<()> {
        self.refuse_pinned(name)?;
        let plugin = self
            .plugins
            .remove(name)
            .ok_or_else(|| NylonRingHostError::PluginNotFound(name.to_owned()))?;
        // Retire before draining so the plugin stays registered for the
        // final sweep even if the drain times out.
        retire_plugin(plugin.clone());
        Self::wait_for_drain(&plugin, grace).await
    }

    /// Reload all plugins.
    pub fn reload(&mut self) -> Result<()> {
        if let Some(name) = self
            .plugins
            .iter()
            .find_map(|(name, plugin)| plugin.pinned.then(|| name.clone()))
        {
            return Err(NylonRingHostError::PluginPinned(name));
        }
        let active_calls: usize = self
            .plugins
            .values()
            .map(|plugin| plugin.call_tracker.active_calls())
            .sum();
        if active_calls != 0 {
            return Err(NylonRingHostError::PluginBusy { active_calls });
        }
        let mut plugins_to_reload = Vec::new();
        for (name, plugin) in &self.plugins {
            plugins_to_reload.push((name.clone(), plugin.path.clone()));
        }

        // Load new versions - insert() will atomically replace old ones
        // This ensures zero downtime (plugin() always returns a value)
        for (name, path) in plugins_to_reload {
            self.load(&name, &path)?;
        }

        Ok(())
    }

    /// Reload all plugins and wait for calls on replaced instances to drain.
    pub async fn reload_with_grace(&mut self, grace: Duration) -> Result<()> {
        if let Some(name) = self
            .plugins
            .iter()
            .find_map(|(name, plugin)| plugin.pinned.then(|| name.clone()))
        {
            return Err(NylonRingHostError::PluginPinned(name));
        }
        let old_plugins: Vec<_> = self
            .plugins
            .drain()
            .map(|(name, plugin)| {
                retire_plugin(plugin.clone());
                (name, plugin)
            })
            .collect();
        for (_, plugin) in &old_plugins {
            Self::wait_for_drain(plugin, grace).await?;
        }
        let plugins_to_reload: Vec<_> = old_plugins
            .iter()
            .map(|(name, plugin)| (name.clone(), plugin.path.clone()))
            .collect();
        drop(old_plugins);
        for (name, path) in plugins_to_reload {
            self.load(&name, &path)?;
        }
        Ok(())
    }

    async fn wait_for_drain(plugin: &LoadedPlugin, grace: Duration) -> Result<()> {
        let deadline = tokio::time::Instant::now() + grace;
        while plugin.call_tracker.active_calls() != 0 {
            if tokio::time::Instant::now() >= deadline {
                return Err(NylonRingHostError::DrainTimeout {
                    remaining: plugin.call_tracker.active_calls(),
                });
            }
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
        Ok(())
    }

    /// Get a handle to a loaded plugin by name.
    pub fn plugin(&self, name: &str) -> Option<PluginHandle> {
        self.plugins
            .get(name)
            .map(|p| PluginHandle { plugin: p.clone() })
    }

    /// Capture lightweight host metrics without allocating the shard map.
    pub fn metrics(&self) -> HostMetrics {
        HostMetrics {
            loaded_plugins: self.plugins.len(),
            pending_requests: self.host_ctx.pending_count(),
            state_sessions: self.host_ctx.state_count(),
            in_flight_calls: self
                .plugins
                .values()
                .map(|plugin| plugin.call_tracker.active_calls())
                .sum(),
        }
    }

    /// Get host extension pointer from host_ctx.
    ///
    /// # Safety
    ///
    /// The caller must ensure that `host_ctx` is a valid pointer to a `HostContext`
    /// instance that was created by this host, or a null pointer.
    pub unsafe fn get_host_ext(host_ctx: *mut c_void) -> *const NrHostExt {
        if host_ctx.is_null() {
            return std::ptr::null();
        }
        let ctx = unsafe { &*host_ctx.cast::<HostContext>() };
        &ctx.host_ext as *const NrHostExt
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct WakeFlag(Arc<std::sync::atomic::AtomicBool>);

    impl std::task::Wake for WakeFlag {
        fn wake(self: Arc<Self>) {
            self.0.store(true, std::sync::atomic::Ordering::Release);
        }

        fn wake_by_ref(self: &Arc<Self>) {
            self.0.store(true, std::sync::atomic::Ordering::Release);
        }
    }

    struct ReentrantWakerState {
        host_ctx: Arc<HostContext>,
        clones: Arc<std::sync::atomic::AtomicUsize>,
        drops: Arc<std::sync::atomic::AtomicUsize>,
    }

    unsafe fn clone_reentrant_waker(data: *const ()) -> std::task::RawWaker {
        let state = unsafe { Arc::<ReentrantWakerState>::from_raw(data.cast()) };
        state.host_ctx.pending_count();
        state
            .clones
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let clone = state.clone();
        let _ = Arc::into_raw(state);
        std::task::RawWaker::new(Arc::into_raw(clone).cast(), &REENTRANT_WAKER_VTABLE)
    }

    unsafe fn wake_reentrant_waker(data: *const ()) {
        let state = unsafe { Arc::<ReentrantWakerState>::from_raw(data.cast()) };
        state.host_ctx.pending_count();
    }

    unsafe fn wake_reentrant_waker_by_ref(data: *const ()) {
        let state = unsafe { Arc::<ReentrantWakerState>::from_raw(data.cast()) };
        state.host_ctx.pending_count();
        let _ = Arc::into_raw(state);
    }

    unsafe fn drop_reentrant_waker(data: *const ()) {
        let state = unsafe { Arc::<ReentrantWakerState>::from_raw(data.cast()) };
        state.host_ctx.pending_count();
        state
            .drops
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }

    static REENTRANT_WAKER_VTABLE: std::task::RawWakerVTable = std::task::RawWakerVTable::new(
        clone_reentrant_waker,
        wake_reentrant_waker,
        wake_reentrant_waker_by_ref,
        drop_reentrant_waker,
    );

    fn reentrant_waker(
        host_ctx: Arc<HostContext>,
        clones: Arc<std::sync::atomic::AtomicUsize>,
        drops: Arc<std::sync::atomic::AtomicUsize>,
    ) -> std::task::Waker {
        let state = Arc::new(ReentrantWakerState {
            host_ctx,
            clones,
            drops,
        });
        let raw = std::task::RawWaker::new(Arc::into_raw(state).cast(), &REENTRANT_WAKER_VTABLE);
        unsafe { std::task::Waker::from_raw(raw) }
    }

    /// Tests that load a real plugin library share one dlopen'd image (and
    /// therefore its process-global init state) — a concurrent test dropping
    /// its `LoadedPlugin` runs `shutdown` under everyone else's feet. Any
    /// test that loads a plugin must hold this lock. This was also the root
    /// cause of the historical `unload_defers_...` flake.
    fn plugin_test_lock() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
        LOCK.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn example_plugin_path() -> Option<std::path::PathBuf> {
        let workspace_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()?
            .parent()?;
        let manifest = workspace_root.join("examples/ex-nyring-plugin/Cargo.toml");
        if !manifest.is_file() {
            return None;
        }
        let status = std::process::Command::new("cargo")
            .args(["build", "--release", "--manifest-path"])
            .arg(&manifest)
            .status()
            .ok()?;
        if !status.success() {
            return None;
        }
        let filename = if cfg!(target_os = "macos") {
            "libex_nyring_plugin.dylib"
        } else if cfg!(target_os = "windows") {
            "ex_nyring_plugin.dll"
        } else {
            "libex_nyring_plugin.so"
        };
        Some(workspace_root.join("target/release").join(filename))
    }

    fn c_plugin_path() -> Option<std::path::PathBuf> {
        let workspace_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()?
            .parent()?;
        let source = workspace_root.join("examples/c-plugin/plugin.c");
        if !source.is_file() {
            return None;
        }
        let extension = if cfg!(target_os = "macos") {
            "dylib"
        } else if cfg!(target_os = "windows") {
            "dll"
        } else {
            "so"
        };
        let output = workspace_root
            .join("target/c-example")
            .join(format!("libnylon_ring_c_example.{extension}"));
        std::fs::create_dir_all(output.parent()?).ok()?;
        let status = std::process::Command::new("cc")
            .args(["-std=c11", "-Wall", "-Wextra", "-Werror", "-shared"])
            .args((!cfg!(target_os = "windows")).then_some("-fPIC"))
            .arg(&source)
            .arg("-o")
            .arg(&output)
            .status()
            .ok()?;
        status.success().then_some(output)
    }

    #[test]
    fn dropping_pending_guard_unregisters_unary_request() {
        let host = NylonRingHost::new();
        let sid = 42;
        context::insert_pending(
            &host.host_ctx,
            sid,
            types::Pending::Unary(types::UnaryPending::waiting()),
        );
        host.host_ctx.set_state(sid, "key".into(), vec![1]);

        drop(PendingGuard::new(&host.host_ctx, sid));

        assert!(context::remove_pending(&host.host_ctx, sid).is_none());
        assert!(!host.host_ctx.state_per_sid.contains_key(&sid));
    }

    #[test]
    fn unary_completion_wakes_latest_waiter_across_threads() {
        let host = NylonRingHost::new();
        let sid = 46;
        context::insert_pending(
            &host.host_ctx,
            sid,
            types::Pending::Unary(types::UnaryPending::waiting()),
        );
        host.host_ctx.set_state(sid, "key".into(), vec![1]);

        let first_woken = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let second_woken = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let first_waker = std::task::Waker::from(Arc::new(WakeFlag(first_woken.clone())));
        let second_waker = std::task::Waker::from(Arc::new(WakeFlag(second_woken.clone())));
        let mut future = Box::pin(context::wait_for_unary(&host.host_ctx, sid));

        let mut first_context = std::task::Context::from_waker(&first_waker);
        assert!(matches!(
            std::future::Future::poll(future.as_mut(), &mut first_context),
            std::task::Poll::Pending
        ));
        let mut second_context = std::task::Context::from_waker(&second_waker);
        assert!(matches!(
            std::future::Future::poll(future.as_mut(), &mut second_context),
            std::task::Poll::Pending
        ));

        let host_ctx = host.host_ctx.clone();
        let dispatch_status = std::thread::spawn(move || {
            context::dispatch_pending(
                &host_ctx,
                sid,
                NrStatus::Ok,
                types::ResponsePayload::Owned(vec![7]),
            )
        })
        .join()
        .unwrap();
        assert_eq!(dispatch_status, NrStatus::Ok);
        assert!(!first_woken.load(std::sync::atomic::Ordering::Acquire));
        assert!(second_woken.load(std::sync::atomic::Ordering::Acquire));
        assert!(!host.host_ctx.state_per_sid.contains_key(&sid));
        assert_eq!(host.metrics().pending_requests, 1);

        assert_eq!(
            context::dispatch_pending(
                &host.host_ctx,
                sid,
                NrStatus::Ok,
                types::ResponsePayload::Owned(vec![8]),
            ),
            NrStatus::Invalid
        );

        match std::future::Future::poll(future.as_mut(), &mut second_context) {
            std::task::Poll::Ready(Some((status, data))) => {
                assert_eq!(status, NrStatus::Ok);
                assert_eq!(data.as_slice(), [7]);
            }
            result => panic!("unexpected unary completion result: {result:?}"),
        }
        assert_eq!(host.metrics().pending_requests, 0);
    }

    #[test]
    fn unary_waker_callbacks_run_outside_pending_lock() {
        let host = NylonRingHost::new();
        let sid = 47;
        context::insert_pending(
            &host.host_ctx,
            sid,
            types::Pending::Unary(types::UnaryPending::waiting()),
        );

        let clones = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let drops = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let first_waker = reentrant_waker(host.host_ctx.clone(), clones.clone(), drops.clone());
        let mut future = Box::pin(context::wait_for_unary(&host.host_ctx, sid));
        {
            let mut context = std::task::Context::from_waker(&first_waker);
            assert!(matches!(
                std::future::Future::poll(future.as_mut(), &mut context),
                std::task::Poll::Pending
            ));
        }
        assert_eq!(clones.load(std::sync::atomic::Ordering::Relaxed), 1);

        let second_woken = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let second_waker = std::task::Waker::from(Arc::new(WakeFlag(second_woken)));
        let mut context = std::task::Context::from_waker(&second_waker);
        assert!(matches!(
            std::future::Future::poll(future.as_mut(), &mut context),
            std::task::Poll::Pending
        ));
        assert_eq!(drops.load(std::sync::atomic::Ordering::Relaxed), 1);

        drop(future);
        context::cleanup_sid(&host.host_ctx, sid);
    }

    #[test]
    fn dropping_stream_receiver_unregisters_stream() {
        let host = NylonRingHost::new();
        let sid = 43;
        let (tx, rx) = stream_channel::acquire(1);
        context::insert_pending(&host.host_ctx, sid, types::Pending::Stream(tx));
        host.host_ctx.set_state(sid, "key".into(), vec![1]);

        drop(StreamReceiver::new(
            rx,
            Some(host.host_ctx.clone()),
            sid,
            None,
        ));

        assert!(context::remove_pending(&host.host_ctx, sid).is_none());
        assert!(!host.host_ctx.state_per_sid.contains_key(&sid));
    }

    #[test]
    fn fast_slot_reentry_is_rejected_and_binding_is_cleared() {
        let mut first = types::UnaryResultSlot {
            sid: 1,
            result: None,
            lease: None,
        };
        let mut second = types::UnaryResultSlot {
            sid: 2,
            result: None,
            lease: None,
        };
        let binding = FastSlotBinding::bind(&mut first).unwrap();
        assert!(matches!(
            FastSlotBinding::bind(&mut second),
            Err(NylonRingHostError::FastPathReentrant)
        ));
        drop(binding);
        assert!(FastSlotBinding::bind(&mut second).is_ok());
    }

    #[test]
    fn bounded_stream_reports_backpressure_and_removes_terminal_once() {
        let host = NylonRingHost::with_stream_capacity(1);
        let sid = 44;
        let (tx, mut rx) = stream_channel::acquire(1);
        context::insert_pending(&host.host_ctx, sid, types::Pending::Stream(tx));

        assert_eq!(
            context::dispatch_pending(
                &host.host_ctx,
                sid,
                NrStatus::Ok,
                types::ResponsePayload::Owned(vec![1]),
            ),
            NrStatus::Ok
        );
        assert_eq!(
            context::dispatch_pending(
                &host.host_ctx,
                sid,
                NrStatus::Ok,
                types::ResponsePayload::Owned(vec![2]),
            ),
            NrStatus::Backpressure
        );
        assert_eq!(rx.try_recv().unwrap().data, vec![1]);
        assert_eq!(
            context::dispatch_pending(
                &host.host_ctx,
                sid,
                NrStatus::StreamEnd,
                types::ResponsePayload::Owned(vec![]),
            ),
            NrStatus::Ok
        );
        assert_eq!(host.metrics().pending_requests, 0);
        assert_eq!(rx.try_recv().unwrap().status, NrStatus::StreamEnd);
    }

    #[test]
    fn concurrent_terminal_frames_complete_stream_once() {
        let host = NylonRingHost::with_stream_capacity(2);
        let sid = 45;
        let (tx, mut rx) = stream_channel::acquire(2);
        context::insert_pending(&host.host_ctx, sid, types::Pending::Stream(tx));
        let barrier = Arc::new(std::sync::Barrier::new(3));

        let results: Vec<_> = std::thread::scope(|scope| {
            let handles: Vec<_> = (0..2)
                .map(|value| {
                    let ctx = host.host_ctx.clone();
                    let barrier = barrier.clone();
                    scope.spawn(move || {
                        barrier.wait();
                        context::dispatch_pending(
                            &ctx,
                            sid,
                            NrStatus::StreamEnd,
                            types::ResponsePayload::Owned(vec![value]),
                        )
                    })
                })
                .collect();
            barrier.wait();
            handles
                .into_iter()
                .map(|handle| handle.join().unwrap())
                .collect()
        });

        assert_eq!(
            results
                .iter()
                .filter(|&&status| status == NrStatus::Ok)
                .count(),
            1
        );
        assert_eq!(
            results
                .iter()
                .filter(|&&status| status == NrStatus::Invalid)
                .count(),
            1
        );
        assert!(rx.try_recv().is_some());
        assert!(rx.try_recv().is_none());
        assert_eq!(host.metrics().pending_requests, 0);
    }

    #[test]
    fn unload_defers_library_drop_and_rejects_new_calls_on_existing_handle() {
        let _plugin_lock = plugin_test_lock();
        let Some(path) = example_plugin_path() else {
            return;
        };
        let mut host = NylonRingHost::new();
        host.load("example", path.to_str().unwrap()).unwrap();
        let handle = host.plugin("example").unwrap();
        let runtime = tokio::runtime::Runtime::new().unwrap();
        assert!(matches!(
            runtime.block_on(handle.call_response_timeout(
                "benchmark_without_response",
                b"",
                Duration::from_millis(1),
            )),
            Err(NylonRingHostError::Timeout)
        ));
        assert_eq!(host.metrics().pending_requests, 0);

        let guard = handle.plugin.begin_call().unwrap();
        assert_eq!(host.metrics().in_flight_calls, 1);

        host.unload("example").unwrap();
        assert!(matches!(
            runtime.block_on(handle.call("benchmark_without_response", b"")),
            Err(NylonRingHostError::PluginUnloaded)
        ));
        drop(guard);
        drop(handle);
    }

    #[test]
    fn stream_receiver_outlives_unload_and_drains_frames() {
        let _plugin_lock = plugin_test_lock();
        let Some(path) = example_plugin_path() else {
            return;
        };
        let mut host = NylonRingHost::new();
        host.load("example", path.to_str().unwrap()).unwrap();
        let handle = host.plugin("example").unwrap();
        let runtime = tokio::runtime::Runtime::new().unwrap();

        let (_sid, mut receiver) = runtime
            .block_on(handle.call_stream("benchmark_stream", b""))
            .unwrap();
        host.unload("example").unwrap();
        drop(handle);

        // The receiver's call guard must keep the retired plugin alive:
        // draining all buffered frames exercises the guarded context path.
        let frames = runtime.block_on(async {
            let mut frames = 0u32;
            while receiver.recv().await.is_some() {
                frames += 1;
            }
            frames
        });
        assert_eq!(frames, 9);
        drop(receiver);
    }

    #[test]
    fn stream_receiver_outlives_host_drop() {
        let _plugin_lock = plugin_test_lock();
        let Some(path) = example_plugin_path() else {
            return;
        };
        let mut host = NylonRingHost::new();
        host.load("example", path.to_str().unwrap()).unwrap();
        let handle = host.plugin("example").unwrap();
        let runtime = tokio::runtime::Runtime::new().unwrap();

        let (_sid, mut receiver) = runtime
            .block_on(handle.call_stream("benchmark_stream", b""))
            .unwrap();
        drop(host);
        drop(handle);

        let frames = runtime.block_on(async {
            let mut frames = 0u32;
            while receiver.recv().await.is_some() {
                frames += 1;
            }
            frames
        });
        assert_eq!(frames, 9);
    }

    #[test]
    fn owned_response_round_trips_and_releases_exactly_once() {
        let _plugin_lock = plugin_test_lock();
        let Some(path) = example_plugin_path() else {
            return;
        };
        let mut host = NylonRingHost::new();
        host.load("example", path.to_str().unwrap()).unwrap();
        let handle = host.plugin("example").unwrap();
        let runtime = tokio::runtime::Runtime::new().unwrap();

        async fn release_count(handle: &PluginHandle) -> u64 {
            let (status, data) = handle
                .call_response("owned_release_count", b"")
                .await
                .unwrap();
            assert_eq!(status, NrStatus::Ok);
            u64::from_le_bytes(data.try_into().unwrap())
        }

        runtime.block_on(async {
            // Zero-copy static response: correct bytes, nothing to release.
            let (status, data) = handle
                .call_response_bytes("benchmark_owned", &[0u8; 128])
                .await
                .unwrap();
            assert_eq!(status, NrStatus::Ok);
            assert_eq!(data.len(), 128);
            assert!(data.iter().all(|&byte| byte == 42));

            let before = release_count(&handle).await;

            // Held view: released only when dropped, not on receipt.
            let (status, held) = handle
                .call_response_bytes("echo_owned", b"held")
                .await
                .unwrap();
            assert_eq!(status, NrStatus::Ok);
            assert_eq!(&*held, b"held");
            assert_eq!(release_count(&handle).await, before);
            drop(held);
            assert_eq!(release_count(&handle).await, before + 1);

            // Vec conversion path copies and releases immediately.
            let (status, data) = handle.call_response("echo_owned", b"copied").await.unwrap();
            assert_eq!(status, NrStatus::Ok);
            assert_eq!(data, b"copied");
            assert_eq!(release_count(&handle).await, before + 2);

            // into_vec on the view also releases exactly once.
            let (_, view) = handle
                .call_response_bytes("echo_owned", b"vec")
                .await
                .unwrap();
            assert_eq!(view.into_vec(), b"vec");
            assert_eq!(release_count(&handle).await, before + 3);

            // Fast path consumes owned responses through the slot copy.
            let (status, data) = handle
                .call_response_fast("echo_owned", b"fast")
                .await
                .unwrap();
            assert_eq!(status, NrStatus::Ok);
            assert_eq!(data, b"fast");
            assert_eq!(release_count(&handle).await, before + 4);
        });
    }

    #[test]
    fn owned_response_keeps_plugin_alive_after_unload() {
        let _plugin_lock = plugin_test_lock();
        let Some(path) = example_plugin_path() else {
            return;
        };
        let mut host = NylonRingHost::new();
        host.load("example", path.to_str().unwrap()).unwrap();
        let handle = host.plugin("example").unwrap();
        let runtime = tokio::runtime::Runtime::new().unwrap();

        let (status, response) = runtime
            .block_on(handle.call_response_bytes("benchmark_owned", &[0u8; 64]))
            .unwrap();
        assert_eq!(status, NrStatus::Ok);

        // The response borrows memory inside the plugin image; its guard
        // must defer the library unload until the view drops.
        host.unload("example").unwrap();
        drop(handle);
        drop(host);
        assert_eq!(response.len(), 64);
        assert!(response.iter().all(|&byte| byte == 42));
        drop(response);
    }

    #[test]
    fn lease_round_trips_and_rejects_misuse() {
        let _plugin_lock = plugin_test_lock();
        let Some(path) = example_plugin_path() else {
            return;
        };
        let mut host = NylonRingHost::new();
        host.load("example", path.to_str().unwrap()).unwrap();
        let handle = host.plugin("example").unwrap();
        let runtime = tokio::runtime::Runtime::new().unwrap();

        runtime.block_on(async {
            // Standard unary path: the response Vec is the leased buffer.
            let (status, data) = handle
                .call_response("echo_lease", b"through the lease")
                .await
                .unwrap();
            assert_eq!(status, NrStatus::Ok);
            assert_eq!(data, b"through the lease");

            // Empty payloads commit a zero-length lease.
            let (status, data) = handle.call_response("echo_lease", b"").await.unwrap();
            assert_eq!(status, NrStatus::Ok);
            assert!(data.is_empty());

            // Fast path: the lease lives in the TLS slot.
            let (status, data) = handle
                .call_response_fast("echo_lease", b"fast lease")
                .await
                .unwrap();
            assert_eq!(status, NrStatus::Ok);
            assert_eq!(data, b"fast lease");

            // The misuse handler asserts double acquire, bad token, bad
            // length, and double commit all report Invalid; an assertion
            // failure would surface here as PluginPanicked.
            let (status, data) = handle
                .call_response("lease_misuse", b"contract")
                .await
                .unwrap();
            assert_eq!(status, NrStatus::Ok);
            assert_eq!(data, b"contract");
        });

        // Every lease was committed; nothing should be parked or pending.
        assert_eq!(host.host_ctx.orphaned_lease_count(), 0);
        assert_eq!(host.host_ctx.pending_count(), 0);
    }

    #[test]
    fn abandoned_leases_are_parked_not_freed() {
        let _plugin_lock = plugin_test_lock();
        let Some(path) = example_plugin_path() else {
            return;
        };
        let mut host = NylonRingHost::new();
        host.load("example", path.to_str().unwrap()).unwrap();
        let handle = host.plugin("example").unwrap();
        let runtime = tokio::runtime::Runtime::new().unwrap();

        runtime.block_on(async {
            // The plugin acquires a lease and never commits: the call times
            // out and cleanup must park the lease (the plugin could still
            // hold the pointer), not free it.
            let result = handle
                .call_response_timeout("lease_abandon", b"", std::time::Duration::from_millis(50))
                .await;
            assert!(matches!(result, Err(NylonRingHostError::Timeout)));
            assert_eq!(host.host_ctx.orphaned_lease_count(), 1);
            assert_eq!(host.host_ctx.pending_count(), 0);

            // Fast path: an uncommitted lease in the TLS slot is parked too.
            let result = handle.call_response_fast("lease_abandon", b"").await;
            assert!(matches!(
                result,
                Err(NylonRingHostError::MissingSynchronousResponse)
            ));
            assert_eq!(host.host_ctx.orphaned_lease_count(), 2);
        });
    }

    #[test]
    fn example_plugin_fire_and_forget_entry_returns_ok() {
        let _plugin_lock = plugin_test_lock();
        let Some(path) = example_plugin_path() else {
            return;
        };
        let mut host = NylonRingHost::new();
        host.load("example", path.to_str().unwrap()).unwrap();
        let handle = host.plugin("example").unwrap();
        let runtime = tokio::runtime::Runtime::new().unwrap();

        assert!(matches!(
            runtime.block_on(handle.call("notify", b"fire and forget")),
            Ok(NrStatus::Ok)
        ));
    }

    #[test]
    fn c_plugin_layout_round_trips_through_host() {
        let _plugin_lock = plugin_test_lock();
        let Some(path) = c_plugin_path() else {
            return;
        };
        let mut host = NylonRingHost::new();
        host.load("c-example", path.to_str().unwrap()).unwrap();
        let handle = host.plugin("c-example").unwrap();
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let (status, response) = runtime
            .block_on(handle.call_response("echo", b"from-c"))
            .unwrap();
        assert_eq!(status, NrStatus::Ok);
        assert_eq!(response, b"from-c");

        // Integer entry dispatch is optional; this plugin leaves both
        // fields NULL, which must surface as a typed error.
        assert!(matches!(
            handle.entry("echo"),
            Err(NylonRingHostError::EntryDispatchUnsupported)
        ));
    }

    #[test]
    fn entry_id_dispatch_round_trips_all_call_shapes() {
        let _plugin_lock = plugin_test_lock();
        let Some(path) = example_plugin_path() else {
            return;
        };
        let mut host = NylonRingHost::new();
        host.load("example", path.to_str().unwrap()).unwrap();
        let handle = host.plugin("example").unwrap();
        let runtime = tokio::runtime::Runtime::new().unwrap();

        assert!(matches!(
            handle.entry("no_such_entry"),
            Err(NylonRingHostError::EntryNotFound(_))
        ));

        let echo = handle.entry("benchmark").unwrap();
        let notify = handle.entry("benchmark_without_response").unwrap();
        let stream = handle.entry("benchmark_stream").unwrap();

        runtime.block_on(async {
            let (status, data) = echo.call_response(b"by-id").await.unwrap();
            assert_eq!(status, NrStatus::Ok);
            assert_eq!(data, b"by-id");

            let (status, data) = echo.call_response_fast(b"fast-by-id").await.unwrap();
            assert_eq!(status, NrStatus::Ok);
            assert_eq!(data, b"fast-by-id");

            assert_eq!(notify.call(b"").await.unwrap(), NrStatus::Ok);

            let (_sid, mut receiver) = stream.call_stream(b"").await.unwrap();
            let mut frames = 0u32;
            while receiver.recv().await.is_some() {
                frames += 1;
            }
            assert_eq!(frames, 9);
        });
    }

    #[test]
    fn pinned_plugin_calls_and_refuses_unload() {
        let _guard = plugin_test_lock();
        let Some(path) = example_plugin_path() else {
            return;
        };
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .unwrap();
        let mut host = NylonRingHost::new();
        host.load_pinned("pinned", path.to_str().unwrap()).unwrap();
        let handle = host.plugin("pinned").unwrap();
        runtime.block_on(async {
            let (status, data) = handle
                .call_response_fast("benchmark", b"pin")
                .await
                .unwrap();
            assert_eq!(status, NrStatus::Ok);
            assert_eq!(data, b"pin");
            let (status, data) = handle.call_response("benchmark", b"pin2").await.unwrap();
            assert_eq!(status, NrStatus::Ok);
            assert_eq!(data, b"pin2");
            // Streams still take real tracker slots on a pinned plugin.
            let (_sid, mut receiver) = handle.call_stream("stream", b"").await.unwrap();
            let mut frames = 0u32;
            while receiver.recv().await.is_some() {
                frames += 1;
            }
            assert!(frames > 0);
        });
        assert_eq!(handle.plugin.call_tracker.active_calls(), 0);
        assert!(matches!(
            host.unload("pinned"),
            Err(NylonRingHostError::PluginPinned(_))
        ));
        assert!(matches!(
            host.reload(),
            Err(NylonRingHostError::PluginPinned(_))
        ));
        assert!(matches!(
            host.load("pinned", path.to_str().unwrap()),
            Err(NylonRingHostError::PluginPinned(_))
        ));
        runtime.block_on(async {
            assert!(matches!(
                host.unload_with_grace("pinned", Duration::from_millis(10))
                    .await,
                Err(NylonRingHostError::PluginPinned(_))
            ));
            assert!(matches!(
                host.reload_with_grace(Duration::from_millis(10)).await,
                Err(NylonRingHostError::PluginPinned(_))
            ));
        });
    }

    /// Manual cycle-budget probe for the single-worker call paths. Run with:
    /// `cargo test --release -p nylon-ring-host --lib cycle_budget_probe -- --ignored --nocapture`
    #[test]
    #[ignore = "manual probe; needs --release for meaningful numbers"]
    fn cycle_budget_probe() {
        use std::future::Future;
        use std::hint::black_box;
        use std::task::{Context, Poll, Waker};

        let _guard = plugin_test_lock();
        let path = example_plugin_path().expect("example plugin must build");
        let mut host = NylonRingHost::new();
        host.load("bench", path.to_str().unwrap()).unwrap();
        let handle = host.plugin("bench").unwrap();
        let fire_entry = handle.entry("benchmark_without_response").unwrap();
        let fast_entry = handle.entry("benchmark").unwrap();

        const N: u64 = 20_000_000;

        fn time_ns(iters: u64, mut f: impl FnMut()) -> f64 {
            for _ in 0..iters / 10 {
                f();
            }
            let start = std::time::Instant::now();
            for _ in 0..iters {
                f();
            }
            start.elapsed().as_nanos() as f64 / iters as f64
        }

        // A 16-deep dependent add chain retires at 1/cycle on Apple Silicon:
        // measures the actual clock so cycle numbers below don't assume
        // nominal boost. The loop lives inside one asm block so the chain
        // never round-trips through memory; the loop counter's subs/branch
        // overlap with the chain and add no cycles.
        let calibrate = |iters: u64| -> f64 {
            let mut x: u64 = black_box(0);
            let start = std::time::Instant::now();
            #[cfg(target_arch = "aarch64")]
            unsafe {
                let mut n = iters;
                std::arch::asm!(
                    "2:",
                    "add {x}, {x}, #1", "add {x}, {x}, #1", "add {x}, {x}, #1",
                    "add {x}, {x}, #1", "add {x}, {x}, #1", "add {x}, {x}, #1",
                    "add {x}, {x}, #1", "add {x}, {x}, #1", "add {x}, {x}, #1",
                    "add {x}, {x}, #1", "add {x}, {x}, #1", "add {x}, {x}, #1",
                    "add {x}, {x}, #1", "add {x}, {x}, #1", "add {x}, {x}, #1",
                    "add {x}, {x}, #1",
                    "subs {n}, {n}, #1",
                    "b.ne 2b",
                    x = inout(reg) x,
                    n = inout(reg) n,
                );
                black_box(n);
            }
            black_box(x);
            start.elapsed().as_nanos() as f64 / iters as f64
        };
        calibrate(200_000_000); // DVFS ramp warmup
        let calib_ns = calibrate(100_000_000).min(calibrate(100_000_000));
        let ghz = 16.0 / calib_ns;
        println!("clock calibration: {calib_ns:.2} ns / 16 adds -> {ghz:.3} GHz");

        let cyc = |ns: f64| ns * ghz;
        let report = |name: &str, ns: f64| {
            println!("{name:<28} {ns:>7.2} ns  {:>6.1} cycles", cyc(ns));
        };

        let plugin = &fire_entry.plugin;
        let handle_by_id_fn = plugin.dispatch.handle_by_id.unwrap();
        let fire_id = fire_entry.id;

        report(
            "component: tracker begin+fin",
            time_ns(N, || {
                let shard = plugin.call_tracker.try_begin().unwrap();
                let _ = black_box(plugin.call_tracker.finish(shard));
            }),
        );

        // Atomic microbenches mimicking the tracker's shard word, on a
        // dedicated 128-byte-aligned line, to separate instruction choice
        // (CAS loop vs single fetch_add) from ordering strength.
        #[repr(align(128))]
        struct PaddedCounter(std::sync::atomic::AtomicUsize);
        use std::sync::atomic::Ordering as AtomOrd;
        let word = PaddedCounter(std::sync::atomic::AtomicUsize::new(0));
        report(
            "atomics: casa loop + ldaddl",
            time_ns(N, || {
                let _ = black_box(
                    word.0
                        .fetch_update(AtomOrd::Acquire, AtomOrd::Relaxed, |v| Some(v + 1)),
                );
                black_box(word.0.fetch_sub(1, AtomOrd::Release));
            }),
        );
        report(
            "atomics: relaxed cas + sub",
            time_ns(N, || {
                let _ = black_box(
                    word.0
                        .fetch_update(AtomOrd::Relaxed, AtomOrd::Relaxed, |v| Some(v + 1)),
                );
                black_box(word.0.fetch_sub(1, AtomOrd::Relaxed));
            }),
        );
        report(
            "atomics: ldadda + ldaddl",
            time_ns(N, || {
                black_box(word.0.fetch_add(1, AtomOrd::Acquire));
                black_box(word.0.fetch_sub(1, AtomOrd::Release));
            }),
        );
        report(
            "atomics: single ldadda",
            time_ns(N, || {
                black_box(word.0.fetch_add(1, AtomOrd::Acquire));
            }),
        );
        report(
            "atomics: relaxed cas + ldaddl",
            time_ns(N, || {
                let _ = black_box(
                    word.0
                        .fetch_update(AtomOrd::Relaxed, AtomOrd::Relaxed, |v| Some(v + 1)),
                );
                black_box(word.0.fetch_sub(1, AtomOrd::Release));
            }),
        );

        // B diagnostic: the same begin/finish pair with 8 threads, each on
        // its own thread-shard. The tracker is already sharded per thread,
        // so per-op cost should stay ~1x the single-thread figure.
        {
            let tracker = std::sync::Arc::new(crate::call_tracker::CallTracker::new());
            let per_thread: u64 = 4_000_000;
            let start = std::time::Instant::now();
            let threads: Vec<_> = (0..8)
                .map(|_| {
                    let tracker = tracker.clone();
                    std::thread::spawn(move || {
                        for _ in 0..per_thread {
                            let shard = tracker.try_begin().unwrap();
                            let _ = black_box(tracker.finish(shard));
                        }
                    })
                })
                .collect();
            for t in threads {
                t.join().unwrap();
            }
            let ns = start.elapsed().as_nanos() as f64 / per_thread as f64;
            report("tracker pair, 8 threads", ns);
        }
        report(
            "component: next_sid",
            time_ns(N, || {
                black_box(sid::next_sid());
            }),
        );
        report(
            "component: remove_state",
            time_ns(N, || {
                plugin.host_ctx.remove_state(black_box(1));
            }),
        );
        report(
            "raw FFI handle_by_id (fire)",
            time_ns(N, || {
                let status = unsafe {
                    handle_by_id_fn(black_box(fire_id), 1, NrBytes::from_slice(black_box(b"")))
                };
                assert_eq!(status, NrStatus::Ok);
            }),
        );

        let mut cx = Context::from_waker(Waker::noop());
        macro_rules! poll_ready {
            ($fut:expr) => {{
                let fut = $fut;
                let mut fut = std::pin::pin!(fut);
                match fut.as_mut().poll(&mut cx) {
                    Poll::Ready(result) => result.unwrap(),
                    Poll::Pending => panic!("probe future must complete synchronously"),
                }
            }};
        }

        report(
            "full fire (entry id)",
            time_ns(N, || {
                poll_ready!(fire_entry.call(black_box(b"")));
            }),
        );
        report(
            "full fire (by name)",
            time_ns(N, || {
                poll_ready!(handle.call("benchmark_without_response", black_box(b"")));
            }),
        );
        report(
            "full fast (entry id)",
            time_ns(N, || {
                black_box(poll_ready!(fast_entry.call_response_fast(black_box(b""))));
            }),
        );
        report(
            "full fast (by name)",
            time_ns(N, || {
                black_box(poll_ready!(
                    handle.call_response_fast("benchmark", black_box(b""))
                ));
            }),
        );
        report(
            "full unary (entry id)",
            time_ns(N, || {
                black_box(poll_ready!(fast_entry.call_response(black_box(b""))));
            }),
        );
        report(
            "full unary (by name)",
            time_ns(N, || {
                black_box(poll_ready!(
                    handle.call_response("benchmark", black_box(b""))
                ));
            }),
        );
    }
}
