//! FFI callback handlers for the plugin interface.

use crate::context::{CURRENT_UNARY_RESULT, HostContext};
use crate::types::{ForeignBytes, ResponsePayload, UnaryResultSlot};
use nylon_ring::{NrBytes, NrOwnedBytesV2, NrStatus, NrStr, NrVec};
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

    let data = data_vec.take().expect("fast path did not consume payload");
    crate::context::dispatch_pending(ctx, sid, status, ResponsePayload::Owned(data))
}

/// v2 callback: delivers a plugin-owned response without a host-side copy.
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
    payload: NrOwnedBytesV2,
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
