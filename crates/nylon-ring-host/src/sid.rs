//! Session ID generation for the nylon-ring-host crate.
//!
//! This module provides thread-local SID (Session ID) generation to minimize
//! contention across threads. Each thread allocates SIDs from a local block,
//! only synchronizing with other threads when the block is exhausted.

use std::cell::Cell;
use std::sync::atomic::{AtomicU64, Ordering};

/// Number of SIDs allocated per block.
const SID_BLOCK_SIZE: u64 = 1_000_000;

/// Global counter for allocating SID blocks.
static GLOBAL_SID: AtomicU64 = AtomicU64::new(1);

/// A block of SIDs allocated to a thread.
#[derive(Copy, Clone)]
struct SidBlock {
    base: u64,
    offset: u64,
}

thread_local! {
    static THREAD_LOCAL_SID_BLOCK: Cell<SidBlock> = const { Cell::new(SidBlock {
        base: 0,
        offset: SID_BLOCK_SIZE,
    }) };
}

/// Generate the next unique session ID.
///
/// This function uses thread-local storage to minimize contention.
/// Each thread maintains a local block of SIDs and only synchronizes
/// with the global counter when the block is exhausted.
pub(crate) fn next_sid() -> u64 {
    THREAD_LOCAL_SID_BLOCK.with(|cell| {
        let mut block = cell.get();
        if block.offset >= SID_BLOCK_SIZE {
            let base = GLOBAL_SID.fetch_add(SID_BLOCK_SIZE, Ordering::Relaxed);
            block = SidBlock { base, offset: 0 };
        }
        let sid = block.base + block.offset;
        block.offset += 1;
        cell.set(block);
        sid
    })
}
