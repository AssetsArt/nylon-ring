use nylon_ring::{NrBytes, NrHostVTable, NrRequest, NrStatus};
use std::ffi::c_void;

static mut HOST_CTX: *mut c_void = std::ptr::null_mut();
static mut HOST_VTABLE: *const NrHostVTable = std::ptr::null();

unsafe fn plugin_init(
    _plugin_ctx: *mut c_void,
    host_ctx: *mut c_void,
    host_vtable: *const NrHostVTable,
) -> NrStatus {
    unsafe {
        HOST_CTX = host_ctx;
        HOST_VTABLE = host_vtable;
    }
    NrStatus::Ok
}

#[inline(always)]
pub fn send_result(sid: u64, status: NrStatus, data: &[u8]) {
    unsafe {
        let f = (*HOST_VTABLE).send_result;
        f(HOST_CTX, sid, status, NrBytes::from_slice(data));
    }
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

    for i in 1..=5 {
        let msg = format!("Frame {}", i);
        send_result(sid, NrStatus::Ok, msg.as_bytes());
    }
    // End stream
    send_result(sid, NrStatus::StreamEnd, &[]);

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

    let response_string = format!("OK: {}", path);
    let response_bytes = response_string.as_bytes();
    send_result(sid, NrStatus::Ok, response_bytes);

    NrStatus::Ok
}

use nylon_ring::define_plugin;

unsafe fn plugin_shutdown(_plugin_ctx: *mut c_void) {
    // No cleanup needed
}

unsafe fn handle_raw_echo(_plugin_ctx: *mut c_void, sid: u64, payload: NrBytes) -> NrStatus {
    send_result(sid, NrStatus::Ok, payload.as_slice());
    NrStatus::Ok
}

define_plugin! {
    init: plugin_init,
    shutdown: plugin_shutdown,
    entries: {
        "stream" => handle_stream,
        "unary" => handle_unary,
    },
    raw_entries: {
        "echo" => handle_raw_echo,
    }
}
