use nylon_ring::{
    NrBytes, NrHostExt, NrHostVTable, NrPluginInfo, NrPluginVTable, NrRequest, NrStatus, NrStr,
};
use std::ffi::c_void;
use std::mem::size_of;
use std::panic;
use std::sync::OnceLock;
use std::thread;
use std::time::Duration;

struct HostHandle {
    ctx: *mut c_void,
    vtable: *const NrHostVTable,
    ext: *const NrHostExt, // Host extension for state management
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
    // Panic-safe: catch any panics before they cross FFI boundary
    let result = panic::catch_unwind(panic::AssertUnwindSafe(|| {
        // Get host extension from host_ctx
        // For now, we'll get it from HostContext structure
        // In a real implementation, this would be passed through init or retrieved differently
        let host_ext = unsafe {
            if host_ctx.is_null() {
                std::ptr::null()
            } else {
                // Access host_ext from HostContext
                // This is a simplified approach - in production, you might want a different mechanism
                let ctx = &*(host_ctx as *const HostContext);
                ctx.host_ext
            }
        };

        let handle = HostHandle {
            ctx: host_ctx,
            vtable: host_vtable,
            ext: host_ext,
        };
        if HOST_HANDLE.set(handle).is_err() {
            return NrStatus::Err;
        }
        NrStatus::Ok
    }));

    result.unwrap_or(NrStatus::Err)
}

// Helper struct to access HostContext (must match host implementation)
#[repr(C)]
struct HostContext {
    _pending_requests: *mut std::ffi::c_void, // Mutex<HashMap> - opaque
    _state_per_sid: *mut std::ffi::c_void,    // Mutex<HashMap> - opaque
    host_ext: *const NrHostExt,
}

extern "C" fn plugin_handle(
    _plugin_ctx: *mut c_void,
    sid: u64,
    req: *const NrRequest,
    _payload: NrBytes,
) -> NrStatus {
    // Panic-safe: catch any panics before they cross FFI boundary
    let result = panic::catch_unwind(panic::AssertUnwindSafe(|| {
        // Validate pointers
        if req.is_null() {
            return NrStatus::Invalid;
        }

        // Copy data needed for background task
        let req_ref = unsafe { &*req };
        let method = match std::str::from_utf8(unsafe {
            std::slice::from_raw_parts(req_ref.method.ptr, req_ref.method.len as usize)
        }) {
            Ok(s) => s.to_string(),
            Err(_) => return NrStatus::Invalid,
        };
        let path = match std::str::from_utf8(unsafe {
            std::slice::from_raw_parts(req_ref.path.ptr, req_ref.path.len as usize)
        }) {
            Ok(s) => s.to_string(),
            Err(_) => return NrStatus::Invalid,
        };

        // Spawn background thread
        thread::spawn(move || {
            if let Some(host) = HOST_HANDLE.get() {
                let send_result = unsafe { (*host.vtable).send_result };

                if path == "/stream" {
                    // Streaming response: 5 frames, 1 per second
                    // Demonstrate state usage: store frame count
                    for i in 1..=5 {
                        // Store state: frame number
                        if !host.ext.is_null() {
                            let frame_key = "frame_count";
                            let frame_value = i.to_string();
                            unsafe {
                                let set_state = (*host.ext).set_state;
                                set_state(
                                    host.ctx,
                                    sid,
                                    NrStr::from_str(frame_key),
                                    NrBytes::from_slice(frame_value.as_bytes()),
                                );
                            }
                        }

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
                } else if path == "/state-demo" {
                    // Unary response with state demonstration
                    // Set some state
                    if !host.ext.is_null() {
                        unsafe {
                            let set_state = (*host.ext).set_state;
                            set_state(
                                host.ctx,
                                sid,
                                NrStr::from_str("request_path"),
                                NrBytes::from_slice(path.as_bytes()),
                            );
                            set_state(
                                host.ctx,
                                sid,
                                NrStr::from_str("request_method"),
                                NrBytes::from_slice(method.as_bytes()),
                            );
                        }

                        // Get state back to demonstrate
                        unsafe {
                            let get_state = (*host.ext).get_state;
                            let stored_path =
                                get_state(host.ctx, sid, NrStr::from_str("request_path"));
                            let path_str = String::from_utf8_lossy(stored_path.as_slice());
                            let response_string =
                                format!("OK: {} {} (stored: {})", method, path, path_str);
                            let response_bytes = response_string.as_bytes();
                            send_result(
                                host.ctx,
                                sid,
                                NrStatus::Ok,
                                NrBytes::from_slice(response_bytes),
                            );
                        }
                    } else {
                        // Fallback if state not available
                        let response_string = format!("OK: {} {}", method, path);
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
                } else {
                    // Regular unary response
                    thread::sleep(Duration::from_secs(2));

                    let response_string = format!("OK: {} {}", method, path);
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
    }));

    result.unwrap_or(NrStatus::Err)
}

extern "C" fn plugin_shutdown(_plugin_ctx: *mut c_void) {
    // Panic-safe: catch any panics before they cross FFI boundary
    let _ = panic::catch_unwind(panic::AssertUnwindSafe(|| {
        // Cleanup if needed
    }));
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
