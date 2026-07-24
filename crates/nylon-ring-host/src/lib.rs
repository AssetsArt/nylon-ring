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
use callbacks::{get_state_callback, send_result_vec_callback, set_state_callback};
use context::{CURRENT_UNARY_RESULT, HostContext};
use libloading::{Library, Symbol};
use nylon_ring::{
    ABI_VERSION, NrBytes, NrHostExt, NrHostVTable, NrPluginInfo, NrPluginVTable, NrStr,
};
use sid::next_sid;
use std::collections::HashMap;
use std::ffi::c_void;
use std::sync::Arc;
use std::time::Duration;

pub use error::NylonRingHostError;
pub use extensions::Extensions;
pub use nylon_ring::NrStatus;
pub use types::{Result, StreamFrame, StreamReceiver};

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

/// A loaded plugin instance.
pub struct LoadedPlugin {
    _lib: Library,
    vtable: &'static NrPluginVTable,
    host_ctx: Arc<HostContext>,
    path: String,
    call_tracker: CallTracker,
}

impl LoadedPlugin {
    fn begin_call(&self) -> Result<BorrowedPluginCallGuard<'_>> {
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
            plugin: self.clone(),
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
        self.tracker.finish(self.shard);
    }
}

pub(crate) struct PluginCallGuard {
    plugin: Arc<LoadedPlugin>,
    shard: usize,
}

impl Drop for PluginCallGuard {
    fn drop(&mut self) {
        self.plugin.call_tracker.finish(self.shard);
    }
}

impl Drop for LoadedPlugin {
    fn drop(&mut self) {
        if let Some(shutdown_fn) = self.vtable.shutdown {
            unsafe {
                shutdown_fn();
            }
        }
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

        let payload_bytes = NrBytes::from_slice(payload);
        let handle_raw_fn = match self.plugin.vtable.handle {
            Some(f) => f,
            None => return Err(NylonRingHostError::MissingRequiredFunctions),
        };

        let status = unsafe { handle_raw_fn(NrStr::new(entry), sid, payload_bytes) };

        if status != NrStatus::Ok {
            return Err(Self::status_error(status));
        }

        let result = context::wait_for_unary(&self.plugin.host_ctx, sid)
            .await
            .ok_or(NylonRingHostError::PluginUnloaded);
        if result.is_ok() {
            pending_guard.disarm();
        }
        result
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

        let payload_bytes = NrBytes::from_slice(payload);
        let handle_raw_fn = match self.plugin.vtable.handle {
            Some(f) => f,
            None => return Err(NylonRingHostError::MissingRequiredFunctions),
        };

        let status = unsafe { handle_raw_fn(NrStr::new(entry), sid, payload_bytes) };
        if status != NrStatus::Ok {
            return Err(Self::status_error(status));
        }

        match tokio::time::timeout(timeout, context::wait_for_unary(&self.plugin.host_ctx, sid))
            .await
        {
            Ok(Some(v)) => {
                pending_guard.disarm();
                Ok(v)
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
        let mut slot = types::UnaryResultSlot { sid, result: None };

        let binding = FastSlotBinding::bind(&mut slot)?;

        let payload_bytes = NrBytes::from_slice(payload);

        let handle_raw_fn = match self.plugin.vtable.handle {
            Some(f) => f,
            None => return Err(NylonRingHostError::MissingRequiredFunctions),
        };

        let status = unsafe { handle_raw_fn(NrStr::new(entry), sid, payload_bytes) };

        drop(binding);
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
        let handle_raw_fn = match self.plugin.vtable.handle {
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

        // Register the stream channel (Map)
        context::insert_pending(&self.plugin.host_ctx, sid, types::Pending::Stream(tx));

        let payload_bytes = NrBytes::from_slice(payload);

        let handle_raw_fn = match self.plugin.vtable.handle {
            Some(f) => f,
            None => {
                context::cleanup_sid(&self.plugin.host_ctx, sid);
                return Err(NylonRingHostError::MissingRequiredFunctions);
            }
        };

        let status = unsafe { handle_raw_fn(NrStr::new(entry), sid, payload_bytes) };

        if status != NrStatus::Ok {
            context::cleanup_sid(&self.plugin.host_ctx, sid);
            return Err(Self::status_error(status));
        }

        Ok((sid, StreamReceiver::new(rx, None, sid, Some(call_guard))))
    }

    /// Send data to an active stream.
    pub fn send_stream_data(&self, sid: u64, data: &[u8]) -> Result<NrStatus> {
        let stream_data_fn = match self.plugin.vtable.stream_data {
            Some(f) => f,
            None => return Err(NylonRingHostError::MissingRequiredFunctions),
        };
        let payload = NrBytes::from_slice(data);
        Ok(unsafe { stream_data_fn(sid, payload) })
    }

    /// Close an active stream from the host side.
    pub fn close_stream(&self, sid: u64) -> Result<NrStatus> {
        let stream_close_fn = match self.plugin.vtable.stream_close {
            Some(f) => f,
            None => return Err(NylonRingHostError::MissingRequiredFunctions),
        };
        Ok(unsafe { stream_close_fn(sid) })
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
        });

        Self {
            plugins: HashMap::new(),
            host_ctx,
            host_vtable,
        }
    }

    /// Load a plugin from the specified path with a given name.
    pub fn load(&mut self, name: &str, path: &str) -> Result<()> {
        unsafe {
            let lib = Library::new(path).map_err(NylonRingHostError::FailedToLoadLibrary)?;

            let get_plugin: Symbol<extern "C" fn() -> *const NrPluginInfo> =
                lib.get(b"nylon_ring_get_plugin_v1\0").map_err(|_| {
                    NylonRingHostError::MissingSymbol("nylon_ring_get_plugin_v1".to_string())
                })?;

            let info_ptr = get_plugin();
            if info_ptr.is_null() {
                return Err(NylonRingHostError::NullPluginInfo);
            }
            let abi_version = std::ptr::read_unaligned(info_ptr.cast::<u32>());
            let struct_size = std::ptr::read_unaligned(info_ptr.cast::<u32>().add(1));

            if abi_version != ABI_VERSION {
                return Err(NylonRingHostError::IncompatibleAbiVersion {
                    expected: ABI_VERSION,
                    actual: abi_version,
                });
            }

            let expected_size = std::mem::size_of::<NrPluginInfo>() as u32;
            if struct_size < expected_size {
                return Err(NylonRingHostError::IncompatiblePluginInfoSize {
                    expected: expected_size,
                    actual: struct_size,
                });
            }

            let info = &*info_ptr;

            if info.vtable.is_null() {
                return Err(NylonRingHostError::NullPluginVTable);
            }
            let plugin_vtable = &*info.vtable;

            if plugin_vtable.init.is_none() || plugin_vtable.handle.is_none() {
                return Err(NylonRingHostError::MissingRequiredFunctions);
            }

            // Initialize plugin
            if let Some(init_fn) = plugin_vtable.init {
                let status = init_fn(
                    Arc::as_ptr(&self.host_ctx) as *mut c_void,
                    &*self.host_vtable,
                );
                if status != NrStatus::Ok {
                    return Err(NylonRingHostError::PluginInitFailed(status));
                }
            }

            let loaded = LoadedPlugin {
                _lib: lib,
                vtable: plugin_vtable,
                host_ctx: self.host_ctx.clone(),
                path: path.to_string(),
                call_tracker: CallTracker::new(),
            };

            if let Some(previous) = self.plugins.insert(name.to_string(), Arc::new(loaded)) {
                previous.stop_accepting_calls();
            }
            Ok(())
        }
    }

    /// Unload a plugin by name.
    pub fn unload(&mut self, name: &str) -> Result<()> {
        let plugin = self
            .plugins
            .remove(name)
            .ok_or_else(|| NylonRingHostError::PluginNotFound(name.to_owned()))?;
        plugin.stop_accepting_calls();
        Ok(())
    }

    /// Stop accepting new calls, then wait for tracked calls to drain.
    pub async fn unload_with_grace(&mut self, name: &str, grace: Duration) -> Result<()> {
        let plugin = self
            .plugins
            .remove(name)
            .ok_or_else(|| NylonRingHostError::PluginNotFound(name.to_owned()))?;
        plugin.stop_accepting_calls();
        Self::wait_for_drain(&plugin, grace).await
    }

    /// Reload all plugins.
    pub fn reload(&mut self) -> Result<()> {
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
        let old_plugins: Vec<_> = self
            .plugins
            .drain()
            .map(|(name, plugin)| {
                plugin.stop_accepting_calls();
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
                StreamFrame {
                    status: NrStatus::Ok,
                    data: vec![7],
                },
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
                StreamFrame {
                    status: NrStatus::Ok,
                    data: vec![8],
                },
            ),
            NrStatus::Invalid
        );

        match std::future::Future::poll(future.as_mut(), &mut second_context) {
            std::task::Poll::Ready(Some((status, data))) => {
                assert_eq!(status, NrStatus::Ok);
                assert_eq!(data, vec![7]);
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
        };
        let mut second = types::UnaryResultSlot {
            sid: 2,
            result: None,
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
                StreamFrame {
                    status: NrStatus::Ok,
                    data: vec![1],
                },
            ),
            NrStatus::Ok
        );
        assert_eq!(
            context::dispatch_pending(
                &host.host_ctx,
                sid,
                StreamFrame {
                    status: NrStatus::Ok,
                    data: vec![2],
                },
            ),
            NrStatus::Backpressure
        );
        assert_eq!(rx.try_recv().unwrap().data, vec![1]);
        assert_eq!(
            context::dispatch_pending(
                &host.host_ctx,
                sid,
                StreamFrame {
                    status: NrStatus::StreamEnd,
                    data: vec![],
                },
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
                            StreamFrame {
                                status: NrStatus::StreamEnd,
                                data: vec![value],
                            },
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
    fn example_plugin_fire_and_forget_entry_returns_ok() {
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
    }
}
