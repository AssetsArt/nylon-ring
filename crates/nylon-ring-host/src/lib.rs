mod error;
mod extensions;

use dashmap::DashMap;
pub use error::NylonRingHostError;
pub use extensions::Extensions;
use libloading::{Library, Symbol};
pub use nylon_ring::NrStatus;
use nylon_ring::{NrBytes, NrHostExt, NrHostVTable, NrPluginInfo, NrPluginVTable, NrStr};
use rustc_hash::FxBuildHasher;
use std::cell::Cell;
use std::ffi::c_void;
use std::panic;
use std::sync::Arc;
use std::{collections::HashMap, time::Instant};
use tokio::sync::{mpsc, oneshot};

type Result<T> = std::result::Result<T, NylonRingHostError>;
enum Pending {
    #[allow(dead_code)]
    Unary(oneshot::Sender<(NrStatus, Vec<u8>)>),
    Stream(mpsc::UnboundedSender<StreamFrame>),
}
#[derive(Debug)]
pub struct StreamFrame {
    pub status: NrStatus,
    pub data: Vec<u8>,
}
pub type StreamReceiver = mpsc::UnboundedReceiver<StreamFrame>;
type FastPendingMap = DashMap<u64, Pending, FxBuildHasher>;
type FastStateMap = DashMap<u64, HashMap<String, NrBytes>, FxBuildHasher>;
type UnarySender = Option<oneshot::Sender<(NrStatus, Vec<u8>)>>;
#[repr(C)]
struct HostContext {
    pending_requests: FastPendingMap,
    state_per_sid: FastStateMap,
    host_ext: *const NrHostExt,
}

// Safety: HostContext can be safely shared across threads because:
// - FastPendingMap and FastStateMap (DashMap) are already thread-safe
// - host_ext is a raw pointer to a stable Box that outlives HostContext
unsafe impl Send for HostContext {}
unsafe impl Sync for HostContext {}
pub struct NylonRingHost {
    _lib: Library,
    plugin_vtable: &'static NrPluginVTable,
    plugin_ctx: *mut c_void,
    host_ctx: Arc<HostContext>,
    host_vtable: Box<NrHostVTable>,
    #[allow(dead_code)]
    host_ext: Box<NrHostExt>,
    next_sid: std::sync::atomic::AtomicU64,
}
unsafe impl Send for NylonRingHost {}
unsafe impl Sync for NylonRingHost {}

thread_local! {
    static CURRENT_UNARY_TX: Cell<*mut UnarySender> =
        const { Cell::new(std::ptr::null_mut()) };
}

impl NylonRingHost {
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

            let host_ctx = Arc::new(HostContext {
                pending_requests: FastPendingMap::with_hasher(FxBuildHasher),
                state_per_sid: FastStateMap::with_hasher(FxBuildHasher),
                host_ext: std::ptr::null(), // Will be set after creating host_ext
            });

            // Create host vtable
            let host_vtable = Box::new(NrHostVTable {
                send_result: Self::send_result_vec_callback,
            });

            // Create host extension for state management
            let host_ext = Box::new(NrHostExt {
                set_state: Self::set_state_callback,
                get_state: Self::get_state_callback,
            });
            let host_ext_ptr = &*host_ext as *const NrHostExt;

            // Update host_ctx with host_ext pointer
            {
                let ctx_ptr = Arc::as_ptr(&host_ctx) as *mut HostContext;
                (*ctx_ptr).host_ext = host_ext_ptr;
            }

            let mut host = Self {
                _lib: lib,
                plugin_vtable,
                plugin_ctx: std::ptr::null_mut(), // Will be set from plugin info
                host_ctx,
                host_vtable,
                host_ext,
                next_sid: std::sync::atomic::AtomicU64::new(1),
            };

            // plugin_ctx from plugin info
            host.plugin_ctx = info.plugin_ctx;

            // Initialize plugin
            if let Some(init_fn) = plugin_vtable.init {
                let status = panic::catch_unwind(panic::AssertUnwindSafe(|| {
                    init_fn(
                        Arc::as_ptr(&host.host_ctx) as *mut c_void,
                        &*host.host_vtable,
                    )
                }));

                match status {
                    Ok(NrStatus::Ok) => {}
                    Ok(other) => return Err(NylonRingHostError::PluginInitFailed(other)),
                    Err(_) => {
                        return Err(NylonRingHostError::PluginInitFailed(NrStatus::Err));
                    }
                }
            }

            Ok(host)
        }
    }

    unsafe extern "C" fn send_result_vec_callback(
        host_ctx: *mut c_void,
        sid: u64,
        status: NrStatus,
        payload: nylon_ring::NrVec<u8>,
    ) {
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            if host_ctx.is_null() {
                return;
            }
            let ctx = &*(host_ctx as *const HostContext);

            // Convert NrVec to Vec<u8> (Zero Copy)
            let mut data_vec = Some(payload.into_vec());

            // ── ultra fast unary path ──
            let mut handled_fast = false;
            CURRENT_UNARY_TX.with(|cell| {
                let ptr = cell.get();
                if !ptr.is_null() {
                    let slot: &mut Option<oneshot::Sender<(NrStatus, Vec<u8>)>> =
                        unsafe { &mut *ptr };

                    if let Some(tx) = slot.take() {
                        if let Some(data) = data_vec.take() {
                            let _ = tx.send((status, data));
                        }
                        ctx.state_per_sid.remove(&sid);
                        handled_fast = true;
                    }
                }
            });

            if handled_fast {
                return;
            }

            let data_vec = match data_vec.take() {
                Some(v) => v,
                None => return, // Already consumed
            };

            let should_clear_state = if let Some((_, entry)) = ctx.pending_requests.remove(&sid) {
                match entry {
                    Pending::Unary(tx) => {
                        let _ = tx.send((status, data_vec));
                        true
                    }
                    Pending::Stream(tx) => {
                        let _ = tx.send(StreamFrame {
                            status,
                            data: data_vec,
                        });
                        let is_finished = matches!(
                            status,
                            NrStatus::Err
                                | NrStatus::Invalid
                                | NrStatus::Unsupported
                                | NrStatus::StreamEnd
                        );
                        if !is_finished {
                            ctx.pending_requests.insert(sid, Pending::Stream(tx));
                        }
                        is_finished
                    }
                }
            } else {
                false
            };

            if should_clear_state {
                ctx.state_per_sid.remove(&sid);
            }
        }));

        let _ = result;
    }

    unsafe extern "C" fn set_state_callback(
        host_ctx: *mut c_void,
        sid: u64,
        key: NrStr,
        value: NrBytes,
    ) -> NrBytes {
        let result = panic::catch_unwind(panic::AssertUnwindSafe(|| {
            if host_ctx.is_null() {
                return NrBytes::from_slice(&[]);
            }
            let ctx = &*(host_ctx as *const HostContext);

            let key_str = key.as_str().to_string();

            ctx.state_per_sid
                .entry(sid)
                .or_default()
                .insert(key_str, value);

            // Return empty bytes on success
            NrBytes::from_slice(&[])
        }));

        // Return empty bytes on panic (safe fallback)
        result.unwrap_or_else(|_| NrBytes::from_slice(&[]))
    }

    unsafe extern "C" fn get_state_callback(
        host_ctx: *mut c_void,
        sid: u64,
        key: NrStr,
    ) -> NrBytes {
        if host_ctx.is_null() {
            return NrBytes::from_slice(&[]);
        }
        let ctx = &*(host_ctx as *const HostContext);

        let key_str = key.as_str();
        if let Some(sid_state) = ctx.state_per_sid.get(&sid) {
            if let Some(value) = sid_state.get(key_str) {
                return *value;
            }
        }

        // Return empty bytes if not found
        NrBytes::from_slice(&[])
    }

    pub async fn call(&self, entry: &str, payload: &[u8]) -> Result<(NrStatus, Vec<u8>)> {
        let _now = Instant::now();
        let sid = self
            .next_sid
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();

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

    pub async fn call_unary_fast(
        &self,
        entry: &str,
        payload: &[u8],
    ) -> Result<(NrStatus, Vec<u8>)> {
        let sid = self
            .next_sid
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);

        // Sender/Receiver ปกติ
        let (tx, rx) = oneshot::channel::<(NrStatus, Vec<u8>)>();

        // เก็บ Sender ไว้ใน Option เพื่อให้ callback มา .take() ไปใช้
        let mut tx_slot: Option<oneshot::Sender<(NrStatus, Vec<u8>)>> = Some(tx);

        CURRENT_UNARY_TX.with(|cell| {
            debug_assert!(
                cell.get().is_null(),
                "CURRENT_UNARY_TX already in use on this thread"
            );
            cell.set(&mut tx_slot as *mut _);
        });

        let payload_bytes = NrBytes::from_slice(payload);

        let handle_raw_fn = match self.plugin_vtable.handle {
            Some(f) => f,
            None => {
                CURRENT_UNARY_TX.with(|cell| cell.set(std::ptr::null_mut()));
                return Err(NylonRingHostError::MissingRequiredFunctions);
            }
        };

        let status = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe {
            handle_raw_fn(NrStr::new(entry), sid, payload_bytes)
        }));

        CURRENT_UNARY_TX.with(|cell| cell.set(std::ptr::null_mut()));

        let status = match status {
            Ok(s) => s,
            Err(_) => {
                return Err(NylonRingHostError::PluginHandleFailed(NrStatus::Err));
            }
        };

        if status != NrStatus::Ok {
            return Err(NylonRingHostError::PluginHandleFailed(status));
        }

        let (st, data) = rx.await.map_err(|_| NylonRingHostError::OneshotClosed)?;
        Ok((st, data))
    }

    pub async fn call_stream(&self, entry: &str, payload: &[u8]) -> Result<(u64, StreamReceiver)> {
        let sid = self
            .next_sid
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);

        let (tx, rx) = mpsc::unbounded_channel::<StreamFrame>();

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

        let status = panic::catch_unwind(panic::AssertUnwindSafe(|| unsafe {
            handle_raw_fn(NrStr::new(entry), sid, payload_bytes)
        }));

        let status = match status {
            Ok(s) => s,
            Err(_) => {
                let _ = self.host_ctx.pending_requests.remove(&sid);
                return Err(NylonRingHostError::PluginHandleFailed(NrStatus::Err));
            }
        };

        if status != NrStatus::Ok {
            let _ = self.host_ctx.pending_requests.remove(&sid);
            return Err(NylonRingHostError::PluginHandleFailed(status));
        }

        Ok((sid, rx))
    }

    pub fn send_stream_data(&self, sid: u64, data: &[u8]) -> Result<NrStatus> {
        let stream_data_fn = match self.plugin_vtable.stream_data {
            Some(f) => f,
            None => return Err(NylonRingHostError::MissingRequiredFunctions),
        };

        let payload = NrBytes::from_slice(data);
        let status = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe {
            stream_data_fn(sid, payload)
        }));

        match status {
            Ok(s) => Ok(s),
            Err(_) => Err(NylonRingHostError::PluginHandleFailed(NrStatus::Err)),
        }
    }

    /// Close an active stream from the host side.
    /// The plugin must have implemented `stream_close` handler.
    pub fn close_stream(&self, sid: u64) -> Result<NrStatus> {
        let stream_close_fn = match self.plugin_vtable.stream_close {
            Some(f) => f,
            None => return Err(NylonRingHostError::MissingRequiredFunctions),
        };

        let status = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe {
            stream_close_fn(sid)
        }));

        match status {
            Ok(s) => Ok(s),
            Err(_) => Err(NylonRingHostError::PluginHandleFailed(NrStatus::Err)),
        }
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
        ctx.host_ext
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
