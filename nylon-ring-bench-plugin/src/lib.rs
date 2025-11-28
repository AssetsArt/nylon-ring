use nylon_ring::{NrBytes, NrHostVTable, NrPluginInfo, NrPluginVTable, NrRequest, NrStatus, NrStr};
use std::ffi::c_void;
use std::mem::size_of;
use std::sync::OnceLock;

struct HostHandle {
    ctx: *mut c_void,
    vtable: *const NrHostVTable,
}

// Safety: The host guarantees that the context and vtable are thread-safe or handles concurrency.
unsafe impl Send for HostHandle {}
unsafe impl Sync for HostHandle {}

static HOST_HANDLE: OnceLock<HostHandle> = OnceLock::new();

extern "C" fn plugin_init(
    _plugin_ctx: *mut c_void,
    host_ctx: *mut c_void,
    host_vtable: *const NrHostVTable,
) -> NrStatus {
    let handle = HostHandle {
        ctx: host_ctx,
        vtable: host_vtable,
    };
    if HOST_HANDLE.set(handle).is_err() {
        return NrStatus::Err;
    }
    NrStatus::Ok
}

extern "C" fn plugin_handle(
    _plugin_ctx: *mut c_void,
    sid: u64,
    req: *const NrRequest,
    _payload: NrBytes,
) -> NrStatus {
    // Copy minimal data needed
    let req_ref = unsafe { &*req };
    let path = req_ref.path.as_str().to_string();

    // Spawn background thread that responds immediately (no sleep)
    std::thread::spawn(move || {
        if let Some(host) = HOST_HANDLE.get() {
            let send_result = unsafe { (*host.vtable).send_result };

            // For streaming requests, send multiple frames quickly
            if path == "/stream" {
                for i in 1..=5 {
                    let msg = format!("Frame {}", i);
                    unsafe {
                        send_result(
                            host.ctx,
                            sid,
                            NrStatus::Ok,
                            NrBytes::from_slice(msg.as_bytes()),
                        );
                    }
                }
                // End stream
                unsafe {
                    send_result(host.ctx, sid, NrStatus::StreamEnd, NrBytes::from_slice(&[]));
                }
            } else {
                // Unary response - respond immediately
                let response_string = format!("OK: {}", path);
                let response_bytes = response_string.as_bytes();
                unsafe {
                    send_result(
                        host.ctx,
                        sid,
                        NrStatus::Ok,
                        NrBytes::from_slice(response_bytes),
                    );
                }
            }
        }
    });

    NrStatus::Ok
}

extern "C" fn plugin_shutdown(_plugin_ctx: *mut c_void) {
    // No cleanup needed
}

static PLUGIN_VTABLE: NrPluginVTable = NrPluginVTable {
    init: Some(plugin_init),
    handle: Some(plugin_handle),
    shutdown: Some(plugin_shutdown),
};

static PLUGIN_INFO: NrPluginInfo = NrPluginInfo {
    abi_version: 1,
    struct_size: size_of::<NrPluginInfo>() as u32,
    name: NrStr {
        ptr: "nylon_ring_bench_plugin".as_ptr(),
        len: 23,
    },
    version: NrStr {
        ptr: "0.1.0".as_ptr(),
        len: 5,
    },
    plugin_ctx: std::ptr::null_mut(),
    vtable: &PLUGIN_VTABLE,
};

#[no_mangle]
pub extern "C" fn nylon_ring_get_plugin_v1() -> *const NrPluginInfo {
    &PLUGIN_INFO
}
