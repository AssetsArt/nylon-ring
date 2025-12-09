//! FFI callback handlers for the plugin interface.

use crate::context::{HostContext, CURRENT_UNARY_RESULT, CURRENT_UNARY_TX};
use crate::types::{Pending, StreamFrame, UnaryResultSlot, UnarySender};
use nylon_ring::{NrBytes, NrStatus, NrStr};
use std::ffi::c_void;

/// Callback invoked by the plugin to send results back to the host.
///
/// This handles three different execution paths:
/// 1. Ultra-fast direct slot (for `call_response_fast`)
/// 2. Fast path with oneshot sender (for `call_response`)
/// 3. Stream/async path with DashMap (for `call_stream`)
///
/// # Safety
///
/// Must be called with a valid `host_ctx` pointer created by this host.
pub(crate) unsafe extern "C" fn send_result_vec_callback(
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

    // ── FAST PATH: oneshot sender (call_response) ──
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
                    NrStatus::Err | NrStatus::Invalid | NrStatus::Unsupported | NrStatus::StreamEnd
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

/// Callback for getting per-SID state from the host.
///
/// # Safety
///
/// Must be called with a valid `host_ctx` pointer created by this host.
/// The returned `NrBytes` is only valid as long as the `DashMap` entry exists.
pub(crate) unsafe extern "C" fn get_state_callback(
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
