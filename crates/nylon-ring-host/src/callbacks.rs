//! FFI callback handlers for the plugin interface.

use crate::context::{HostContext, CURRENT_UNARY_RESULT, CURRENT_UNARY_TX};
use crate::types::{StreamFrame, UnaryResultSlot, UnarySender};
use nylon_ring::{NrBytes, NrStatus, NrStr};
use std::ffi::c_void;

/// Callback invoked by the plugin to send results back to the host.
///
/// This handles three different execution paths:
/// 1. Ultra-fast direct slot (for `call_response_fast`)
/// 2. Fast path with oneshot sender (legacy optimization, mostly replaced by Slab)
/// 3. Slab/Waker path (God Mode)
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
            // For Slab architecture, if we allocated a slot, we might need to clear it?
            // Assuming call_response_fast might NOT allocate a Slab slot if it uses a special SID range?
            // Or if it DOES allocate, the caller is responsible for freeing it.
            // But here we just set the thread-local result.
            handled_fast = true;
        }
    });

    if handled_fast {
        return;
    }

    // ── FAST PATH: oneshot sender (Legacy / Thread Local Fast Path) ──
    CURRENT_UNARY_TX.with(|cell| {
        let ptr = cell.get();
        if !ptr.is_null() {
            let slot: &mut UnarySender = unsafe { &mut *ptr };

            if let Some(tx) = slot.take() {
                if let Some(data) = data_vec.take() {
                    let _ = tx.send((status, data));
                }
                handled_fast = true;
            }
        }
    });

    if handled_fast {
        return;
    }

    // ── SHARDED MAP / CHANNEL PATH ──
    let data_vec = match data_vec.take() {
        Some(v) => v,
        None => return, // Already consumed
    };

    // Try normal lookup/removal from Sharded Map
    if let Some(entry) = crate::context::remove_pending(ctx, sid) {
        match entry {
            crate::types::Pending::Unary(tx) => {
                // Oneshot: just send result
                let _ = tx.send((status, data_vec));
            }
            crate::types::Pending::Stream(tx) => {
                // Stream: send frame
                let _ = tx.send(StreamFrame {
                    status,
                    data: data_vec,
                });

                // If stream is NOT finished, we must PUT IT BACK so next callback finds it.
                let is_finished = matches!(
                    status,
                    NrStatus::Err | NrStatus::Invalid | NrStatus::Unsupported | NrStatus::StreamEnd
                );

                if !is_finished {
                    crate::context::reinsert_pending(ctx, sid, crate::types::Pending::Stream(tx));
                }
            }
        }
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

/// Callback for dispatching calls to other plugins.
///
/// # Safety
///
/// Must be called with a valid `host_ctx` pointer created by this host.
/// Dispatch (Sync): Blocks until completion.
pub(crate) unsafe extern "C" fn dispatch_sync(
    host_ctx: *mut c_void,
    target_plugin: NrStr,
    entry: NrStr,
    payload: NrBytes,
) -> nylon_ring::NrTuple<NrStatus, nylon_ring::NrVec<u8>> {
    if host_ctx.is_null() {
        return Default::default();
    }
    let ctx = &*(host_ctx as *const HostContext);
    let target = target_plugin.as_str();

    let plugin = match ctx.get_plugin(target) {
        Some(p) => p,
        None => {
            return nylon_ring::NrTuple {
                a: NrStatus::Err,
                b: Default::default(),
            }
        }
    };

    let handle_fn = match plugin.vtable.handle {
        Some(f) => f,
        None => {
            return nylon_ring::NrTuple {
                a: NrStatus::Err,
                b: Default::default(),
            }
        }
    };

    let sid = crate::sid::next_sid();
    let (tx, rx) = tokio::sync::oneshot::channel();

    // Register pending request in target plugin's HostContext (NOT the caller's)
    // The target plugin will call send_result(sid, ...) which will look up this sid.
    // KEY POINT: HostContext is shared! All plugins share the same HostContext?
    // Wait, LoadedPlugin has `host_ctx: Arc<HostContext>`.
    // NylonRingHost creates ONE HostContext and shares it with all plugins.
    // So yes, we insert into the shared HostContext.
    crate::context::insert_pending(&plugin.host_ctx, sid, crate::types::Pending::Unary(tx));

    let status = handle_fn(entry, sid, payload);

    if status != NrStatus::Ok {
        crate::context::remove_pending(&plugin.host_ctx, sid);
        return nylon_ring::NrTuple {
            a: status,
            b: Default::default(),
        };
    }

    // BLOCKING WAIT using Tokio Handle?
    // If we are in a plugin callback, we might be on a Tokio thread or a dedicated thread.
    // If we are on a Tokio worker, blocking it is generally bad, but for "dispatch_sync", it is implied.
    // However, if we block the thread, the target plugin (if scheduled on same thread) cannot run.
    // We rely on Tokio's multi-threaded runtime.

    // Use futures::executor::block_on? Or simple rx.blocking_recv() if available?
    // oneshot::Receiver doesn't have blocking_recv.
    // We can use std::sync::mpsc for this? No, `send_result` uses oneshot/mpsc from `Pending`.
    // We must use `futures::executor::block_on` or similar.

    match futures::executor::block_on(rx) {
        Ok((st, data)) => nylon_ring::NrTuple {
            a: st,
            b: nylon_ring::NrVec::from_vec(data),
        },
        Err(_) => nylon_ring::NrTuple {
            a: NrStatus::Err,
            b: Default::default(),
        },
    }
}

/// Dispatch (Fast): TLS optimization (Caller handles TLS setup/teardown? No, Host must bridge it).
/// Impl: This is tricky.
/// Plugin A calls dispatch_fast -> Host -> Plugin B.
/// Plugin B writes to TLS.
/// Host reads TLS.
/// Host returns to Plugin A.
/// Since everything is in the same process/thread (for fast path), this works.
pub(crate) unsafe extern "C" fn dispatch_fast(
    host_ctx: *mut c_void,
    target_plugin: NrStr,
    entry: NrStr,
    payload: NrBytes,
) -> nylon_ring::NrTuple<NrStatus, nylon_ring::NrVec<u8>> {
    if host_ctx.is_null() {
        return Default::default();
    }
    let ctx = &*(host_ctx as *const HostContext);
    let target = target_plugin.as_str();

    let plugin = match ctx.get_plugin(target) {
        Some(p) => p,
        None => {
            return nylon_ring::NrTuple {
                a: NrStatus::Err,
                b: Default::default(),
            }
        }
    };

    let handle_fn = match plugin.vtable.handle {
        Some(f) => f,
        None => {
            return nylon_ring::NrTuple {
                a: NrStatus::Err,
                b: Default::default(),
            }
        }
    };

    // We need to capture the result from Plugin B.
    // Plugin B expects CURRENT_UNARY_RESULT to be set.
    // But Plugin A might ALSO have set it?
    // CURRENT_UNARY_RESULT is thread-local.
    // We must SAVE the current value (if any), set ours, call B, then RESTORE.

    let sid = crate::sid::next_sid();
    let mut slot: UnaryResultSlot = None;

    let ret = CURRENT_UNARY_RESULT.with(|cell| {
        let prev = cell.get();
        cell.set(&mut slot as *mut _);

        let status = handle_fn(entry, sid, payload);

        cell.set(prev); // Restore
        status
    });

    if ret != NrStatus::Ok {
        return nylon_ring::NrTuple {
            a: ret,
            b: Default::default(),
        };
    }

    match slot {
        Some((st, data)) => nylon_ring::NrTuple {
            a: st,
            b: nylon_ring::NrVec::from_vec(data),
        },
        None => nylon_ring::NrTuple {
            a: NrStatus::Err,
            b: Default::default(),
        },
    }
}

/// Dispatch (Async): Fire and forget.
pub(crate) unsafe extern "C" fn dispatch_async(
    host_ctx: *mut c_void,
    target_plugin: NrStr,
    entry: NrStr,
    payload: NrBytes,
) -> NrStatus {
    if host_ctx.is_null() {
        return NrStatus::Err;
    }
    let ctx = &*(host_ctx as *const HostContext);
    let target = target_plugin.as_str();

    let plugin = match ctx.get_plugin(target) {
        Some(p) => p,
        None => return NrStatus::Err,
    };

    let handle_fn = match plugin.vtable.handle {
        Some(f) => f,
        None => return NrStatus::Err,
    };

    let sid = crate::sid::next_sid(); // | 0x8000...? Using normal SID is fine if loose.
                                      // If we want to guarantee no response overhead, use Fast SID flag?
                                      // Let's stick to simple SID.

    // No Pending registration needed for Fire-And-Forget?
    // If Plugin B sends result, it will be dropped if not found in map.

    handle_fn(entry, sid, payload)
}

/// Dispatch (Stream): Returns SID and setups channel.
pub(crate) unsafe extern "C" fn dispatch_stream(
    host_ctx: *mut c_void,
    target_plugin: NrStr,
    entry: NrStr,
    payload: NrBytes,
) -> nylon_ring::NrTuple<NrStatus, u64> {
    if host_ctx.is_null() {
        return nylon_ring::NrTuple {
            a: NrStatus::Err,
            b: 0,
        };
    }
    let ctx = &*(host_ctx as *const HostContext);
    let target = target_plugin.as_str();

    let plugin = match ctx.get_plugin(target) {
        Some(p) => p,
        None => {
            return nylon_ring::NrTuple {
                a: NrStatus::Err,
                b: 0,
            }
        }
    };

    let handle_fn = match plugin.vtable.handle {
        Some(f) => f,
        None => {
            return nylon_ring::NrTuple {
                a: NrStatus::Err,
                b: 0,
            }
        }
    };

    let sid = crate::sid::next_sid();
    let (tx, rx) = std::sync::mpsc::channel::<StreamFrame>();

    // We need to STORE `rx` somewhere accessible by `stream_read`.
    // Typically `Pending` stores `tx` (Sender) so the Host can WRITE to it when Plugin sends result.
    // Wait.
    // Plugin B sends data -> Host gets callback -> Host finds `tx` in Pending -> Writes to `rx`.
    // Plugin A calls `stream_read` -> Host needs `rx`.
    // So we need to store `rx` separately?
    //
    // Actually:
    // 1. insert_pending stores `tx`.
    // 2. Plugin B calls `send_result`. Host looks up `tx`, sends frame.
    // 3. `rx` receives frame.
    // 4. `stream_read` needs `rx`.
    // Where do we store `rx`?
    // We need a NEW map for "Active Streams being consumed".
    // Or we can use the `state_per_sid`? It stores `Vec<u8>`. Not strictly typed.
    // We might need a `stream_channels: DashMap<u64, UnboundedReceiver<StreamFrame>>` in HostContext.
    //
    // Let's compromise: We store `rx` in a global/static map or extend HostContext?
    // Modifying HostContext is better. But requires updating `context.rs`.

    crate::context::insert_pending(&plugin.host_ctx, sid, crate::types::Pending::Stream(tx));
    crate::context::insert_stream_receiver(ctx, sid, rx);
    crate::context::insert_stream_target(ctx, sid, plugin.clone());

    let status = handle_fn(entry, sid, payload);
    nylon_ring::NrTuple { a: status, b: sid }
}

/// Stream Read: Pulls next frame.
pub(crate) unsafe extern "C" fn stream_read(
    host_ctx: *mut c_void,
    sid: u64,
) -> nylon_ring::NrTuple<NrStatus, nylon_ring::NrVec<u8>> {
    if host_ctx.is_null() {
        return nylon_ring::NrTuple {
            a: NrStatus::Err,
            b: Default::default(),
        };
    }
    let ctx = &*(host_ctx as *const HostContext);

    // Blocking read from receiver
    if let Some(rx_guard) = crate::context::get_stream_receiver(ctx, sid) {
        match rx_guard.recv() {
            Ok(frame) => nylon_ring::NrTuple {
                a: frame.status,
                b: nylon_ring::NrVec::from_vec(frame.data),
            },
            Err(_) => nylon_ring::NrTuple {
                a: NrStatus::Err,
                b: Default::default(),
            }, // Channel closed or not found
        }
    } else {
        nylon_ring::NrTuple {
            a: NrStatus::Invalid,
            b: Default::default(),
        }
    }
}

pub(crate) unsafe extern "C" fn stream_write(
    host_ctx: *mut c_void,
    sid: u64,
    data: NrBytes,
) -> NrStatus {
    if host_ctx.is_null() {
        return NrStatus::Err;
    }
    let ctx = &*(host_ctx as *const HostContext);

    // To write to a stream, we need to know the TARGET PLUGIN for this SID.
    // But streams are usually 1-to-1?
    // If Plugin A wants to *send* data to Plugin B (e.g. streaming UPLOAD),
    // Plugin A calls `stream_write`.
    // Host must call Plugin B's `stream_data` callback.
    // We need to map `SID -> Target Plugin`.
    //
    // We can store this in `state_per_sid`? Or extended HostContext.
    // Let's assume we have `get_stream_target(ctx, sid) -> Option<Arc<LoadedPlugin>>`.

    if let Some(plugin) = crate::context::get_stream_target(ctx, sid) {
        if let Some(stream_data) = plugin.vtable.stream_data {
            stream_data(sid, data)
        } else {
            NrStatus::Unsupported
        }
    } else {
        NrStatus::Err
    }
}

pub(crate) unsafe extern "C" fn stream_close(host_ctx: *mut c_void, sid: u64) -> NrStatus {
    if host_ctx.is_null() {
        return NrStatus::Err;
    }
    let ctx = &*(host_ctx as *const HostContext);

    if let Some(plugin) = crate::context::get_stream_target(ctx, sid) {
        if let Some(stream_close) = plugin.vtable.stream_close {
            stream_close(sid)
        } else {
            NrStatus::Unsupported
        }
    } else {
        NrStatus::Err
    }
}
