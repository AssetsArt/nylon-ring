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

use callbacks::{
    dispatch_async, dispatch_fast, dispatch_stream, dispatch_sync, get_state_callback,
    send_result_vec_callback, set_state_callback, stream_close, stream_read, stream_write,
};
use context::{HostContext, CURRENT_UNARY_RESULT};
use dashmap::DashMap;
use libloading::{Library, Symbol};
use nylon_ring::{NrBytes, NrHostExt, NrHostVTable, NrPluginInfo, NrPluginVTable, NrStr};
use sid::next_sid;
use std::ffi::c_void;
use std::sync::Arc;
use types::{Result, StreamFrame, StreamReceiver};

pub use error::NylonRingHostError;
pub use extensions::Extensions;
pub use nylon_ring::NrStatus;
pub use types::StreamFrame as PublicStreamFrame;

/// A loaded plugin instance.
pub struct LoadedPlugin {
    _lib: Library,
    pub(crate) vtable: &'static NrPluginVTable,
    #[allow(dead_code)]
    plugin_ctx: *mut c_void,
    host_ctx: Arc<HostContext>,
    path: String,
}

unsafe impl Send for LoadedPlugin {}
unsafe impl Sync for LoadedPlugin {}

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

        // Wait for response (Allocation here for oneshot state)
        rx.await.map_err(|_| NylonRingHostError::OneshotClosed)
    }

    /// Ultra-fast unary call for synchronous plugins.
    pub async fn call_response_fast(
        &self,
        entry: &str,
        payload: &[u8],
    ) -> Result<(NrStatus, Vec<u8>)> {
        // Use a "Fast SID" that bypasses the Map (High bit set)
        let sid = next_sid();

        let mut slot: types::UnaryResultSlot = None;

        // bind TLS slot
        CURRENT_UNARY_RESULT.with(|cell| {
            debug_assert!(
                cell.get().is_null(),
                "CURRENT_UNARY_RESULT already in use on this thread"
            );
            cell.set(&mut slot as *mut _);
        });

        let payload_bytes = NrBytes::from_slice(payload);

        let handle_raw_fn = match self.plugin.vtable.handle {
            Some(f) => f,
            None => {
                CURRENT_UNARY_RESULT.with(|cell| cell.set(std::ptr::null_mut()));
                return Err(NylonRingHostError::MissingRequiredFunctions);
            }
        };

        let status = unsafe { handle_raw_fn(NrStr::new(entry), sid, payload_bytes) };

        // unbind TLS slot
        CURRENT_UNARY_RESULT.with(|cell| cell.set(std::ptr::null_mut()));

        if status != NrStatus::Ok {
            return Err(NylonRingHostError::PluginHandleFailed(status));
        }

        match slot {
            Some((st, data)) => Ok((st, data)),
            None => Err(NylonRingHostError::OneshotClosed),
        }
    }

    /// Fire-and-forget call to a plugin entry point.
    pub async fn call(&self, entry: &str, payload: &[u8]) -> Result<NrStatus> {
        // Use Fast SID
        let sid = next_sid() | 0x8000_0000_0000_0000;

        let payload_bytes = NrBytes::from_slice(payload);
        let handle_raw_fn = match self.plugin.vtable.handle {
            Some(f) => f,
            None => {
                return Err(NylonRingHostError::MissingRequiredFunctions);
            }
        };

        let status = unsafe { handle_raw_fn(NrStr::new(entry), sid, payload_bytes) };

        if status != NrStatus::Ok {
            return Err(NylonRingHostError::PluginHandleFailed(status));
        }
        Ok(status)
    }

    /// Call a plugin entry point with a streaming response pattern.
    pub async fn call_stream(&self, entry: &str, payload: &[u8]) -> Result<(u64, StreamReceiver)> {
        let sid = next_sid();

        let (tx, rx) = std::sync::mpsc::channel::<StreamFrame>();

        // Register the stream channel (Map)
        context::insert_pending(&self.plugin.host_ctx, sid, types::Pending::Stream(tx));

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

        Ok((sid, rx))
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
    plugins: types::PluginRegistry,
    host_ctx: Arc<HostContext>,
    host_vtable: Box<NrHostVTable>,
}

unsafe impl Send for NylonRingHost {}
unsafe impl Sync for NylonRingHost {}

impl Default for NylonRingHost {
    fn default() -> Self {
        Self::new()
    }
}

impl NylonRingHost {
    /// Create a new empty host.
    pub fn new() -> Self {
        let plugins = Arc::new(DashMap::new());

        let host_ctx = Arc::new(HostContext::new(
            NrHostExt {
                set_state: set_state_callback,
                get_state: get_state_callback,
            },
            Arc::downgrade(&plugins),
        ));

        let host_vtable = Box::new(NrHostVTable {
            send_result: send_result_vec_callback,
            dispatch_sync,
            dispatch_fast,
            dispatch_async,
            dispatch_stream,
            stream_read,
            stream_write,
            stream_close,
        });

        Self {
            plugins,
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
            let info = &*info_ptr;

            if !info.compatible(1) {
                return Err(NylonRingHostError::IncompatibleAbiVersion {
                    expected: 1,
                    actual: info.abi_version,
                });
            }

            if info.vtable.is_null() {
                return Err(NylonRingHostError::NullPluginVTable);
            }
            let plugin_vtable = &*info.vtable;

            if plugin_vtable.init.is_none() || plugin_vtable.handle.is_none() {
                return Err(NylonRingHostError::MissingRequiredFunctions);
            }

            // Plugin context from info
            let plugin_ctx = info.plugin_ctx;

            // Initialize plugin
            if let Some(init_fn) = plugin_vtable.init {
                init_fn(
                    Arc::as_ptr(&self.host_ctx) as *mut c_void,
                    &*self.host_vtable,
                );
            }

            let loaded = LoadedPlugin {
                _lib: lib,
                vtable: plugin_vtable,
                plugin_ctx,
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
        for entry in self.plugins.iter() {
            plugins_to_reload.push((entry.key().clone(), entry.value().path.clone()));
        }

        // Remove all first (triggers shutdown)
        self.plugins.clear();

        // Load them back
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
        let ctx = &*(host_ctx as *const HostContext);
        &ctx.host_ext as *const NrHostExt
    }
}
