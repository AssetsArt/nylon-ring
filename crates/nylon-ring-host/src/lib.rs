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
use context::{HostContext, CURRENT_UNARY_RESULT};
use libloading::{Library, Symbol};
use nylon_ring::{NrBytes, NrHostExt, NrHostVTable, NrPluginInfo, NrPluginVTable, NrStr};
use sid::next_sid;
use std::ffi::c_void;
use std::sync::Arc;
use types::{Pending, Result, StreamFrame, StreamReceiver};

pub use error::NylonRingHostError;
pub use extensions::Extensions;
pub use nylon_ring::NrStatus;
pub use types::StreamFrame as PublicStreamFrame;

/// The main host for loading and managing nylon-ring plugins.
pub struct NylonRingHost {
    _lib: Library,
    plugin_vtable: &'static NrPluginVTable,
    plugin_ctx: *mut c_void,
    host_ctx: Arc<HostContext>,
    host_vtable: Box<NrHostVTable>,
}

unsafe impl Send for NylonRingHost {}
unsafe impl Sync for NylonRingHost {}

impl NylonRingHost {
    /// Load a plugin from the specified path.
    ///
    /// # Arguments
    ///
    /// * `path` - Path to the plugin dynamic library
    ///
    /// # Returns
    ///
    /// Returns a `NylonRingHost` instance on success, or an error if the plugin
    /// cannot be loaded or initialized.
    pub fn load(path: &str) -> Result<Self> {
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

            // Create host context with internal host_ext
            let host_ctx = Arc::new(HostContext::new(NrHostExt {
                set_state: set_state_callback,
                get_state: get_state_callback,
            }));

            // Create host vtable
            let host_vtable = Box::new(NrHostVTable {
                send_result: send_result_vec_callback,
            });

            let mut host = Self {
                _lib: lib,
                plugin_vtable,
                plugin_ctx: std::ptr::null_mut(), // Will be set from plugin info
                host_ctx,
                host_vtable,
            };

            // plugin_ctx from plugin info
            host.plugin_ctx = info.plugin_ctx;

            // Initialize plugin
            if let Some(init_fn) = plugin_vtable.init {
                init_fn(
                    Arc::as_ptr(&host.host_ctx) as *mut c_void,
                    &*host.host_vtable,
                );
            }

            Ok(host)
        }
    }

    /// Call a plugin entry point with a request-response pattern.
    ///
    /// This method waits for the plugin to send a response via the callback.
    ///
    /// # Arguments
    ///
    /// * `entry` - The entry point name to call
    /// * `payload` - The request payload
    ///
    /// # Returns
    ///
    /// Returns a tuple of (status, response data) on success.
    pub async fn call_response(&self, entry: &str, payload: &[u8]) -> Result<(NrStatus, Vec<u8>)> {
        let sid = next_sid();
        let (tx, rx) = tokio::sync::oneshot::channel();

        {
            self.host_ctx
                .pending_requests
                .insert(sid, Pending::Unary(tx));
        }

        let payload_bytes = NrBytes::from_slice(payload);
        let handle_raw_fn = match self.plugin_vtable.handle {
            Some(f) => f,
            None => {
                let _ = self.host_ctx.pending_requests.remove(&sid);
                return Err(NylonRingHostError::MissingRequiredFunctions);
            }
        };

        let status = unsafe { handle_raw_fn(NrStr::new(entry), sid, payload_bytes) };

        if status != NrStatus::Ok {
            let _ = self.host_ctx.pending_requests.remove(&sid);
            return Err(NylonRingHostError::PluginHandleFailed(status));
        }

        rx.await.map_err(|_| NylonRingHostError::OneshotClosed)
    }

    /// Ultra-fast unary call for synchronous plugins.
    ///
    /// **Warning**: The plugin must call `send_result` synchronously on the
    /// same thread. Do not use with async or multi-threaded plugins.
    ///
    /// # Arguments
    ///
    /// * `entry` - The entry point name to call
    /// * `payload` - The request payload
    ///
    /// # Returns
    ///
    /// Returns a tuple of (status, response data) on success.
    pub async fn call_response_fast(
        &self,
        entry: &str,
        payload: &[u8],
    ) -> Result<(NrStatus, Vec<u8>)> {
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

        let handle_raw_fn = match self.plugin_vtable.handle {
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
    ///
    /// This method does not wait for a response.
    ///
    /// # Arguments
    ///
    /// * `entry` - The entry point name to call
    /// * `payload` - The request payload
    ///
    /// # Returns
    ///
    /// Returns the immediate status from the plugin's handle function.
    pub async fn call(&self, entry: &str, payload: &[u8]) -> Result<NrStatus> {
        let sid = next_sid();

        let payload_bytes = NrBytes::from_slice(payload);
        let handle_raw_fn = match self.plugin_vtable.handle {
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
    ///
    /// The plugin can send multiple responses for a single request.
    ///
    /// # Arguments
    ///
    /// * `entry` - The entry point name to call
    /// * `payload` - The request payload
    ///
    /// # Returns
    ///
    /// Returns a tuple of (session ID, stream receiver) on success.
    pub async fn call_stream(&self, entry: &str, payload: &[u8]) -> Result<(u64, StreamReceiver)> {
        let sid = next_sid();

        let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<StreamFrame>();

        {
            self.host_ctx
                .pending_requests
                .insert(sid, Pending::Stream(tx));
        }

        let payload_bytes = NrBytes::from_slice(payload);

        let handle_raw_fn = match self.plugin_vtable.handle {
            Some(f) => f,
            None => {
                let _ = self.host_ctx.pending_requests.remove(&sid);
                return Err(NylonRingHostError::MissingRequiredFunctions);
            }
        };

        let status = unsafe { handle_raw_fn(NrStr::new(entry), sid, payload_bytes) };

        if status != NrStatus::Ok {
            let _ = self.host_ctx.pending_requests.remove(&sid);
            return Err(NylonRingHostError::PluginHandleFailed(status));
        }

        Ok((sid, rx))
    }

    /// Send data to an active stream.
    ///
    /// # Arguments
    ///
    /// * `sid` - The session ID of the stream
    /// * `data` - The data to send
    ///
    /// # Returns
    ///
    /// Returns the status from the plugin's stream_data handler.
    pub fn send_stream_data(&self, sid: u64, data: &[u8]) -> Result<NrStatus> {
        let stream_data_fn = match self.plugin_vtable.stream_data {
            Some(f) => f,
            None => return Err(NylonRingHostError::MissingRequiredFunctions),
        };
        let payload = NrBytes::from_slice(data);
        Ok(unsafe { stream_data_fn(sid, payload) })
    }

    /// Close an active stream from the host side.
    ///
    /// The plugin must have implemented `stream_close` handler.
    ///
    /// # Arguments
    ///
    /// * `sid` - The session ID of the stream to close
    ///
    /// # Returns
    ///
    /// Returns the status from the plugin's stream_close handler.
    pub fn close_stream(&self, sid: u64) -> Result<NrStatus> {
        let stream_close_fn = match self.plugin_vtable.stream_close {
            Some(f) => f,
            None => return Err(NylonRingHostError::MissingRequiredFunctions),
        };
        Ok(unsafe { stream_close_fn(sid) })
    }

    /// Get host extension pointer from host_ctx.
    ///
    /// # Safety
    ///
    /// The caller must ensure that `host_ctx` is a valid pointer to a `HostContext`
    /// instance that was created by this host, or a null pointer. The returned pointer
    /// is valid for the lifetime of the `HostContext`.
    pub unsafe fn get_host_ext(host_ctx: *mut c_void) -> *const NrHostExt {
        if host_ctx.is_null() {
            return std::ptr::null();
        }
        let ctx = &*(host_ctx as *const HostContext);
        &ctx.host_ext as *const NrHostExt
    }
}

impl Drop for NylonRingHost {
    fn drop(&mut self) {
        if let Some(shutdown_fn) = self.plugin_vtable.shutdown {
            unsafe {
                shutdown_fn();
            }
        }
    }
}
