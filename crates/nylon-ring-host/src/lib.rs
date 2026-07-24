//! Nylon Ring Host - A high-performance plugin host for the nylon-ring ABI.
//!
//! This crate provides the host-side implementation for loading and managing
//! plugins that conform to the nylon-ring ABI. It supports multiple execution
//! modes including fire-and-forget calls, request-response patterns, and
//! bidirectional streaming.

mod callbacks;
mod context;
mod error;
mod extensions;
mod sid;
mod types;

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

pub use error::NylonRingHostError;
pub use extensions::Extensions;
pub use nylon_ring::NrStatus;
pub use types::{Result, StreamFrame, StreamReceiver};

struct PendingGuard {
    host_ctx: Arc<HostContext>,
    sid: u64,
}

impl PendingGuard {
    fn new(host_ctx: Arc<HostContext>, sid: u64) -> Self {
        Self { host_ctx, sid }
    }
}

impl Drop for PendingGuard {
    fn drop(&mut self) {
        context::cleanup_sid(&self.host_ctx, self.sid);
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
    /// Call a plugin entry point with a request-response pattern.
    pub async fn call_response(&self, entry: &str, payload: &[u8]) -> Result<(NrStatus, Vec<u8>)> {
        // Create Oneshot Channel
        let (tx, rx) = tokio::sync::oneshot::channel();

        // Generate SID
        let sid = next_sid();

        // Insert into Map (Async Path)
        context::insert_pending(&self.plugin.host_ctx, sid, types::Pending::Unary(tx));
        let _pending_guard = PendingGuard::new(self.plugin.host_ctx.clone(), sid);

        let payload_bytes = NrBytes::from_slice(payload);
        let handle_raw_fn = match self.plugin.vtable.handle {
            Some(f) => f,
            None => return Err(NylonRingHostError::MissingRequiredFunctions),
        };

        let status = unsafe { handle_raw_fn(NrStr::new(entry), sid, payload_bytes) };

        if status != NrStatus::Ok {
            return Err(NylonRingHostError::PluginHandleFailed(status));
        }

        // Wait for response (Allocation here for oneshot state)
        rx.await.map_err(|_| NylonRingHostError::OneshotClosed)
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
        let (tx, rx) = tokio::sync::oneshot::channel();
        let sid = next_sid();

        context::insert_pending(&self.plugin.host_ctx, sid, types::Pending::Unary(tx));

        let payload_bytes = NrBytes::from_slice(payload);
        let handle_raw_fn = match self.plugin.vtable.handle {
            Some(f) => f,
            None => {
                context::remove_pending(&self.plugin.host_ctx, sid);
                return Err(NylonRingHostError::MissingRequiredFunctions);
            }
        };

        let status = unsafe { handle_raw_fn(NrStr::new(entry), sid, payload_bytes) };
        if status != NrStatus::Ok {
            context::remove_pending(&self.plugin.host_ctx, sid);
            return Err(NylonRingHostError::PluginHandleFailed(status));
        }

        match tokio::time::timeout(timeout, rx).await {
            Ok(Ok(v)) => Ok(v),
            Ok(Err(_)) => Err(NylonRingHostError::OneshotClosed),
            Err(_) => {
                // Drop the still-registered pending slot so a late callback
                // does not write into a freed oneshot sender.
                context::remove_pending(&self.plugin.host_ctx, sid);
                Err(NylonRingHostError::Timeout)
            }
        }
    }

    /// Ultra-fast unary call for synchronous plugins.
    pub async fn call_response_fast(
        &self,
        entry: &str,
        payload: &[u8],
    ) -> Result<(NrStatus, Vec<u8>)> {
        // RAII guard: ensures the TLS slot pointer is cleared even if the
        // plugin's `handle` callback panics across the FFI boundary. Without
        // this, an unwind would leave a dangling pointer to a stack slot,
        // and the next thread-local consumer could write into freed memory.
        struct TlsSlotGuard;
        impl Drop for TlsSlotGuard {
            fn drop(&mut self) {
                CURRENT_UNARY_RESULT.with(|cell| cell.set(std::ptr::null_mut()));
            }
        }

        let mut slot = types::UnaryResultSlot { sid, result: None };

        let binding = FastSlotBinding::bind(&mut slot)?;

        let payload_bytes = NrBytes::from_slice(payload);

        let handle_raw_fn = match self.plugin.vtable.handle {
            Some(f) => f,
            None => return Err(NylonRingHostError::MissingRequiredFunctions),
        };

        let status = unsafe { handle_raw_fn(NrStr::new(entry), sid, payload_bytes) };

        drop(binding);
        self.plugin.host_ctx.state_per_sid.remove(&sid);

        if status != NrStatus::Ok {
            return Err(NylonRingHostError::PluginHandleFailed(status));
        }

        match slot.result {
            Some((st, data)) => Ok((st, data)),
            None => Err(NylonRingHostError::OneshotClosed),
        }
    }

    /// Fire-and-forget call to a plugin entry point.
    pub async fn call(&self, entry: &str, payload: &[u8]) -> Result<NrStatus> {
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
        self.plugin.host_ctx.state_per_sid.remove(&sid);

        if status != NrStatus::Ok {
            return Err(NylonRingHostError::PluginHandleFailed(status));
        }
        Ok(status)
    }

    /// Call a plugin entry point with a streaming response pattern.
    pub async fn call_stream(&self, entry: &str, payload: &[u8]) -> Result<(u64, StreamReceiver)> {
        let sid = next_sid();

        let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<StreamFrame>();

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
            return Err(NylonRingHostError::PluginHandleFailed(status));
        }

        Ok((
            sid,
            StreamReceiver::new(rx, self.plugin.host_ctx.clone(), sid),
        ))
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
        let host_ctx = Arc::new(HostContext::new(NrHostExt {
            set_state: set_state_callback,
            get_state: get_state_callback,
        }));

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
            };

            self.plugins.insert(name.to_string(), Arc::new(loaded));
            Ok(())
        }
    }

    /// Unload a plugin by name.
    pub fn unload(&mut self, name: &str) -> Result<()> {
        self.plugins.remove(name);
        Ok(())
    }

    /// Reload all plugins.
    pub fn reload(&mut self) -> Result<()> {
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

    /// Get a handle to a loaded plugin by name.
    pub fn plugin(&self, name: &str) -> Option<PluginHandle> {
        self.plugins
            .get(name)
            .map(|p| PluginHandle { plugin: p.clone() })
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

    #[test]
    fn dropping_pending_guard_unregisters_unary_request() {
        let host = NylonRingHost::new();
        let sid = 42;
        let (tx, _rx) = tokio::sync::oneshot::channel();
        context::insert_pending(&host.host_ctx, sid, types::Pending::Unary(tx));
        host.host_ctx.state_per_sid.insert(sid, HashMap::new());

        drop(PendingGuard::new(host.host_ctx.clone(), sid));

        assert!(context::remove_pending(&host.host_ctx, sid).is_none());
        assert!(!host.host_ctx.state_per_sid.contains_key(&sid));
    }

    #[test]
    fn dropping_stream_receiver_unregisters_stream() {
        let host = NylonRingHost::new();
        let sid = 43;
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        context::insert_pending(&host.host_ctx, sid, types::Pending::Stream(tx));
        host.host_ctx.state_per_sid.insert(sid, HashMap::new());

        drop(StreamReceiver::new(rx, host.host_ctx.clone(), sid));

        assert!(context::remove_pending(&host.host_ctx, sid).is_none());
        assert!(!host.host_ctx.state_per_sid.contains_key(&sid));
    }
}
