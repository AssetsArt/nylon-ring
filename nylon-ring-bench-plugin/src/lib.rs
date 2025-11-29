use nylon_ring::{NrBytes, NrHostVTable, NrRequest, NrStatus};
use std::ffi::c_void;
use std::sync::OnceLock;

struct HostHandle {
    ctx: *mut c_void,
    vtable: *const NrHostVTable,
}

// Safety: The host guarantees that the context and vtable are thread-safe or handles concurrency.
unsafe impl Send for HostHandle {}
unsafe impl Sync for HostHandle {}

static HOST_HANDLE: OnceLock<HostHandle> = OnceLock::new();

unsafe fn plugin_init(
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

// Handlers
unsafe fn handle_stream(
    _plugin_ctx: *mut c_void,
    sid: u64,
    req: *const NrRequest,
    _payload: NrBytes,
) -> NrStatus {
    if req.is_null() {
        return NrStatus::Invalid;
    }
    let req_ref = &*req;
    let _path = match std::str::from_utf8(std::slice::from_raw_parts(
        req_ref.path.ptr,
        req_ref.path.len as usize,
    )) {
        Ok(s) => s.to_string(),
        Err(_) => return NrStatus::Invalid,
    };

    std::thread::spawn(move || {
        if let Some(host) = HOST_HANDLE.get() {
            let send_result = (*host.vtable).send_result;

            for i in 1..=5 {
                let msg = format!("Frame {}", i);
                send_result(
                    host.ctx,
                    sid,
                    NrStatus::Ok,
                    NrBytes::from_slice(msg.as_bytes()),
                );
            }
            // End stream
            send_result(host.ctx, sid, NrStatus::StreamEnd, NrBytes::from_slice(&[]));
        }
    });

    NrStatus::Ok
}

unsafe fn handle_unary(
    _plugin_ctx: *mut c_void,
    sid: u64,
    req: *const NrRequest,
    _payload: NrBytes,
) -> NrStatus {
    if req.is_null() {
        return NrStatus::Invalid;
    }
    let req_ref = &*req;
    let path = match std::str::from_utf8(std::slice::from_raw_parts(
        req_ref.path.ptr,
        req_ref.path.len as usize,
    )) {
        Ok(s) => s.to_string(),
        Err(_) => return NrStatus::Invalid,
    };

    std::thread::spawn(move || {
        if let Some(host) = HOST_HANDLE.get() {
            let send_result = (*host.vtable).send_result;
            let response_string = format!("OK: {}", path);
            let response_bytes = response_string.as_bytes();
            send_result(
                host.ctx,
                sid,
                NrStatus::Ok,
                NrBytes::from_slice(response_bytes),
            );
        }
    });

    NrStatus::Ok
}

use nylon_ring::define_plugin;

unsafe fn plugin_shutdown(_plugin_ctx: *mut c_void) {
    // No cleanup needed
}

define_plugin! {
    init: plugin_init,
    shutdown: plugin_shutdown,
    entries: {
        "stream" => handle_stream,
        "unary" => handle_unary,
    }
}
