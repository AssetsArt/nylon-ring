use anyhow::{anyhow, Context, Result};
use libloading::{Library, Symbol};
use nylon_ring::{
    NrBytes, NrHeader, NrHostVTable, NrPluginInfo, NrPluginVTable, NrRequest, NrStatus, NrStr,
};
use std::collections::HashMap;
use std::ffi::c_void;
use std::sync::{Arc, Mutex};
use tokio::sync::oneshot;

pub struct HighLevelRequest {
    pub method: String,
    pub path: String,
    pub query: String,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

struct HostContext {
    pending_requests: Mutex<HashMap<u64, oneshot::Sender<(NrStatus, Vec<u8>)>>>,
}

pub struct NylonRingHost {
    _lib: Library, // Keep library loaded
    plugin_vtable: &'static NrPluginVTable,
    plugin_ctx: *mut c_void,
    host_ctx: Arc<HostContext>,
    host_vtable: Box<NrHostVTable>, // Stable address
    next_sid: std::sync::atomic::AtomicU64,
}

// Safety: The host is responsible for thread safety.
// libloading::Library is Send/Sync.
// Pointers need care, but we are designing this for async usage.
unsafe impl Send for NylonRingHost {}
unsafe impl Sync for NylonRingHost {}

impl NylonRingHost {
    pub fn load(path: &str) -> Result<Self> {
        unsafe {
            let lib = Library::new(path).context("Failed to load plugin library")?;
            let get_plugin: Symbol<extern "C" fn() -> *const NrPluginInfo> = lib
                .get(b"nylon_ring_get_plugin_v1\0")
                .context("Missing nylon_ring_get_plugin_v1 symbol")?;

            let info_ptr = get_plugin();
            if info_ptr.is_null() {
                return Err(anyhow!("Plugin info pointer is null"));
            }
            let info = &*info_ptr;

            if !info.compatible(1) {
                return Err(anyhow!(
                    "Incompatible ABI version. Expected 1, got {}",
                    info.abi_version
                ));
            }

            if info.vtable.is_null() {
                return Err(anyhow!("Plugin vtable is null"));
            }
            let plugin_vtable = &*info.vtable;

            if plugin_vtable.init.is_none() || plugin_vtable.handle.is_none() {
                return Err(anyhow!("Plugin vtable missing required functions"));
            }

            let host_ctx = Arc::new(HostContext {
                pending_requests: Mutex::new(HashMap::new()),
            });

            // Create host vtable
            let host_vtable = Box::new(NrHostVTable {
                send_result: Self::send_result_callback,
            });

            let mut host = Self {
                _lib: lib,
                plugin_vtable,
                plugin_ctx: std::ptr::null_mut(), // Will be set by plugin if it wants, but actually we pass it from info
                host_ctx,
                host_vtable,
                next_sid: std::sync::atomic::AtomicU64::new(1),
            };

            // The plugin info has a plugin_ctx field, which might be initialized by the plugin itself?
            // Wait, the ABI says:
            // pub plugin_ctx: *mut c_void,
            // pub vtable: *const NrPluginVTable,
            // And init takes: (plugin_ctx, host_ctx, host_vtable)
            // Usually the plugin_ctx in NrPluginInfo is the "global" context for that plugin instance.
            host.plugin_ctx = info.plugin_ctx;

            // Initialize plugin
            if let Some(init_fn) = plugin_vtable.init {
                let status = init_fn(
                    host.plugin_ctx,
                    Arc::as_ptr(&host.host_ctx) as *mut c_void,
                    &*host.host_vtable,
                );
                if status != NrStatus::Ok {
                    return Err(anyhow!("Plugin init failed with status {:?}", status));
                }
            }

            Ok(host)
        }
    }

    unsafe extern "C" fn send_result_callback(
        host_ctx: *mut c_void,
        sid: u64,
        status: NrStatus,
        payload: NrBytes,
    ) {
        let ctx = &*(host_ctx as *const HostContext);
        let mut map = ctx.pending_requests.lock().unwrap();
        if let Some(tx) = map.remove(&sid) {
            let data = payload.as_slice().to_vec();
            let _ = tx.send((status, data));
        }
    }

    pub async fn call(&self, req: HighLevelRequest) -> Result<(NrStatus, Vec<u8>)> {
        let sid = self
            .next_sid
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();

        {
            let mut map = self.host_ctx.pending_requests.lock().unwrap();
            map.insert(sid, tx);
        }

        // Convert HighLevelRequest to NrRequest
        // We need to keep the owned strings alive during the call
        // Since handle() must return immediately, we can just keep them in this scope?
        // Wait, handle() returns immediately, but it might spawn a thread that uses the data?
        // NO. The ABI says: "Plugin is required to copy what it needs and return immediately."
        // So we only need to keep them alive for the duration of the `handle` call.

        let method_str = req.method;
        let path_str = req.path;
        let query_str = req.query;

        // Convert headers
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

        let handle_fn = self.plugin_vtable.handle.unwrap();
        let status = unsafe { handle_fn(self.plugin_ctx, sid, &nr_req, payload) };

        if status != NrStatus::Ok {
            // Remove from map if immediate failure
            let mut map = self.host_ctx.pending_requests.lock().unwrap();
            map.remove(&sid);
            return Err(anyhow!(
                "Plugin handle failed immediately with status {:?}",
                status
            ));
        }

        // Wait for callback
        rx.await.context("Failed to receive response from plugin")
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
