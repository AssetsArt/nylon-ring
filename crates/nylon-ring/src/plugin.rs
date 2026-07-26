//! Trait-based plugin API, inspired by
//! [Pingora's `ProxyHttp`/`CTX` design](https://github.com/cloudflare/pingora/blob/main/docs/user_guide/ctx.md):
//! implement [`Plugin`] on a struct and export it with [`export_plugin!`](crate::export_plugin) —
//! no raw pointers, no `unsafe` handlers, panics contained at the ABI
//! boundary.
//!
//! Two kinds of state, exactly like Pingora:
//!
//! - **Per-call state** lives in [`Plugin::Ctx`], created fresh by
//!   [`Plugin::new_ctx`] for every incoming call and dropped when the call
//!   ends.
//! - **Cross-call state** lives on the plugin struct itself: one instance
//!   serves every call concurrently, so plain fields with `Atomic*`,
//!   `Mutex`, or anything `Sync` work like Pingora's service struct.
//!
//! ```no_run
//! use nylon_ring::{export_plugin, NrStatus, Plugin, Reply, Session};
//! use std::sync::atomic::{AtomicU64, Ordering};
//!
//! struct Demo {
//!     // cross-call state, shared by every call
//!     calls: AtomicU64,
//! }
//!
//! struct DemoCtx {
//!     // per-call state (Pingora-style CTX)
//!     uppercase: bool,
//! }
//!
//! impl Plugin for Demo {
//!     type Ctx = DemoCtx;
//!     const ENTRIES: &'static [&'static str] = &["echo", "shout"];
//!
//!     fn new() -> Self {
//!         Demo { calls: AtomicU64::new(0) }
//!     }
//!
//!     fn new_ctx(&self) -> DemoCtx {
//!         DemoCtx { uppercase: false }
//!     }
//!
//!     fn on_call(&self, session: &mut Session<'_>, ctx: &mut DemoCtx) -> Reply {
//!         self.calls.fetch_add(1, Ordering::Relaxed);
//!         match session.entry() {
//!             "echo" => Reply::Bytes(session.payload().to_vec()),
//!             "shout" => {
//!                 ctx.uppercase = true;
//!                 let text = String::from_utf8_lossy(session.payload()).to_string();
//!                 Reply::Text(if ctx.uppercase { text.to_uppercase() } else { text })
//!             }
//!             _ => Reply::Fail(NrStatus::Invalid),
//!         }
//!     }
//! }
//!
//! export_plugin!(Demo);
//! ```
//!
//! Entries listed in [`Plugin::ASYNC_ENTRIES`] dispatch to
//! [`Plugin::on_async_call`] instead — a native `async` handler (no
//! `async_trait` crate needed). The ABI callback returns immediately and
//! the reply is delivered when the future completes, through the host's
//! cross-thread async path. Futures run via [`Plugin::spawn_async`]:
//! dependency-free thread-per-call by default, or override it with
//! `tokio::spawn` (or any executor) for real workloads.
//!
//! The raw `define_plugin!` macro remains available for handlers that need
//! full control over the ABI (owned responses, buffer leases); the safe
//! host-call helpers in this module ([`send_result`], [`send_result_owned`],
//! [`acquire_result_buffer`], [`commit_result_buffer`]) can be mixed into
//! either style.

use crate::{
    NR_ENTRY_UNKNOWN, NrBufferLease, NrBytes, NrHostVTable, NrOwnedBytes, NrStatus, NrStr, NrVec,
};
use std::ffi::c_void;
use std::future::Future;
use std::sync::atomic::{AtomicPtr, Ordering};

static HOST_CTX: AtomicPtr<c_void> = AtomicPtr::new(std::ptr::null_mut());
static HOST_VTABLE: AtomicPtr<NrHostVTable> = AtomicPtr::new(std::ptr::null_mut());

fn host() -> Option<(*mut c_void, &'static NrHostVTable)> {
    let ctx = HOST_CTX.load(Ordering::Acquire);
    let vtable = HOST_VTABLE.load(Ordering::Acquire);
    if ctx.is_null() || vtable.is_null() {
        return None;
    }
    // SAFETY: the host guarantees the vtable outlives the loaded plugin,
    // and init stored a non-null pointer to it.
    Some((ctx, unsafe { &*vtable }))
}

/// Sends a response or stream frame for `sid` through the host's
/// `send_result`. Borrowed payloads (`owned = 0`) are copied by the host
/// before this returns. Reports `Invalid` when the plugin has not been
/// initialized.
pub fn send_result(sid: u64, status: NrStatus, data: NrVec<u8>) -> NrStatus {
    let Some((ctx, vtable)) = host() else {
        return NrStatus::Invalid;
    };
    // SAFETY: ctx/vtable come from the host's init call and stay valid for
    // the lifetime of the loaded plugin.
    unsafe { (vtable.send_result)(ctx, sid, status, data) }
}

/// Sends a response the host consumes without copying; the host takes
/// ownership and calls the payload's release exactly once.
pub fn send_result_owned(sid: u64, status: NrStatus, data: NrOwnedBytes) -> NrStatus {
    let Some((ctx, vtable)) = host() else {
        return NrStatus::Invalid;
    };
    // SAFETY: as in `send_result`.
    unsafe { (vtable.send_result_owned)(ctx, sid, status, data) }
}

/// Leases a host-owned response buffer for `sid`; check
/// [`NrBufferLease::is_failed`] before writing.
pub fn acquire_result_buffer(sid: u64, capacity: u64) -> NrBufferLease {
    let Some((ctx, vtable)) = host() else {
        return NrBufferLease::failed();
    };
    // SAFETY: as in `send_result`.
    unsafe { (vtable.acquire_result_buffer)(ctx, sid, capacity) }
}

/// Commits a leased buffer as the response for `sid`.
pub fn commit_result_buffer(
    sid: u64,
    status: NrStatus,
    token: u64,
    initialized_len: u64,
) -> NrStatus {
    let Some((ctx, vtable)) = host() else {
        return NrStatus::Invalid;
    };
    // SAFETY: as in `send_result`.
    unsafe { (vtable.commit_result_buffer)(ctx, sid, status, token, initialized_len) }
}

/// One incoming call: the entry it targets, its payload, and the session id
/// for manual (streaming) replies.
pub struct Session<'a> {
    sid: u64,
    entry_id: u32,
    entry: &'static str,
    payload: &'a [u8],
}

impl Session<'_> {
    /// The session id the host assigned to this call.
    pub fn sid(&self) -> u64 {
        self.sid
    }

    /// The entry name this call targets.
    pub fn entry(&self) -> &'static str {
        self.entry
    }

    /// The entry's dispatch id (its index in [`Plugin::ENTRIES`]).
    pub fn entry_id(&self) -> u32 {
        self.entry_id
    }

    /// The request payload.
    pub fn payload(&self) -> &[u8] {
        self.payload
    }

    /// Sends one stream data frame; finish with [`Session::end_stream`]
    /// and return [`Reply::None`] from the handler.
    pub fn send_frame(&mut self, data: Vec<u8>) -> NrStatus {
        send_result(self.sid, NrStatus::Ok, NrVec::from_vec(data))
    }

    /// Ends a stream with a final frame.
    pub fn end_stream(&mut self, data: Vec<u8>) -> NrStatus {
        send_result(self.sid, NrStatus::StreamEnd, NrVec::from_vec(data))
    }
}

/// What a [`Plugin::on_call`] handler answers with.
pub enum Reply {
    /// Respond with these bytes; ownership transfers to the host zero-copy.
    Bytes(Vec<u8>),
    /// Respond with this text; ownership transfers to the host zero-copy.
    Text(String),
    /// Respond zero-copy from a static buffer — nothing to allocate or free.
    Static(&'static [u8]),
    /// Send no response: fire-and-forget entries, or streams that already
    /// finished through [`Session::end_stream`].
    None,
    /// Fail the call with this status.
    Fail(NrStatus),
}

/// One incoming async call: like [`Session`] but self-contained — the
/// payload is copied out of host memory so the future may outlive the
/// ABI callback that created it.
pub struct AsyncSession {
    sid: u64,
    entry_id: u32,
    entry: &'static str,
    payload: Vec<u8>,
}

impl AsyncSession {
    /// The session id the host assigned to this call.
    pub fn sid(&self) -> u64 {
        self.sid
    }

    /// The entry name this call targets.
    pub fn entry(&self) -> &'static str {
        self.entry
    }

    /// The entry's dispatch id (its index in the combined
    /// [`Plugin::ENTRIES`] + [`Plugin::ASYNC_ENTRIES`] list).
    pub fn entry_id(&self) -> u32 {
        self.entry_id
    }

    /// The request payload.
    pub fn payload(&self) -> &[u8] {
        &self.payload
    }

    /// Takes the payload without copying it again.
    pub fn into_payload(self) -> Vec<u8> {
        self.payload
    }

    /// Sends one stream data frame; safe from any thread.
    pub fn send_frame(&self, data: Vec<u8>) -> NrStatus {
        send_result(self.sid, NrStatus::Ok, NrVec::from_vec(data))
    }

    /// Ends a stream with a final frame.
    pub fn end_stream(&self, data: Vec<u8>) -> NrStatus {
        send_result(self.sid, NrStatus::StreamEnd, NrVec::from_vec(data))
    }
}

/// A boxed future handed to [`Plugin::spawn_async`]; drive it to completion
/// to deliver the call's reply.
pub type AsyncTask = std::pin::Pin<Box<dyn Future<Output = ()> + Send + 'static>>;

/// A nylon-ring plugin. Implement this and export it with
/// [`export_plugin!`](crate::export_plugin); see the [module docs](self) for the state model.
pub trait Plugin: Send + Sync + 'static {
    /// Per-call state, created fresh for every incoming call and dropped
    /// when the call ends. Use `()` when no per-call state is needed.
    /// (`'static` so async handlers can carry it across await points.)
    type Ctx: Send + 'static;

    /// Entry names dispatched to [`Plugin::on_call`]. An entry's dispatch
    /// id is its index here, so appending keeps existing ids stable.
    const ENTRIES: &'static [&'static str];

    /// Entry names dispatched to [`Plugin::on_async_call`]. Their dispatch
    /// ids continue after [`Plugin::ENTRIES`] (a name in both lists
    /// dispatches synchronously).
    const ASYNC_ENTRIES: &'static [&'static str] = &[];

    /// Constructs the plugin instance; runs once when the host initializes
    /// the library.
    fn new() -> Self
    where
        Self: Sized;

    /// Creates the per-call context.
    fn new_ctx(&self) -> Self::Ctx;

    /// Handles one call. Runs on a host thread; may be called concurrently
    /// from many threads, which is why it takes `&self`.
    fn on_call(&self, session: &mut Session<'_>, ctx: &mut Self::Ctx) -> Reply;

    /// Handles one [`ASYNC_ENTRIES`](Plugin::ASYNC_ENTRIES) call. The ABI
    /// callback returns `Ok` immediately; the reply is delivered whenever
    /// the returned future completes, from whatever thread runs it — the
    /// host's standard async path supports this deferred, cross-thread
    /// reply natively. Implement with plain `async fn` syntax; no
    /// `async_trait` crate is needed.
    ///
    /// [`Reply::Fail`] is sent to the host as a result (so an awaiting
    /// caller fails instead of hanging); [`Reply::None`] sends nothing.
    fn on_async_call(
        &self,
        session: AsyncSession,
        ctx: Self::Ctx,
    ) -> impl Future<Output = Reply> + Send {
        let _ = (session, ctx);
        async { Reply::Fail(NrStatus::Unsupported) }
    }

    /// Runs an async handler's task. Only futures that are still pending
    /// after their first poll arrive here — ready-on-first-poll handlers
    /// deliver inline on the host thread and never touch the executor.
    /// The default drives the task on a dedicated thread with a
    /// park/unpark waker — correct, dependency-free, and fine for
    /// occasional calls. Override with a real executor for throughput,
    /// e.g. `tokio::spawn(task);` on a runtime the plugin owns (stop it in
    /// [`Plugin::on_shutdown`]).
    fn spawn_async(&self, task: AsyncTask) {
        std::thread::spawn(move || block_on(task));
    }

    /// Data the host pushes into a plugin-consumed stream.
    fn on_stream_data(&self, _sid: u64, _data: &[u8]) -> NrStatus {
        NrStatus::Unsupported
    }

    /// The host closed a plugin-consumed stream.
    fn on_stream_close(&self, _sid: u64) -> NrStatus {
        NrStatus::Unsupported
    }

    /// Runs when the host shuts the plugin down; stop worker threads here.
    fn on_shutdown(&self) {}
}

/// Minimal executor for the default [`Plugin::spawn_async`]: parks the
/// thread between polls.
fn block_on(mut task: AsyncTask) {
    struct ThreadWaker(std::thread::Thread);
    impl std::task::Wake for ThreadWaker {
        fn wake(self: std::sync::Arc<Self>) {
            self.0.unpark();
        }
    }
    let waker = std::task::Waker::from(std::sync::Arc::new(ThreadWaker(std::thread::current())));
    let mut cx = std::task::Context::from_waker(&waker);
    while task.as_mut().poll(&mut cx).is_pending() {
        std::thread::park();
    }
}

/// ABI glue behind [`export_plugin!`](crate::export_plugin); not part of the public API.
#[doc(hidden)]
pub mod __glue {
    use super::*;
    use std::panic::{AssertUnwindSafe, catch_unwind};
    use std::sync::OnceLock;

    pub fn init<P: Plugin>(
        instance: &'static OnceLock<P>,
        host_ctx: *mut c_void,
        host_vtable: *const NrHostVTable,
    ) -> NrStatus {
        catch_unwind(AssertUnwindSafe(|| {
            if host_ctx.is_null() || host_vtable.is_null() {
                return NrStatus::Invalid;
            }
            HOST_CTX.store(host_ctx, Ordering::Release);
            HOST_VTABLE.store(host_vtable.cast_mut(), Ordering::Release);
            instance.get_or_init(P::new);
            NrStatus::Ok
        }))
        .unwrap_or(NrStatus::Panic)
    }

    pub fn shutdown<P: Plugin>(instance: &'static OnceLock<P>) {
        let _ = catch_unwind(AssertUnwindSafe(|| {
            if let Some(plugin) = instance.get() {
                plugin.on_shutdown();
            }
            HOST_VTABLE.store(std::ptr::null_mut(), Ordering::Release);
            HOST_CTX.store(std::ptr::null_mut(), Ordering::Release);
        }));
    }

    /// Dispatch id in the combined `ENTRIES` + `ASYNC_ENTRIES` space.
    fn entry_index<P: Plugin>(entry_bytes: &[u8]) -> Option<u32> {
        if let Some(id) = P::ENTRIES
            .iter()
            .position(|name| name.as_bytes() == entry_bytes)
        {
            return Some(id as u32);
        }
        P::ASYNC_ENTRIES
            .iter()
            .position(|name| name.as_bytes() == entry_bytes)
            .map(|index| (P::ENTRIES.len() + index) as u32)
    }

    pub fn resolve_entry<P: Plugin>(entry: NrStr) -> u32 {
        catch_unwind(AssertUnwindSafe(|| {
            // SAFETY: the host keeps the entry name valid for this call.
            let entry_bytes = match unsafe { entry.as_bytes() } {
                Ok(bytes) => bytes,
                Err(_) => return NR_ENTRY_UNKNOWN,
            };
            entry_index::<P>(entry_bytes).unwrap_or(NR_ENTRY_UNKNOWN)
        }))
        .unwrap_or(NR_ENTRY_UNKNOWN)
    }

    /// # Safety
    /// `entry` and `payload` must be valid for the duration of this call,
    /// which the host guarantees for vtable callbacks.
    pub unsafe fn handle<P: Plugin>(
        instance: &'static OnceLock<P>,
        entry: NrStr,
        sid: u64,
        payload: NrBytes,
    ) -> NrStatus {
        catch_unwind(AssertUnwindSafe(|| {
            // SAFETY: per this function's contract.
            let entry_bytes = match unsafe { entry.as_bytes() } {
                Ok(bytes) => bytes,
                Err(_) => return NrStatus::Invalid,
            };
            let Some(id) = entry_index::<P>(entry_bytes) else {
                return NrStatus::Invalid;
            };
            // SAFETY: per this function's contract.
            unsafe { dispatch::<P>(instance, id, sid, payload) }
        }))
        .unwrap_or(NrStatus::Panic)
    }

    /// # Safety
    /// As for [`handle`].
    pub unsafe fn handle_by_id<P: Plugin>(
        instance: &'static OnceLock<P>,
        id: u32,
        sid: u64,
        payload: NrBytes,
    ) -> NrStatus {
        catch_unwind(AssertUnwindSafe(|| {
            if id as usize >= P::ENTRIES.len() + P::ASYNC_ENTRIES.len() {
                return NrStatus::Invalid;
            }
            // SAFETY: per this function's contract.
            unsafe { dispatch::<P>(instance, id, sid, payload) }
        }))
        .unwrap_or(NrStatus::Panic)
    }

    /// Sends a completed reply for `sid`; shared by the sync and async
    /// paths (the sync path returns the resulting status to the host).
    fn deliver(sid: u64, reply: Reply) -> NrStatus {
        match reply {
            Reply::Bytes(data) => send_result(sid, NrStatus::Ok, NrVec::from_vec(data)),
            Reply::Text(text) => send_result(sid, NrStatus::Ok, NrVec::from_string(text)),
            Reply::Static(data) => {
                send_result_owned(sid, NrStatus::Ok, NrOwnedBytes::from_static(data))
            }
            Reply::None => NrStatus::Ok,
            Reply::Fail(status) => status,
        }
    }

    /// # Safety
    /// As for [`handle`].
    unsafe fn dispatch<P: Plugin>(
        instance: &'static OnceLock<P>,
        id: u32,
        sid: u64,
        payload: NrBytes,
    ) -> NrStatus {
        let Some(plugin) = instance.get() else {
            return NrStatus::Invalid;
        };
        // SAFETY: per this function's contract.
        let payload = match unsafe { payload.as_slice() } {
            Ok(payload) => payload,
            Err(_) => return NrStatus::Invalid,
        };
        let sync_count = P::ENTRIES.len() as u32;
        if id < sync_count {
            let mut session = Session {
                sid,
                entry_id: id,
                entry: P::ENTRIES[id as usize],
                payload,
            };
            let mut ctx = plugin.new_ctx();
            return deliver(sid, plugin.on_call(&mut session, &mut ctx));
        }

        // Async entry: copy the payload out of host memory, hand the future
        // to the plugin's executor, and report Ok now — the host's standard
        // async path accepts the reply later from any thread.
        let session = AsyncSession {
            sid,
            entry_id: id,
            entry: P::ASYNC_ENTRIES[(id - sync_count) as usize],
            payload: payload.to_vec(),
        };
        let ctx = plugin.new_ctx();
        let handler = plugin.on_async_call(session, ctx);
        let mut task: AsyncTask = Box::pin(async move {
            match handler.await {
                // A failed async call must still answer the awaiting host.
                Reply::Fail(status) => {
                    let _ = send_result(sid, status, NrVec::from_vec(Vec::new()));
                }
                reply => {
                    let _ = deliver(sid, reply);
                }
            }
        });
        // Fast path: a future that is ready on its first poll (no real
        // await) delivers inline on the host thread and never touches the
        // executor. A Pending future moves to spawn_async, whose first real
        // poll re-registers a live waker per the Future contract.
        let mut poll_ctx = std::task::Context::from_waker(std::task::Waker::noop());
        if task.as_mut().poll(&mut poll_ctx).is_ready() {
            return NrStatus::Ok;
        }
        plugin.spawn_async(task);
        NrStatus::Ok
    }

    /// # Safety
    /// As for [`handle`].
    pub unsafe fn stream_data<P: Plugin>(
        instance: &'static OnceLock<P>,
        sid: u64,
        data: NrBytes,
    ) -> NrStatus {
        catch_unwind(AssertUnwindSafe(|| {
            let Some(plugin) = instance.get() else {
                return NrStatus::Invalid;
            };
            // SAFETY: per this function's contract.
            let data = match unsafe { data.as_slice() } {
                Ok(data) => data,
                Err(_) => return NrStatus::Invalid,
            };
            plugin.on_stream_data(sid, data)
        }))
        .unwrap_or(NrStatus::Panic)
    }

    pub fn stream_close<P: Plugin>(instance: &'static OnceLock<P>, sid: u64) -> NrStatus {
        catch_unwind(AssertUnwindSafe(|| {
            let Some(plugin) = instance.get() else {
                return NrStatus::Invalid;
            };
            plugin.on_stream_close(sid)
        }))
        .unwrap_or(NrStatus::Panic)
    }
}

/// Exports a [`Plugin`] implementation as this cdylib's nylon-ring entry
/// point. Use exactly once per plugin crate; see the [module docs](self)
/// for a complete example.
#[macro_export]
macro_rules! export_plugin {
    ($ty:ty) => {
        static NY_PLUGIN_INSTANCE: std::sync::OnceLock<$ty> = std::sync::OnceLock::new();

        static PLUGIN_VTABLE: $crate::NrPluginVTable = $crate::NrPluginVTable {
            init: Some(ny_plugin_init_wrapper),
            handle: Some(ny_plugin_handle_wrapper),
            shutdown: Some(ny_plugin_shutdown_wrapper),
            stream_data: Some(ny_plugin_stream_data_wrapper),
            stream_close: Some(ny_plugin_stream_close_wrapper),
            resolve_entry: Some(ny_plugin_resolve_entry_wrapper),
            handle_by_id: Some(ny_plugin_handle_by_id_wrapper),
        };

        static PLUGIN_INFO: $crate::NrPluginInfo = $crate::NrPluginInfo {
            abi_version: $crate::ABI_VERSION,
            struct_size: std::mem::size_of::<$crate::NrPluginInfo>() as u32,
            name: $crate::NrStr::from_static(env!("CARGO_PKG_NAME")),
            version: $crate::NrStr::from_static(env!("CARGO_PKG_VERSION")),
            plugin_ctx: std::ptr::null_mut(),
            vtable: &PLUGIN_VTABLE,
        };

        #[unsafe(no_mangle)]
        pub extern "C" fn nylon_ring_get_plugin() -> *const $crate::NrPluginInfo {
            &PLUGIN_INFO
        }

        unsafe extern "C" fn ny_plugin_init_wrapper(
            host_ctx: *mut std::ffi::c_void,
            host_vtable: *const $crate::NrHostVTable,
        ) -> $crate::NrStatus {
            $crate::plugin::__glue::init::<$ty>(&NY_PLUGIN_INSTANCE, host_ctx, host_vtable)
        }

        unsafe extern "C" fn ny_plugin_shutdown_wrapper() {
            $crate::plugin::__glue::shutdown::<$ty>(&NY_PLUGIN_INSTANCE);
        }

        unsafe extern "C" fn ny_plugin_handle_wrapper(
            entry: $crate::NrStr,
            sid: u64,
            payload: $crate::NrBytes,
        ) -> $crate::NrStatus {
            // SAFETY: the host keeps entry/payload valid for this callback.
            unsafe {
                $crate::plugin::__glue::handle::<$ty>(&NY_PLUGIN_INSTANCE, entry, sid, payload)
            }
        }

        unsafe extern "C" fn ny_plugin_handle_by_id_wrapper(
            id: u32,
            sid: u64,
            payload: $crate::NrBytes,
        ) -> $crate::NrStatus {
            // SAFETY: the host keeps the payload valid for this callback.
            unsafe {
                $crate::plugin::__glue::handle_by_id::<$ty>(&NY_PLUGIN_INSTANCE, id, sid, payload)
            }
        }

        unsafe extern "C" fn ny_plugin_resolve_entry_wrapper(entry: $crate::NrStr) -> u32 {
            $crate::plugin::__glue::resolve_entry::<$ty>(entry)
        }

        unsafe extern "C" fn ny_plugin_stream_data_wrapper(
            sid: u64,
            data: $crate::NrBytes,
        ) -> $crate::NrStatus {
            // SAFETY: the host keeps the data valid for this callback.
            unsafe { $crate::plugin::__glue::stream_data::<$ty>(&NY_PLUGIN_INSTANCE, sid, data) }
        }

        unsafe extern "C" fn ny_plugin_stream_close_wrapper(sid: u64) -> $crate::NrStatus {
            $crate::plugin::__glue::stream_close::<$ty>(&NY_PLUGIN_INSTANCE, sid)
        }
    };
}
