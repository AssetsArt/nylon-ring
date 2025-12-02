mod error;
mod extensions;

use dashmap::DashMap;
pub use error::NylonRingHostError;
pub use extensions::Extensions;
use libloading::{Library, Symbol};
pub use nylon_ring::NrStatus;
use nylon_ring::{
    NrBytes, NrHeader, NrHostExt, NrHostVTable, NrPluginInfo, NrPluginVTable, NrRequest, NrStr,
};
use rustc_hash::FxBuildHasher;
use std::cell::Cell;
use std::ffi::c_void;
use std::panic;
use std::sync::Arc;
use std::{collections::HashMap, time::Instant};
use tokio::sync::{mpsc, oneshot};

type Result<T> = std::result::Result<T, NylonRingHostError>;

pub struct HighLevelRequest {
    pub method: String,
    pub path: String,
    pub query: String,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
    /// Extensions: type-safe key-value storage for custom metadata.
    /// These are not sent to the plugin but can be used by the host for routing, logging, etc.
    pub extensions: Extensions,
}

/// One cell in the pending map:
/// - Unary: standard request/response
/// - Stream: streaming / websocket-style
enum Pending {
    #[allow(dead_code)]
    Unary(oneshot::Sender<(NrStatus, Vec<u8>)>),
    Stream(mpsc::UnboundedSender<StreamFrame>),
}

/// One frame in a streaming response.
#[derive(Debug)]
pub struct StreamFrame {
    pub status: NrStatus,
    pub data: Vec<u8>,
}

/// Receiver type for streaming.
pub type StreamReceiver = mpsc::UnboundedReceiver<StreamFrame>;

// Note: This struct must match the layout expected by plugins.
// Plugins access host_ext field directly, so we need #[repr(C)] for ABI compatibility.
// Type aliases using FxBuildHasher for faster integer-key hashing
type FastPendingMap = DashMap<u64, Pending, FxBuildHasher>;
type FastStateMap = DashMap<u64, HashMap<String, Vec<u8>>, FxBuildHasher>;

#[repr(C)]
struct HostContext {
    pending_requests: FastPendingMap,
    state_per_sid: FastStateMap,
    host_ext: *const NrHostExt, // Pointer to host extension (stable address)
}

pub struct NylonRingHost {
    _lib: Library, // Keep library loaded
    plugin_vtable: &'static NrPluginVTable,
    plugin_ctx: *mut c_void,
    host_ctx: Arc<HostContext>,
    host_vtable: Box<NrHostVTable>, // Stable address
    #[allow(dead_code)]
    host_ext: Box<NrHostExt>, // Stable address for state extension
    next_sid: std::sync::atomic::AtomicU64,
}

// Safety: The host is responsible for thread safety.
unsafe impl Send for NylonRingHost {}
unsafe impl Sync for NylonRingHost {}

// Ultra-fast unary mode:
thread_local! {
    static CURRENT_UNARY_TX: Cell<*mut Option<oneshot::Sender<(NrStatus, Vec<u8>)>>> =
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

            if plugin_vtable.init.is_none()
                || (plugin_vtable.handle.is_none() && plugin_vtable.handle_raw.is_none())
            {
                return Err(NylonRingHostError::MissingRequiredFunctions);
            }

            let host_ctx = Arc::new(HostContext {
                pending_requests: FastPendingMap::with_hasher(FxBuildHasher::default()),
                state_per_sid: FastStateMap::with_hasher(FxBuildHasher::default()),
                host_ext: std::ptr::null(), // Will be set after creating host_ext
            });

            // Create host vtable
            let host_vtable = Box::new(NrHostVTable {
                send_result: Self::send_result_callback,
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
                        host.plugin_ctx,
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

    /// Callback called from plugin (any thread)
    /// - มี 2 path:
    ///   1) ultra fast unary: ใช้ thread-local CURRENT_UNARY_TX
    ///   2) ปกติ: ใช้ DashMap pending_requests
    /// This function is panic-safe and will not propagate panics across FFI boundary.
    unsafe extern "C" fn send_result_callback(
        host_ctx: *mut c_void,
        sid: u64,
        status: NrStatus,
        payload: NrBytes,
    ) {
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            if host_ctx.is_null() {
                return;
            }
            let ctx = &*(host_ctx as *const HostContext);

            // ── ultra fast unary path ──
            let mut handled_fast = false;
            CURRENT_UNARY_TX.with(|cell| {
                let ptr = cell.get();
                if !ptr.is_null() {
                    // ptr: *mut Option<Sender<_>>
                    let slot: &mut Option<oneshot::Sender<(NrStatus, Vec<u8>)>> =
                        unsafe { &mut *ptr };

                    if let Some(tx) = slot.take() {
                        let data = payload.as_slice().to_vec();
                        let _ = tx.send((status, data));
                        ctx.state_per_sid.remove(&sid);
                        handled_fast = true;
                    }
                }
            });

            if handled_fast {
                return;
            }

            // ── ปกติ: ใช้ DashMap pending_requests ──
            let should_clear_state = if let Some((_, entry)) = ctx.pending_requests.remove(&sid) {
                let data = payload.as_slice().to_vec();
                match entry {
                    Pending::Unary(tx) => {
                        let _ = tx.send((status, data));
                        true
                    }
                    Pending::Stream(tx) => {
                        let _ = tx.send(StreamFrame { status, data });
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

    /// Callback for setting state (called from plugin)
    /// This function is panic-safe and will not propagate panics across FFI boundary.
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
            let value_vec = value.as_slice().to_vec();

            ctx.state_per_sid
                .entry(sid)
                .or_insert_with(HashMap::new)
                .insert(key_str, value_vec);

            // Return empty bytes on success
            NrBytes::from_slice(&[])
        }));

        // Return empty bytes on panic (safe fallback)
        result.unwrap_or_else(|_| NrBytes::from_slice(&[]))
    }

    /// Callback for getting state (called from plugin)
    /// This function is panic-safe and will not propagate panics across FFI boundary.
    unsafe extern "C" fn get_state_callback(
        host_ctx: *mut c_void,
        sid: u64,
        key: NrStr,
    ) -> NrBytes {
        let result = panic::catch_unwind(panic::AssertUnwindSafe(|| {
            if host_ctx.is_null() {
                return NrBytes::from_slice(&[]);
            }
            let ctx = &*(host_ctx as *const HostContext);

            let key_str = key.as_str();
            if let Some(sid_state) = ctx.state_per_sid.get(&sid) {
                if let Some(value) = sid_state.get(key_str) {
                    return NrBytes::from_slice(value);
                }
            }

            // Return empty bytes if not found
            NrBytes::from_slice(&[])
        }));

        // Return empty bytes on panic (safe fallback)
        result.unwrap_or_else(|_| NrBytes::from_slice(&[]))
    }

    /// Unary RPC: plugin should call send_result exactly once for this sid.
    /// ใช้ DashMap + oneshot → safety & support async
    pub async fn call(&self, entry: &str, req: HighLevelRequest) -> Result<(NrStatus, Vec<u8>)> {
        let sid = self
            .next_sid
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();

        {
            self.host_ctx
                .pending_requests
                .insert(sid, Pending::Unary(tx));
        }

        let method_str = req.method;
        let path_str = req.path;
        let query_str = req.query;

        let header_objs: Vec<NrHeader> = req
            .headers
            .iter()
            .map(|(k, v)| NrHeader::new(k, v))
            .collect();

        let nr_req = NrRequest {
            path: NrStr::from_str(&path_str),
            method: NrStr::from_str(&method_str),
            query: NrStr::from_str(&query_str),
            headers: header_objs.as_ptr(),
            headers_len: header_objs.len() as u32,
            _reserved0: 0,
            _reserved1: 0,
        };

        let payload = NrBytes::from_slice(&req.body);

        let handle_fn = match self.plugin_vtable.handle {
            Some(f) => f,
            None => return Err(NylonRingHostError::MissingRequiredFunctions),
        };

        let status = panic::catch_unwind(panic::AssertUnwindSafe(|| unsafe {
            handle_fn(
                self.plugin_ctx,
                NrStr::from_str(entry),
                sid,
                &nr_req,
                payload,
            )
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

        rx.await.map_err(|_| NylonRingHostError::OneshotClosed)
    }

    /// Streaming call: plugin may call send_result multiple times.
    /// The stream closes when plugin sends one of:
    /// - NrStatus::StreamEnd
    /// - NrStatus::Err / Invalid / Unsupported
    pub async fn call_stream(
        &self,
        entry: &str,
        req: HighLevelRequest,
    ) -> Result<(u64, StreamReceiver)> {
        let sid = self
            .next_sid
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);

        let (tx, rx) = mpsc::unbounded_channel::<StreamFrame>();

        {
            self.host_ctx
                .pending_requests
                .insert(sid, Pending::Stream(tx));
        }

        let method_str = req.method;
        let path_str = req.path;
        let query_str = req.query;

        let header_objs: Vec<NrHeader> = req
            .headers
            .iter()
            .map(|(k, v)| NrHeader::new(k, v))
            .collect();

        let nr_req = NrRequest {
            path: NrStr::from_str(&path_str),
            method: NrStr::from_str(&method_str),
            query: NrStr::from_str(&query_str),
            headers: header_objs.as_ptr(),
            headers_len: header_objs.len() as u32,
            _reserved0: 0,
            _reserved1: 0,
        };

        let payload = NrBytes::from_slice(&req.body);

        let handle_fn = match self.plugin_vtable.handle {
            Some(f) => f,
            None => return Err(NylonRingHostError::MissingRequiredFunctions),
        };

        let status = panic::catch_unwind(panic::AssertUnwindSafe(|| unsafe {
            handle_fn(
                self.plugin_ctx,
                NrStr::from_str(entry),
                sid,
                &nr_req,
                payload,
            )
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

    /// Raw Streaming call: plugin may call send_result multiple times.
    /// The stream closes when plugin sends one of:
    /// - NrStatus::StreamEnd
    /// - NrStatus::Err / Invalid / Unsupported
    pub async fn call_raw_stream(
        &self,
        entry: &str,
        payload: &[u8],
    ) -> Result<(u64, StreamReceiver)> {
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

        let handle_raw_fn = match self.plugin_vtable.handle_raw {
            Some(f) => f,
            None => {
                let _ = self.host_ctx.pending_requests.remove(&sid);
                return Err(NylonRingHostError::MissingRequiredFunctions);
            }
        };

        let status = panic::catch_unwind(panic::AssertUnwindSafe(|| unsafe {
            handle_raw_fn(self.plugin_ctx, NrStr::from_str(entry), sid, payload_bytes)
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

    /// Raw RPC: plugin should call send_result exactly once for this sid.
    /// ใช้ DashMap + oneshot (ปลอดภัย รองรับ async/thread)
    pub async fn call_raw(&self, entry: &str, payload: &[u8]) -> Result<(NrStatus, Vec<u8>)> {
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
        self.call_raw_internal(sid, entry, &payload_bytes)?;
        rx.await.map_err(|_| NylonRingHostError::OneshotClosed)
    }

    pub async fn call_raw_nr_bytes(
        &self,
        entry: &str,
        payload: &NrBytes,
    ) -> Result<(NrStatus, Vec<u8>)> {
        let sid = self
            .next_sid
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();

        {
            self.host_ctx
                .pending_requests
                .insert(sid, Pending::Unary(tx));
        }
        self.call_raw_internal(sid, entry, payload)?;
        rx.await.map_err(|_| NylonRingHostError::OneshotClosed)
    }

    fn call_raw_internal(&self, sid: u64, entry: &str, payload: &NrBytes) -> Result<NrStatus> {
        let handle_raw_fn = match self.plugin_vtable.handle_raw {
            Some(f) => f,
            None => {
                let _ = self.host_ctx.pending_requests.remove(&sid);
                return Err(NylonRingHostError::MissingRequiredFunctions);
            }
        };

        let status = panic::catch_unwind(panic::AssertUnwindSafe(|| unsafe {
            handle_raw_fn(self.plugin_ctx, NrStr::from_str(entry), sid, *payload)
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

        Ok(status)
    }

    /// Ultra-fast unary raw call:
    pub async fn call_raw_unary_fast(
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

        let handle_raw_fn = match self.plugin_vtable.handle_raw {
            Some(f) => f,
            None => {
                CURRENT_UNARY_TX.with(|cell| cell.set(std::ptr::null_mut()));
                return Err(NylonRingHostError::MissingRequiredFunctions);
            }
        };

        let status = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe {
            handle_raw_fn(self.plugin_ctx, NrStr::from_str(entry), sid, payload_bytes)
        }));

        // เคลียร์ thread-local ให้แน่ใจว่าไม่ค้าง
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

        // ตอนนี้ plugin น่าจะเรียก send_result ไปแล้ว → rx จะ ready
        let (st, data) = rx.await.map_err(|_| NylonRingHostError::OneshotClosed)?;
        Ok((st, data))
    }

    /// Send data to an active stream (bidirectional streaming).
    /// The plugin must have implemented `stream_data` handler.
    pub fn send_stream_data(&self, sid: u64, data: &[u8]) -> Result<NrStatus> {
        let stream_data_fn = match self.plugin_vtable.stream_data {
            Some(f) => f,
            None => return Err(NylonRingHostError::MissingRequiredFunctions),
        };

        let payload = NrBytes::from_slice(data);
        let status = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe {
            stream_data_fn(self.plugin_ctx, sid, payload)
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
            stream_close_fn(self.plugin_ctx, sid)
        }));

        match status {
            Ok(s) => Ok(s),
            Err(_) => Err(NylonRingHostError::PluginHandleFailed(NrStatus::Err)),
        }
    }

    /// Get host extension pointer from host_ctx.
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
                shutdown_fn(self.plugin_ctx);
            }
        }
    }
}
