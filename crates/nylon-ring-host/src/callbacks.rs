//! FFI callback handlers for the plugin interface.

use crate::context::{CURRENT_UNARY_RESULT, HostContext};
use crate::types::{ForeignBytes, ResponsePayload, UnaryResultSlot};
use nylon_ring::{NrBufferLease, NrBytes, NrOwnedBytes, NrStatus, NrStr, NrVec};
use std::ffi::c_void;

/// Callback invoked by the plugin to send results back to the host.
///
/// This handles two execution paths:
/// 1. Ultra-fast direct slot (for `call_response_fast`)
/// 2. Sharded pending-request map (unary and streaming calls)
///
/// # Safety
///
/// Must be called with a valid `host_ctx` pointer created by this host.
pub(crate) unsafe extern "C" fn send_result_vec_callback(
    host_ctx: *mut c_void,
    sid: u64,
    status: NrStatus,
    payload: nylon_ring::NrVec<u8>,
) -> NrStatus {
    if host_ctx.is_null() {
        return NrStatus::Invalid;
    }
    let ctx = unsafe { &*host_ctx.cast::<HostContext>() };

    // Convert NrVec to Vec<u8>
    let mut data_vec = Some(payload.into_vec());

    // ── ULTRA FAST DIRECT SLOT (call_response_fast) ──
    let mut handled_fast = false;

    CURRENT_UNARY_RESULT.with(|cell| {
        let ptr = cell.get();
        if !ptr.is_null() {
            let slot: &mut UnaryResultSlot = unsafe { &mut *ptr };

            if slot.sid == sid
                && let Some(data) = data_vec.take()
            {
                slot.result = Some((status, data));
                handled_fast = true;
            }
        }
    });

    if handled_fast {
        ctx.remove_state(sid);
        return NrStatus::Ok;
    }

    // ── DIRECT STREAM SLOT (frames sent inside the same `handle` call) ──
    let mut stream_status = None;
    crate::context::CURRENT_STREAM_FRAME.with(|cell| {
        let ptr = cell.get();
        if !ptr.is_null() {
            let slot: &mut crate::context::StreamFrameSlot = unsafe { &mut *ptr };
            if slot.sid == sid
                && let Some(data) = data_vec.take()
            {
                let terminal = status.is_terminal();
                match slot
                    .chan
                    .try_send(crate::types::StreamFrame { status, data })
                {
                    Ok(()) => {
                        if terminal {
                            slot.terminal_seen = true;
                        }
                        stream_status = Some(NrStatus::Ok);
                    }
                    Err(_rejected_frame) => stream_status = Some(NrStatus::Backpressure),
                }
            }
        }
    });
    if let Some(status) = stream_status {
        return status;
    }

    let data = data_vec.take().expect("fast path did not consume payload");
    crate::context::dispatch_pending(ctx, sid, status, ResponsePayload::Owned(data))
}

/// Delivers a plugin-owned response without a host-side copy.
///
/// The host takes ownership of `payload` and calls its release exactly once
/// (on delivery-path drop, or when the eventual consumer drops the response
/// view). The fast-path slot and stream frames still copy, because their
/// results are host-owned `Vec`s; standard unary requests store the foreign
/// buffer as-is.
///
/// # Safety
///
/// Must be called with a valid `host_ctx` pointer created by this host.
pub(crate) unsafe extern "C" fn send_result_owned_callback(
    host_ctx: *mut c_void,
    sid: u64,
    status: NrStatus,
    payload: NrOwnedBytes,
) -> NrStatus {
    let foreign = ForeignBytes::from_abi(payload);
    if host_ctx.is_null() {
        return NrStatus::Invalid;
    }
    let ctx = unsafe { &*host_ctx.cast::<HostContext>() };

    let mut payload = Some(ResponsePayload::Foreign(foreign));
    let mut handled_fast = false;
    CURRENT_UNARY_RESULT.with(|cell| {
        let ptr = cell.get();
        if !ptr.is_null() {
            let slot: &mut UnaryResultSlot = unsafe { &mut *ptr };
            if slot.sid == sid
                && let Some(payload) = payload.take()
            {
                // The fast-path slot hands out a Vec; copying here is safe
                // because this callback runs inside the plugin's handle call.
                slot.result = Some((status, payload.into_vec()));
                handled_fast = true;
            }
        }
    });

    if handled_fast {
        ctx.remove_state(sid);
        return NrStatus::Ok;
    }

    let payload = payload.take().expect("fast path did not consume payload");
    crate::context::dispatch_pending(ctx, sid, status, payload)
}

/// Leases a host-owned buffer for the response to `sid`.
///
/// Serves the fast-path TLS slot first (same-thread synchronous calls),
/// then the pending-request map. Fails with a null lease for unknown sids,
/// streaming sids, and sids that already hold an outstanding lease.
///
/// # Safety
///
/// Must be called with a valid `host_ctx` pointer created by this host.
pub(crate) unsafe extern "C" fn acquire_result_buffer_callback(
    host_ctx: *mut c_void,
    sid: u64,
    capacity: u64,
) -> NrBufferLease {
    if host_ctx.is_null() {
        return NrBufferLease::failed();
    }
    let ctx = unsafe { &*host_ctx.cast::<HostContext>() };

    let mut fast_lease = None;
    CURRENT_UNARY_RESULT.with(|cell| {
        let ptr = cell.get();
        if !ptr.is_null() {
            let slot: &mut UnaryResultSlot = unsafe { &mut *ptr };
            if slot.sid == sid {
                if slot.lease.is_none() && slot.result.is_none() {
                    let Ok(capacity) = usize::try_from(capacity) else {
                        fast_lease = Some(NrBufferLease::failed());
                        return;
                    };
                    let mut buffer: Vec<u8> = Vec::with_capacity(capacity);
                    fast_lease = Some(NrBufferLease {
                        ptr: buffer.as_mut_ptr(),
                        cap: buffer.capacity() as u64,
                        token: buffer.as_ptr() as u64,
                    });
                    slot.lease = Some(buffer);
                } else {
                    fast_lease = Some(NrBufferLease::failed());
                }
            }
        }
    });
    if let Some(lease) = fast_lease {
        return lease;
    }

    crate::context::acquire_pending_lease(ctx, sid, capacity)
}

/// Commits a leased buffer as the response to `sid`.
///
/// # Safety
///
/// Must be called with a valid `host_ctx` pointer created by this host, and
/// the plugin must have initialized the first `initialized_len` bytes of the
/// leased buffer (ABI contract).
pub(crate) unsafe extern "C" fn commit_result_buffer_callback(
    host_ctx: *mut c_void,
    sid: u64,
    status: NrStatus,
    token: u64,
    initialized_len: u64,
) -> NrStatus {
    if host_ctx.is_null() {
        return NrStatus::Invalid;
    }
    let ctx = unsafe { &*host_ctx.cast::<HostContext>() };

    let mut fast_status = None;
    CURRENT_UNARY_RESULT.with(|cell| {
        let ptr = cell.get();
        if !ptr.is_null() {
            let slot: &mut UnaryResultSlot = unsafe { &mut *ptr };
            if slot.sid == sid {
                let Some(mut buffer) = slot.lease.take() else {
                    fast_status = Some(NrStatus::Invalid);
                    return;
                };
                if buffer.as_ptr() as u64 != token || initialized_len > buffer.capacity() as u64 {
                    slot.lease = Some(buffer);
                    fast_status = Some(NrStatus::Invalid);
                    return;
                }
                if slot.result.is_some() {
                    // The plugin already responded another way; its commit
                    // promises no further writes, so the lease can be freed.
                    fast_status = Some(NrStatus::Invalid);
                    return;
                }
                // SAFETY: length is within capacity and the plugin's commit
                // asserts it wrote that prefix (ABI contract).
                unsafe { buffer.set_len(initialized_len as usize) };
                slot.result = Some((status, buffer));
                fast_status = Some(NrStatus::Ok);
            }
        }
    });
    if let Some(status) = fast_status {
        if status == NrStatus::Ok {
            ctx.remove_state(sid);
        }
        return status;
    }

    crate::context::commit_pending_lease(ctx, sid, status, token, initialized_len)
}

/// Callback for setting per-SID state in the host.
///
/// # Safety
///
/// Must be called with a valid `host_ctx` pointer created by this host.
pub(crate) unsafe extern "C" fn set_state_callback(
    host_ctx: *mut c_void,
    sid: u64,
    key: NrStr,
    value: NrBytes,
) -> NrStatus {
    if host_ctx.is_null() {
        return NrStatus::Invalid;
    }
    let ctx = unsafe { &*host_ctx.cast::<HostContext>() };

    let key_str = match unsafe { key.as_str() } {
        Ok(key) => key.to_owned(),
        Err(_) => return NrStatus::Invalid,
    };

    // Copy data from NrBytes to owned Vec<u8>
    let value_vec = match unsafe { value.as_slice() } {
        Ok(value) => value.to_vec(),
        Err(_) => return NrStatus::Invalid,
    };

    ctx.set_state(sid, key_str, value_vec);

    NrStatus::Ok
}

/// Callback for getting per-SID state from the host.
///
/// # Safety
///
/// Must be called with a valid `host_ctx` pointer created by this host.
/// The returned vector owns a copy of the stored bytes.
pub(crate) unsafe extern "C" fn get_state_callback(
    host_ctx: *mut c_void,
    sid: u64,
    key: NrStr,
) -> NrVec<u8> {
    if host_ctx.is_null() {
        return NrVec::default();
    }
    let ctx = unsafe { &*host_ctx.cast::<HostContext>() };

    let key_str = match unsafe { key.as_str() } {
        Ok(key) => key,
        Err(_) => return NrVec::default(),
    };
    if let Some(sid_state) = ctx.state_per_sid.get(&sid)
        && let Some(value) = sid_state.get(key_str)
    {
        return NrVec::from_vec(value.clone());
    }

    NrVec::default()
}
