mod error;
mod extensions;

use dashmap::DashMap;
pub use error::NylonRingHostError;
pub use extensions::Extensions;
use libloading::{Library, Symbol};
pub use nylon_ring::NrStatus;
use nylon_ring::{NrBytes, NrHostExt, NrHostVTable, NrPluginInfo, NrPluginVTable, NrStr};
use rustc_hash::FxBuildHasher;
use std::collections::HashMap;
use std::ffi::c_void;
use std::sync::Arc;
use std::{
    cell::Cell,
    sync::atomic::{AtomicU64, Ordering},
};
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
type FastStateMap = DashMap<u64, HashMap<String, Vec<u8>>, FxBuildHasher>;
type UnarySender = Option<oneshot::Sender<(NrStatus, Vec<u8>)>>;
type UnaryResultSlot = Option<(NrStatus, Vec<u8>)>;

/// Host context shared with the plugin (opaque on the plugin side).
///
/// Note:
/// - `host_ext` now lives **inside** `HostContext` → no dangling pointer when
///   `NylonRingHost` is dropped.
#[repr(C)]
struct HostContext {
    pending_requests: FastPendingMap,
    state_per_sid: FastStateMap,
    host_ext: NrHostExt,
}

// Safety: HostContext can be safely shared across threads because:
// - FastPendingMap and FastStateMap (DashMap) are thread-safe
// - NrHostExt only contains function pointers, which are Send + Sync
unsafe impl Send for HostContext {}
unsafe impl Sync for HostContext {}

pub struct NylonRingHost {
    _lib: Library,
    plugin_vtable: &'static NrPluginVTable,
    plugin_ctx: *mut c_void,
    host_ctx: Arc<HostContext>,
    host_vtable: Box<NrHostVTable>,
}

unsafe impl Send for NylonRingHost {}
unsafe impl Sync for NylonRingHost {}

const SID_BLOCK_SIZE: u64 = 1_000_000;
static GLOBAL_SID: AtomicU64 = AtomicU64::new(1);

#[derive(Copy, Clone)]
struct SidBlock {
    base: u64,
    offset: u64,
}

thread_local! {
    // ใช้สำหรับ fast-path เดิม (call_response ที่ใช้ oneshot) – ตอนนี้ยังมีใช้แค่ใน call_response
    static CURRENT_UNARY_TX: Cell<*mut UnarySender> =
        const { Cell::new(std::ptr::null_mut()) };

    // ultra-fast path: plugin เขียน result ลง slot นี้ตรง ๆ
    static CURRENT_UNARY_RESULT: Cell<*mut UnaryResultSlot> =
        const { Cell::new(std::ptr::null_mut()) };

    static THREAD_LOCAL_SID_BLOCK: Cell<SidBlock> = const { Cell::new(SidBlock {
         base: 0,
         offset: SID_BLOCK_SIZE,
     }) };
}

fn next_sid() -> u64 {
    THREAD_LOCAL_SID_BLOCK.with(|cell| {
        let mut block = cell.get();
        if block.offset >= SID_BLOCK_SIZE {
            let base = GLOBAL_SID.fetch_add(SID_BLOCK_SIZE, Ordering::Relaxed);
            block = SidBlock { base, offset: 0 };
        }
        let sid = block.base + block.offset;
        block.offset += 1;
        cell.set(block);
        sid
    })
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

            // Create host context with internal host_ext
            let host_ctx = Arc::new(HostContext {
                pending_requests: FastPendingMap::with_hasher(FxBuildHasher),
                state_per_sid: FastStateMap::with_hasher(FxBuildHasher),
                host_ext: NrHostExt {
                    set_state: Self::set_state_callback,
                    get_state: Self::get_state_callback,
                },
            });

            // Create host vtable
            let host_vtable = Box::new(NrHostVTable {
                send_result: Self::send_result_vec_callback,
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

    unsafe extern "C" fn send_result_vec_callback(
        host_ctx: *mut c_void,
        sid: u64,
        status: NrStatus,
        payload: nylon_ring::NrVec<u8>,
    ) {
        if host_ctx.is_null() {
            return;
        }
        let ctx = &*(host_ctx as *const HostContext);

        // Convert NrVec to Vec<u8>
        let mut data_vec = Some(payload.into_vec());

        // ── ULTRA FAST DIRECT SLOT (call_response_fast) ──
        let mut handled_fast = false;

        CURRENT_UNARY_RESULT.with(|cell| {
            let ptr = cell.get();
            if !ptr.is_null() {
                let slot: &mut UnaryResultSlot = unsafe { &mut *ptr };

                if let Some(data) = data_vec.take() {
                    *slot = Some((status, data));
                }
                ctx.state_per_sid.remove(&sid);
                handled_fast = true;
            }
        });

        if handled_fast {
            return;
        }

        // ── FAST PATH: oneshot sender (call_response ปกติที่ใช้ pending_requests: Unary) ──
        CURRENT_UNARY_TX.with(|cell| {
            let ptr = cell.get();
            if !ptr.is_null() {
                let slot: &mut UnarySender = unsafe { &mut *ptr };

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

        // ── STREAM / ASYNC PATH: DashMap ──
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
    }

    unsafe extern "C" fn set_state_callback(
        host_ctx: *mut c_void,
        sid: u64,
        key: NrStr,
        value: NrBytes,
    ) -> NrBytes {
        if host_ctx.is_null() {
            return NrBytes::from_slice(&[]);
        }
        let ctx = &*(host_ctx as *const HostContext);

        let key_str = key.as_str().to_string();

        // Copy data from NrBytes to owned Vec<u8>
        let value_vec = value.as_slice().to_vec();

        ctx.state_per_sid
            .entry(sid)
            .or_default()
            .insert(key_str, value_vec);

        // Return empty bytes on success
        NrBytes::from_slice(&[])
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
                // Return NrBytes pointing to the Vec<u8> data
                return NrBytes::from_slice(value.as_slice());
            }
        }

        // Return empty bytes if not found
        NrBytes::from_slice(&[])
    }

    pub async fn call_response(&self, entry: &str, payload: &[u8]) -> Result<(NrStatus, Vec<u8>)> {
        let sid = next_sid();
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

    /// Ultra-fast unary call:
    /// - plugin ต้องเรียก send_result แบบ synchronous บน thread เดียวกัน
    /// - ห้ามใช้กับ plugin ที่ async / cross-thread
    pub async fn call_response_fast(
        &self,
        entry: &str,
        payload: &[u8],
    ) -> Result<(NrStatus, Vec<u8>)> {
        let sid = next_sid();

        let mut slot: UnaryResultSlot = None;

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

    pub async fn call_stream(&self, entry: &str, payload: &[u8]) -> Result<(u64, StreamReceiver)> {
        let sid = next_sid();

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

        let status = unsafe { handle_raw_fn(NrStr::new(entry), sid, payload_bytes) };

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
        Ok(unsafe { stream_data_fn(sid, payload) })
    }

    /// Close an active stream from the host side.
    /// The plugin must have implemented `stream_close` handler.
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
