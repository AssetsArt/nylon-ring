//! Reusable bounded channel for stream frames.
//!
//! Every stream used to allocate a fresh `tokio::sync::mpsc` channel, whose
//! setup (allocations plus mutex init/destroy) bounded multi-core stream
//! scaling. This channel keeps the same observable semantics — bounded
//! capacity with `Backpressure` on overflow, and `recv() == None` once the
//! sender is gone and the queue is drained — but its storage is recycled
//! through a thread-local pool.
//!
//! Soundness of reuse rests on two invariants:
//! - All sends happen while the pending-map shard entry is locked, and
//!   `cleanup_sid` removes the entry (dropping the sender) under that same
//!   lock, so once recycling starts no send can still be in flight.
//! - A channel is pooled only when the receiver holds the last reference
//!   (`Arc::get_mut` succeeds); otherwise it is simply dropped. Stale frames,
//!   the parked waker, and the closed flag are cleared before pooling, so no
//!   state can leak into the next stream.

use crate::types::StreamFrame;
use spin_lock::Mutex;
use std::cell::RefCell;
use std::collections::VecDeque;
use std::future::poll_fn;
use std::sync::Arc;
use std::task::{Poll, Waker};

/// Minimal TTAS spin mutex for the per-frame channel state.
///
/// Locks from std (pthread) and parking_lot were both measured materially
/// slower here: pthread mutexes issue full memory barriers that stall this
/// path once every core is busy, and parking_lot's compare-exchange unlock
/// costs an extra exclusive pair per release. This lock is a single acquire
/// CAS to lock and a plain release store to unlock.
///
/// A spin lock is sound here because contention is structurally near zero:
/// every send happens under the pending-map shard entry lock (serialized),
/// and the receiver only holds the lock for a queue pop or waker swap — a
/// handful of instructions with no allocation. If the holder is preempted
/// mid-section anyway, the bounded spin falls back to `yield_now`, so a
/// waiter cannot burn a full scheduling quantum.
mod spin_lock {
    use std::cell::UnsafeCell;
    use std::ops::{Deref, DerefMut};
    use std::sync::atomic::{AtomicBool, Ordering};

    const SPINS_BEFORE_YIELD: u32 = 64;

    #[derive(Debug, Default)]
    pub(crate) struct Mutex<T> {
        locked: AtomicBool,
        value: UnsafeCell<T>,
    }

    // SAFETY: the lock protocol below gives exclusive access to `value`
    // between the acquire CAS and the release store.
    unsafe impl<T: Send> Send for Mutex<T> {}
    unsafe impl<T: Send> Sync for Mutex<T> {}

    pub(crate) struct Guard<'a, T> {
        lock: &'a Mutex<T>,
    }

    impl<T> Mutex<T> {
        pub(crate) fn new(value: T) -> Self {
            Self {
                locked: AtomicBool::new(false),
                value: UnsafeCell::new(value),
            }
        }

        pub(crate) fn lock(&self) -> Guard<'_, T> {
            let mut spins = 0u32;
            loop {
                // Test-and-test-and-set: spin on a plain load so waiting
                // does not ping-pong the cache line with CAS traffic.
                if !self.locked.load(Ordering::Relaxed)
                    && self
                        .locked
                        .compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed)
                        .is_ok()
                {
                    return Guard { lock: self };
                }
                spins += 1;
                if spins < SPINS_BEFORE_YIELD {
                    std::hint::spin_loop();
                } else {
                    std::thread::yield_now();
                }
            }
        }

        pub(crate) fn get_mut(&mut self) -> &mut T {
            self.value.get_mut()
        }
    }

    impl<T> Deref for Guard<'_, T> {
        type Target = T;
        fn deref(&self) -> &T {
            // SAFETY: the guard holds the lock.
            unsafe { &*self.lock.value.get() }
        }
    }

    impl<T> DerefMut for Guard<'_, T> {
        fn deref_mut(&mut self) -> &mut T {
            // SAFETY: the guard holds the lock.
            unsafe { &mut *self.lock.value.get() }
        }
    }

    impl<T> Drop for Guard<'_, T> {
        fn drop(&mut self) {
            self.lock.locked.store(false, Ordering::Release);
        }
    }
}

/// Upper bound of idle channels cached per thread.
const MAX_POOLED: usize = 16;

#[derive(Debug)]
struct ChannelState {
    queue: VecDeque<StreamFrame>,
    waker: Option<Waker>,
    closed: bool,
}

/// Aligned to the M1's 128-byte cache lines: the whole struct is ~96 bytes,
/// so unaligned instances from neighboring allocations can share a line
/// across threads and turn every send/recv into cross-core traffic.
#[derive(Debug)]
#[repr(align(128))]
pub(crate) struct StreamChannel {
    state: Mutex<ChannelState>,
    capacity: usize,
}

/// Sending half, stored in the pending map. Dropping it closes the channel:
/// the receiver drains buffered frames and then observes `None`.
#[derive(Debug)]
pub(crate) struct StreamSender {
    chan: Arc<StreamChannel>,
}

impl StreamChannel {
    /// Queues a frame. When the queue already holds `capacity` frames — or
    /// the channel is already closed (only reachable through the thread-local
    /// frame slot after an async terminal frame removed the pending entry) —
    /// the frame is handed back in `Err` so its payload drops outside the
    /// lock.
    pub(crate) fn try_send(&self, frame: StreamFrame) -> Result<(), StreamFrame> {
        let mut state = self.state.lock();
        if state.closed || state.queue.len() >= self.capacity {
            drop(state);
            return Err(frame);
        }
        state.queue.push_back(frame);
        let waker = state.waker.take();
        drop(state);
        if let Some(waker) = waker {
            waker.wake();
        }
        Ok(())
    }
}

impl StreamSender {
    pub(crate) fn try_send(&self, frame: StreamFrame) -> Result<(), StreamFrame> {
        self.chan.try_send(frame)
    }

    /// Shared handle for the thread-local frame slot: keeps the channel
    /// alive across the plugin's `handle` call independently of the pending
    /// map entry (an async terminal frame may remove that entry mid-call).
    pub(crate) fn channel(&self) -> Arc<StreamChannel> {
        self.chan.clone()
    }
}

impl Drop for StreamSender {
    fn drop(&mut self) {
        let mut state = self.chan.state.lock();
        state.closed = true;
        let waker = state.waker.take();
        drop(state);
        if let Some(waker) = waker {
            waker.wake();
        }
    }
}

/// Receiving half, owned by [`crate::StreamReceiver`].
#[derive(Debug)]
pub(crate) struct StreamChannelReceiver {
    chan: Option<Arc<StreamChannel>>,
}

impl StreamChannelReceiver {
    /// Waits for the next frame; `None` once the sender is gone and the
    /// queue is drained.
    pub(crate) async fn recv(&mut self) -> Option<StreamFrame> {
        // Consume tokio coop budget like the tokio channel this replaced:
        // a receiver whose frames are always ready would otherwise never
        // yield, monopolizing its worker thread and starving co-scheduled
        // tasks (parked workers get no wake-up to steal them).
        tokio::task::consume_budget().await;
        let chan = self.chan.as_ref().expect("receiver used after recycle");
        poll_fn(|cx| {
            // Waker callbacks (clone/drop) may run arbitrary code, so they
            // must not run while the channel lock is held; mirror the
            // re-lock pattern used by `context::wait_for_unary`.
            let mut replacement = None;
            loop {
                let mut state = chan.state.lock();
                if let Some(frame) = state.queue.pop_front() {
                    drop(state);
                    drop(replacement);
                    return Poll::Ready(Some(frame));
                }
                if state.closed {
                    drop(state);
                    drop(replacement);
                    return Poll::Ready(None);
                }
                if state
                    .waker
                    .as_ref()
                    .is_some_and(|waker| waker.will_wake(cx.waker()))
                {
                    drop(state);
                    drop(replacement);
                    return Poll::Pending;
                }
                if let Some(replacement) = replacement.take() {
                    let previous = state.waker.replace(replacement);
                    drop(state);
                    drop(previous);
                    return Poll::Pending;
                }
                drop(state);
                replacement = Some(cx.waker().clone());
            }
        })
        .await
    }

    /// Non-blocking receive; `None` when the queue is currently empty.
    #[cfg(test)]
    pub(crate) fn try_recv(&mut self) -> Option<StreamFrame> {
        let chan = self.chan.as_ref().expect("receiver used after recycle");
        chan.state.lock().queue.pop_front()
    }

    /// Returns the channel to the current thread's pool for reuse.
    ///
    /// Reuse requires sole ownership: the sender must already be dropped
    /// (guaranteed by `cleanup_sid` running first). If any other reference
    /// still exists the channel is dropped instead of pooled.
    pub(crate) fn recycle(&mut self) {
        let Some(mut chan) = self.chan.take() else {
            return;
        };
        let Some(chan_mut) = Arc::get_mut(&mut chan) else {
            return;
        };
        let state = chan_mut.state.get_mut();
        state.queue.clear();
        state.waker = None;
        state.closed = false;
        POOL.with(|pool| {
            let mut pool = pool.borrow_mut();
            if pool.len() < MAX_POOLED {
                pool.push(chan);
            }
        });
    }
}

thread_local! {
    static POOL: RefCell<Vec<Arc<StreamChannel>>> = const { RefCell::new(Vec::new()) };
}

/// Creates (or reuses) a bounded channel for one stream.
pub(crate) fn acquire(capacity: usize) -> (StreamSender, StreamChannelReceiver) {
    let pooled = POOL.with(|pool| {
        let mut pool = pool.borrow_mut();
        match pool.last() {
            Some(chan) if chan.capacity == capacity => pool.pop(),
            _ => None,
        }
    });
    let chan = pooled.unwrap_or_else(|| {
        Arc::new(StreamChannel {
            state: Mutex::new(ChannelState {
                queue: VecDeque::with_capacity(capacity),
                waker: None,
                closed: false,
            }),
            capacity,
        })
    });
    (
        StreamSender { chan: chan.clone() },
        StreamChannelReceiver { chan: Some(chan) },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    use nylon_ring::NrStatus;
    use std::future::Future;

    struct ThreadWaker(std::thread::Thread);

    impl std::task::Wake for ThreadWaker {
        fn wake(self: Arc<Self>) {
            self.0.unpark();
        }
    }

    fn block_on<F: Future>(fut: F) -> F::Output {
        let mut fut = std::pin::pin!(fut);
        let waker = Waker::from(Arc::new(ThreadWaker(std::thread::current())));
        let mut cx = std::task::Context::from_waker(&waker);
        loop {
            match fut.as_mut().poll(&mut cx) {
                Poll::Ready(value) => return value,
                Poll::Pending => std::thread::park(),
            }
        }
    }

    fn frame(byte: u8) -> StreamFrame {
        StreamFrame {
            status: NrStatus::Ok,
            data: vec![byte],
        }
    }

    fn drain_pool() {
        POOL.with(|pool| pool.borrow_mut().clear());
    }

    #[test]
    fn sender_drop_closes_after_buffered_frames() {
        drain_pool();
        let (tx, mut rx) = acquire(4);
        assert!(tx.try_send(frame(1)).is_ok());
        assert!(tx.try_send(frame(2)).is_ok());
        drop(tx);
        block_on(async {
            assert_eq!(rx.recv().await.unwrap().data, vec![1]);
            assert_eq!(rx.recv().await.unwrap().data, vec![2]);
            assert!(rx.recv().await.is_none());
            assert!(rx.recv().await.is_none(), "closed channel must stay closed");
        });
    }

    #[test]
    fn full_queue_reports_backpressure_and_returns_frame() {
        drain_pool();
        let (tx, mut rx) = acquire(1);
        assert!(tx.try_send(frame(1)).is_ok());
        assert_eq!(
            tx.try_send(frame(2))
                .expect_err("second frame must not fit")
                .data,
            vec![2]
        );
        assert_eq!(rx.try_recv().unwrap().data, vec![1]);
        assert!(tx.try_send(frame(3)).is_ok());
    }

    #[test]
    fn recycle_clears_stale_state_and_reuses_allocation() {
        drain_pool();
        let (tx, mut rx) = acquire(4);
        assert!(tx.try_send(frame(9)).is_ok());
        drop(tx);
        let first_ptr = Arc::as_ptr(rx.chan.as_ref().unwrap());
        // Recycle without consuming: stale frame and closed flag must not
        // leak into the next stream.
        rx.recycle();

        let (tx, mut rx) = acquire(4);
        assert_eq!(Arc::as_ptr(rx.chan.as_ref().unwrap()), first_ptr);
        assert!(rx.try_recv().is_none(), "stale frame leaked across reuse");
        assert!(tx.try_send(frame(1)).is_ok());
        block_on(async {
            assert_eq!(rx.recv().await.unwrap().data, vec![1]);
        });
        drop(tx);
        block_on(async {
            assert!(rx.recv().await.is_none());
        });
    }

    #[test]
    fn recycle_refuses_shared_channel_and_capacity_mismatch() {
        drain_pool();
        let (tx, mut rx) = acquire(4);
        // Sender still alive: recycling must drop the reference, not pool it.
        rx.recycle();
        assert_eq!(
            POOL.with(|pool| pool.borrow().len()),
            0,
            "shared channel must not be pooled"
        );
        drop(tx);
        let (tx, mut rx) = acquire(4);
        drop(tx);
        rx.recycle();
        let (_tx, rx3) = acquire(8);
        assert_eq!(rx3.chan.as_ref().unwrap().capacity, 8);
    }

    #[test]
    fn cross_thread_send_wakes_receiver() {
        drain_pool();
        let (tx, mut rx) = acquire(4);
        let sender = std::thread::spawn(move || {
            for value in 0..3u8 {
                let mut pending = frame(value);
                loop {
                    match tx.try_send(pending) {
                        Ok(()) => break,
                        Err(rejected) => {
                            pending = rejected;
                            std::thread::yield_now();
                        }
                    }
                }
            }
        });
        block_on(async {
            for value in 0..3u8 {
                assert_eq!(rx.recv().await.unwrap().data, vec![value]);
            }
            assert!(rx.recv().await.is_none());
        });
        sender.join().unwrap();
    }
}
