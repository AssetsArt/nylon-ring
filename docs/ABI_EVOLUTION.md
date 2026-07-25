# ABI

Nylon Ring speaks **one ABI version** (`ABI_VERSION = 2`), negotiated
through the single `nylon_ring_get_plugin` export. Earlier revisions
carried a v1/v2 split with dual exports and fallbacks; it was collapsed
before any real deployment existed, so hosts and plugins never need to
reason about versions — a mismatch is simply a load failure.

This document records what the ABI guarantees, what may still change
behind it, and the performance rationale for its zero-copy members.

## Version negotiation

- The plugin exports `nylon_ring_get_plugin`, returning `*const
  NrPluginInfo`.
- `NrPluginInfo.abi_version` must equal the host's `ABI_VERSION`; any
  mismatch is a hard load failure. There is no minor-version negotiation.
- `NrPluginInfo.struct_size` must be **at least** the host's
  `size_of::<NrPluginInfo>()`; appending fields to `NrPluginInfo` under
  this rule is the one sanctioned structural extension point (plugin→host
  direction only).
- Both sides must be the same target triple. The ABI is C-layout stable,
  not cross-architecture stable.

## Types (layout, size, field order)

| Type | Layout notes |
|---|---|
| `NrStr` | `ptr: *const u8, len: u32, _reserved: u32`. `_reserved` fills the padding after `len`; producers must write zero. |
| `NrBytes` | `ptr: *const u8, len: u64`. |
| `NrVec<T>` | `ptr, len, cap, owned: u8, _reserved: [u8; 7], drop_fn`. `owned = 0` means borrowed and never freed by the consumer. |
| `NrOwnedBytes` | `ptr, len: u64, owner_ctx, release: Option<fn>`. Consumer calls `release` exactly once, from any thread; null release = nothing to free. |
| `NrBufferLease` | `ptr: *mut u8, cap: u64, token: u64`. Null `ptr` = acquire failed. |
| `NrKV`, `NrKVAny`, `NrAny`, `NrMap`, `NrIndexSlot`, `NrTuple` | `#[repr(C)]`, as declared. |
| `NrPluginInfo` | Append-only (see `struct_size` rule); existing fields never move. |
| `NrPluginVTable` | `init, handle, shutdown, stream_data, stream_close, resolve_entry, handle_by_id`, all `Option<fn>` — a null pointer is "not supported". `resolve_entry`/`handle_by_id` must be both null or both set. |
| `NrHostVTable` | `send_result, send_result_owned, acquire_result_buffer, commit_result_buffer`. |
| `NrHostExt` | `set_state`, `get_state`. Extension tables are reached through a pointer, so new sibling tables can be added without changing this one. |

## Status values

`NrStatus` is a transparent `u32`, not a Rust enum, so unknown values stay
well-defined. Assigned values are never reused or renumbered:

| Value | Name | Terminal for streams? |
|---:|---|---|
| 0 | `Ok` | no |
| 1 | `Err` | yes |
| 2 | `Invalid` | yes |
| 3 | `Unsupported` | yes |
| 4 | `StreamEnd` | yes |
| 5 | `Panic` | yes |
| 6 | `Backpressure` | no |
| 7..=u32::MAX | reserved | — |

New statuses take the next free value and are documented here first.

## Ownership and lifetime rules

- `NrStr` and `NrBytes` are **borrowed views**, valid only for the duration
  of the call that passed them; whoever retains the data must copy.
- An owned `NrVec` (`owned = 1`) is freed exclusively through its
  producer's `drop_fn`. The consumer never calls its own allocator on the
  buffer: cross-allocator frees are a hard bug, and no ABI field proves
  allocator identity.
- A borrowed `NrVec` (`owned = 0`, `drop_fn = None`) may be sent as a
  response payload; the host copies it inside `send_result` before
  returning.
- `NrOwnedBytes` (via `send_result_owned`): the producer keeps
  `ptr..ptr+len` valid and immutable until the consumer calls `release`
  exactly once, possibly from a different thread. The host holds an
  in-flight call guard for as long as it retains the view, so `unload()`
  defers the library drop until the last response view is gone.
- `NrBufferLease` (via `acquire_result_buffer`/`commit_result_buffer`):
  - A failed acquire returns a null `ptr` (unknown sid, streaming sid, or
    a lease already outstanding — at most one per sid); the plugin must
    fall back to another response path.
  - `commit` passes the lease's `token` back with `initialized_len <= cap`
    bytes written. On `Ok` the buffer belongs to the host; a failed commit
    (bad token, oversized length) keeps the lease valid for a corrected
    retry; committing a consumed lease reports `Invalid`.
  - Responding through any other channel for the same sid consumes the
    lease; touching the buffer afterwards is undefined behavior.
  - A lease whose call the host abandons (timeout, cancel, receiver drop)
    is parked, not freed — the plugin may still hold the pointer. It is
    freed on a late commit or at host-context drop, after the plugin's
    shutdown contract forbids further writes.
- `send_result` may be called from any thread, before or after the
  originating `handle` returns.

## Behavioral contract

- `handle` returning non-`Ok` means the call failed and no response will
  be routed for that `sid`.
- Stream frames are delivered in `send_result` order; a terminal status
  closes the stream. `Backpressure` reports a full bounded queue and the
  frame is not enqueued — the producer decides whether to retry.
- Integer entry dispatch: `resolve_entry(name)` returns a dispatch id
  (or `NR_ENTRY_UNKNOWN`) that stays valid for the lifetime of the loaded
  plugin instance; `handle_by_id` must behave identically to `handle` with
  the corresponding name. Hosts resolve once (`PluginHandle::entry`) and
  skip the per-call name comparison.
- Plugin panics are contained at the FFI boundary and surface as `Panic`.
- Hosts guarantee the lifecycle invariants (in-flight call guards,
  graceful unload/reload drain, timeouts, per-stream backpressure)
  regardless of plugin behavior.

## Changeable behind the ABI

- Everything host-internal: routing/shard structure, SID allocation,
  completion state machines, lock layout.
- Additive Rust API on the ABI types — new methods, never layout.
- `define_plugin!` codegen, provided observable dispatch semantics stay
  identical (byte-wise entry matching; ids are list indices).
- New entry-point *names* — the entry namespace belongs to applications.
- Appending fields to `NrPluginInfo` under the `struct_size` rule.

## Performance rationale (measured on the reference M1 Pro)

The three zero-copy members exist because a non-empty unary round trip
through `send_result` pays two alloc/free pairs and two payload copies:

- **`send_result_owned`** (plugin-owned buffers, host consumes zero-copy
  through `PluginHandle::call_response_bytes`): flat ~55 ns at any payload
  size versus 53/89/150/221 ns for the copying path at 0/128/1024/4096 B;
  at 10 workers and 4 KiB, 18.3M → 116.0M calls/s.
- **`acquire_result_buffer`/`commit_result_buffer`** (host-owned buffers,
  plugin serializes in place, plain `call_response` Vec API becomes
  zero-copy): 59/77/106/145 ns over the same sizes — ~+6 ns fixed for
  acquire+commit, break-even ≈ 64 B; at 10 workers and 4 KiB,
  19.2M → 50.1M calls/s.
- **`resolve_entry`/`handle_by_id`** (skip the per-call name compare and
  `NrStr` construction): fast path 25 → 16 ns (+51%), fire-and-forget
  11 → 10 ns (10 workers: 554M → 603M calls/s); unary is map-dominated
  and unchanged.
