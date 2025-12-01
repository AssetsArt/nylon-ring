use nylon_ring::{define_plugin, NrBytes, NrHostExt, NrHostVTable, NrRequest, NrStatus, NrStr};
use std::ffi::c_void;
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

unsafe fn plugin_init(
    _plugin_ctx: *mut c_void,
    host_ctx: *mut c_void,
    host_vtable: *const NrHostVTable,
) -> NrStatus {
    // Get host extension using the helper function from host
    // This is safer than accessing HostContext directly
    let host_ext = if host_ctx.is_null() {
        std::ptr::null()
    } else {
        // Use the helper function from nylon_ring_host crate
        // Note: This requires linking against nylon_ring_host, which is fine for examples
        // In production, plugins should use a different mechanism or the host should
        // provide this function via a shared library
        nylon_ring_host::NylonRingHost::get_host_ext(host_ctx)
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
}

// Handlers
unsafe fn handle_stream(
    _plugin_ctx: *mut c_void,
    sid: u64,
    req: *const NrRequest,
    _payload: NrBytes,
) -> NrStatus {
    // Validate pointers
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

    thread::spawn(move || {
        if let Some(host) = HOST_HANDLE.get() {
            let send_result = (*host.vtable).send_result;

            // Streaming response: 5 frames, 1 per second
            for i in 1..=5 {
                // Store state: frame number
                if !host.ext.is_null() {
                    let frame_key = "frame_count";
                    let frame_value = i.to_string();
                    let set_state = (*host.ext).set_state;
                    set_state(
                        host.ctx,
                        sid,
                        NrStr::from_str(frame_key),
                        NrBytes::from_slice(frame_value.as_bytes()),
                    );
                }

                thread::sleep(Duration::from_secs(1));
                let msg = format!("Frame {}/5 from {}", i, path);
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

unsafe fn handle_state(
    _plugin_ctx: *mut c_void,
    sid: u64,
    req: *const NrRequest,
    _payload: NrBytes,
) -> NrStatus {
    if req.is_null() {
        return NrStatus::Invalid;
    }
    let req_ref = &*req;
    let method = match std::str::from_utf8(std::slice::from_raw_parts(
        req_ref.method.ptr,
        req_ref.method.len as usize,
    )) {
        Ok(s) => s.to_string(),
        Err(_) => return NrStatus::Invalid,
    };
    let path = match std::str::from_utf8(std::slice::from_raw_parts(
        req_ref.path.ptr,
        req_ref.path.len as usize,
    )) {
        Ok(s) => s.to_string(),
        Err(_) => return NrStatus::Invalid,
    };

    thread::spawn(move || {
        if let Some(host) = HOST_HANDLE.get() {
            let send_result = (*host.vtable).send_result;

            if !host.ext.is_null() {
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

                // Get state back to demonstrate
                let get_state = (*host.ext).get_state;
                let stored_path = get_state(host.ctx, sid, NrStr::from_str("request_path"));
                let path_str = String::from_utf8_lossy(stored_path.as_slice());
                let response_string = format!("OK: {} {} (stored: {})", method, path, path_str);
                let response_bytes = response_string.as_bytes();
                send_result(
                    host.ctx,
                    sid,
                    NrStatus::Ok,
                    NrBytes::from_slice(response_bytes),
                );
            } else {
                let response_string = format!("OK: {} {}", method, path);
                let response_bytes = response_string.as_bytes();
                send_result(
                    host.ctx,
                    sid,
                    NrStatus::Ok,
                    NrBytes::from_slice(response_bytes),
                );
            }
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
    let method = match std::str::from_utf8(std::slice::from_raw_parts(
        req_ref.method.ptr,
        req_ref.method.len as usize,
    )) {
        Ok(s) => s.to_string(),
        Err(_) => return NrStatus::Invalid,
    };
    let path = match std::str::from_utf8(std::slice::from_raw_parts(
        req_ref.path.ptr,
        req_ref.path.len as usize,
    )) {
        Ok(s) => s.to_string(),
        Err(_) => return NrStatus::Invalid,
    };

    thread::spawn(move || {
        if let Some(host) = HOST_HANDLE.get() {
            let send_result = (*host.vtable).send_result;
            thread::sleep(Duration::from_secs(2));

            let response_string = format!("OK: {} {}", method, path);
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

unsafe fn plugin_shutdown(_plugin_ctx: *mut c_void) {
    // Cleanup if needed
}

unsafe fn handle_raw_echo(_plugin_ctx: *mut c_void, sid: u64, payload: NrBytes) -> NrStatus {
    let payload_slice = payload.as_slice();
    let payload_vec = payload_slice.to_vec();

    thread::spawn(move || {
        if let Some(host) = HOST_HANDLE.get() {
            let send_result = (*host.vtable).send_result;
            thread::sleep(Duration::from_millis(100)); // Simulate some work

            send_result(
                host.ctx,
                sid,
                NrStatus::Ok,
                NrBytes::from_slice(&payload_vec),
            );
        }
    });

    NrStatus::Ok
}

unsafe fn handle_raw_stream(_plugin_ctx: *mut c_void, sid: u64, payload: NrBytes) -> NrStatus {
    let payload_slice = payload.as_slice();
    let payload_vec = payload_slice.to_vec();

    thread::spawn(move || {
        if let Some(host) = HOST_HANDLE.get() {
            let send_result = (*host.vtable).send_result;

            let payload_str = String::from_utf8_lossy(&payload_vec);

            // Stream 3 frames
            for i in 0..3 {
                thread::sleep(Duration::from_millis(50));
                let msg = format!("Stream frame {} for {}", i, payload_str);
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

unsafe fn handle_stream_data(_plugin_ctx: *mut c_void, sid: u64, data: NrBytes) -> NrStatus {
    let data_str = match std::str::from_utf8(data.as_slice()) {
        Ok(s) => s,
        Err(_) => return NrStatus::Invalid,
    };

    // Echo back with "Echo: " prefix
    let msg = format!("Echo: {}", data_str);

    if let Some(host) = HOST_HANDLE.get() {
        let send_result = (*host.vtable).send_result;
        send_result(
            host.ctx,
            sid,
            NrStatus::Ok,
            NrBytes::from_slice(msg.as_bytes()),
        );
    }

    NrStatus::Ok
}

unsafe fn handle_stream_close(_plugin_ctx: *mut c_void, sid: u64) -> NrStatus {
    if let Some(host) = HOST_HANDLE.get() {
        let send_result = (*host.vtable).send_result;
        send_result(
            host.ctx,
            sid,
            NrStatus::StreamEnd,
            NrBytes::from_slice(b"Stream closed by host"),
        );
    }
    NrStatus::Ok
}

define_plugin! {
    init: plugin_init,
    shutdown: plugin_shutdown,
    entries: {
        "stream" => handle_stream,
        "state" => handle_state,
        "unary" => handle_unary,
    },
    raw_entries: {
        "echo" => handle_raw_echo,
        "stream" => handle_raw_stream,
    },
    stream_handlers: {
        data: handle_stream_data,
        close: handle_stream_close,
    }
}
