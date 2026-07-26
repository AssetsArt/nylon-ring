//! The trait-based plugin API in action: shared state on the struct,
//! per-call state in `Ctx`, and no `unsafe` anywhere in the plugin.

use nylon_ring::{AsyncSession, NrStatus, Plugin, Reply, Session, export_plugin};
use std::sync::atomic::{AtomicU64, Ordering};

/// Cross-call state: one instance serves every call, so plain fields with
/// atomics (or `Mutex`) are shared exactly like Pingora's service struct.
struct Demo {
    calls: AtomicU64,
}

/// Per-call state (Pingora-style CTX): created fresh for each call and
/// dropped when the call ends.
struct DemoCtx {
    uppercase: bool,
}

impl Plugin for Demo {
    type Ctx = DemoCtx;
    const ENTRIES: &'static [&'static str] = &["echo", "shout", "count", "notify", "stream"];
    const ASYNC_ENTRIES: &'static [&'static str] = &["async_echo", "async_delay"];

    fn new() -> Self {
        Demo {
            calls: AtomicU64::new(0),
        }
    }

    fn new_ctx(&self) -> DemoCtx {
        DemoCtx { uppercase: false }
    }

    fn on_call(&self, session: &mut Session<'_>, ctx: &mut DemoCtx) -> Reply {
        self.calls.fetch_add(1, Ordering::Relaxed);
        match session.entry() {
            "echo" => Reply::Bytes(session.payload().to_vec()),
            "shout" => {
                // Decide something early in the call, use it later — the
                // Pingora CTX pattern, compressed into one handler here.
                ctx.uppercase = session.payload().is_ascii();
                let text = String::from_utf8_lossy(session.payload()).to_string();
                Reply::Text(if ctx.uppercase {
                    text.to_uppercase()
                } else {
                    text
                })
            }
            "count" => {
                let calls = self.calls.load(Ordering::Relaxed);
                Reply::Bytes(calls.to_le_bytes().to_vec())
            }
            "notify" => Reply::None,
            "stream" => {
                for i in 1..=3u32 {
                    let status = session.send_frame(format!("frame {i}").into_bytes());
                    if status != NrStatus::Ok {
                        return Reply::Fail(status);
                    }
                }
                let status = session.end_stream(b"done".to_vec());
                if status != NrStatus::Ok {
                    return Reply::Fail(status);
                }
                Reply::None
            }
            _ => Reply::Fail(NrStatus::Invalid),
        }
    }

    // Native `async fn` — no async_trait crate. The ABI callback returned
    // Ok already; a ready-on-first-poll future (async_echo) delivers
    // inline, while one that suspends (async_delay) finishes on the
    // executor thread and replies from there.
    // No shared-counter update here: `count` reports synchronous calls
    // only, which also keeps this hot path free of cross-thread traffic.
    async fn on_async_call(&self, session: AsyncSession, _ctx: DemoCtx) -> Reply {
        match session.entry() {
            "async_echo" => Reply::Bytes(session.into_payload()),
            "async_delay" => {
                // Suspend once so the future is Pending on its first poll
                // and genuinely exercises spawn_async; then stand in for
                // real async work on the executor thread.
                yield_once().await;
                std::thread::sleep(std::time::Duration::from_millis(10));
                Reply::Bytes(session.into_payload())
            }
            _ => Reply::Fail(NrStatus::Invalid),
        }
    }
}

/// Returns Pending on its first poll and wakes immediately, forcing the
/// glue's inline fast path to hand the call to the executor.
fn yield_once() -> impl std::future::Future<Output = ()> {
    let mut yielded = false;
    std::future::poll_fn(move |cx| {
        if yielded {
            std::task::Poll::Ready(())
        } else {
            yielded = true;
            cx.waker().wake_by_ref();
            std::task::Poll::Pending
        }
    })
}

export_plugin!(Demo);
