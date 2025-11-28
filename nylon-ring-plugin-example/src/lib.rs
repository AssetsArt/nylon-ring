use nylon_ring::{NrBytes, NrHostVTable, NrPluginInfo, NrPluginVTable, NrRequest, NrStatus, NrStr};
use std::ffi::c_void;
use std::mem::size_of;
use std::sync::OnceLock;
use std::thread;
use std::time::Duration;

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
    // Copy data needed for background task
    let req_ref = unsafe { &*req };
    let method = req_ref.method.as_str().to_string();
    let path = req_ref.path.as_str().to_string();

    // Spawn background thread
    thread::spawn(move || {
        if path == "/stream" {
            // Streaming response: 5 frames, 1 per second
            if let Some(host) = HOST_HANDLE.get() {
                let send_result = unsafe { (*host.vtable).send_result };

                for i in 1..=5 {
                    thread::sleep(Duration::from_secs(1));
                    let msg = format!("Frame {}/5 from {}", i, path);
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
            }
        } else {
            // Unary response
            thread::sleep(Duration::from_secs(2));

            let response_string = format!("OK: {} {}", method, path);
            let response_bytes = response_string.as_bytes();

            if let Some(host) = HOST_HANDLE.get() {
                let send_result = unsafe { (*host.vtable).send_result };
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
    // Cleanup if needed
}

static PLUGIN_VTABLE: NrPluginVTable = NrPluginVTable {
    init: Some(plugin_init),
    handle: Some(plugin_handle),
    shutdown: Some(plugin_shutdown),
};

// Safety: These types are ABI-stable data carriers.
// Users must ensure that the pointers they contain are valid and accessed safely.
// We need to implement Sync for NrPluginInfo because it is used in a static.
// The ABI types in nylon-ring crate already implement Send/Sync unsafe.

static PLUGIN_INFO: NrPluginInfo = NrPluginInfo {
    abi_version: 1,
    struct_size: size_of::<NrPluginInfo>() as u32,
    name: NrStr {
        ptr: "nylon_ring_plugin_example".as_ptr(),
        len: 25,
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
