# ABI evolution

This document records what ABI v1 freezes, what may still change inside v1,
how a plugin/host pair negotiates versions, and the concrete proposals for
ABI v2. It exists so that performance or feature work never mutates the wire
contract by accident: if a change conflicts with the "Frozen in v1" section,
it must be written up here as a v2 proposal instead of being implemented.

## Version negotiation as implemented

- The plugin exports `nylon_ring_get_plugin_v1`, returning `*const
  NrPluginInfo`.
- `NrPluginInfo.abi_version` must equal the host's `ABI_VERSION` (currently
  `1`); any mismatch is a hard load failure. There is no minor-version
  negotiation inside v1.
- `NrPluginInfo.struct_size` must be **at least** the host's
  `size_of::<NrPluginInfo>()`. A plugin built against a later revision that
  *appended* fields to `NrPluginInfo` still loads on an older host; an older
  plugin does not load on a host that requires the larger struct. Appending
  to `NrPluginInfo` is therefore the one sanctioned structural extension
  point, and it only flows in the plugin→host direction.
- Both sides must be the same target triple. The ABI is C-layout stable, not
  cross-architecture stable.

## Frozen in v1

Everything in this section is load-bearing for already-shipped binaries.
None of it may change while `ABI_VERSION == 1`.

### Types (layout, size, field order)

| Type | Layout notes |
|---|---|
| `NrStr` | `ptr: *const u8, len: u32, _reserved: u32`. `_reserved` fills the padding after `len`; producers must write zero. |
| `NrBytes` | `ptr: *const u8, len: u64`. |
| `NrVec<T>` | `ptr, len, cap, owned: u8, _reserved: [u8; 7], drop_fn`. `owned = 0` means borrowed and never freed by the consumer. |
| `NrKV`, `NrKVAny`, `NrAny`, `NrMap`, `NrIndexSlot`, `NrTuple` | `#[repr(C)]`, frozen as declared. |
| `NrPluginInfo` | Append-only (see `struct_size` rule above); existing fields never move. |
| `NrPluginVTable` | `init, handle, shutdown, stream_data, stream_close`, all `Option<fn>` — a null pointer is "not supported", which is how v1 already tolerates absent capabilities. |
| `NrHostVTable` | `send_result` only. |
| `NrHostExt` | `set_state`, `get_state`. Extension tables are reached through a pointer, so they can gain *new sibling tables* in v2 but not change shape in v1. |

### Status values

`NrStatus` is a transparent `u32`, not a Rust enum, so unknown values stay
well-defined. Assigned values are frozen forever — a value is never reused or
renumbered, even in v2:

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

### Ownership and lifetime rules

- `NrStr` and `NrBytes` are **borrowed views**. They are valid only for the
  duration of the call that passed them; whoever retains the data must copy.
- An owned `NrVec` (`owned = 1`) is freed exclusively through its producer's
  `drop_fn`. The consumer never calls its own allocator on the buffer:
  cross-allocator frees are a hard bug, and no field of v1 proves allocator
  identity (function-pointer address comparison is not sound — Rust may merge
  equivalent functions or duplicate one generic across images).
- A borrowed `NrVec` (`owned = 0`, `drop_fn = None`) may be sent as a
  response payload; the host copies it inside `send_result` before returning,
  so the memory only needs to outlive that synchronous call.
- `send_result` may be called from any thread, before or after the
  originating `handle` returns.

### Behavioral contract

- `handle` returning non-`Ok` means the call failed and no response will be
  routed for that `sid`.
- Stream frames are delivered in `send_result` order; a terminal status (see
  table) closes the stream. `Backpressure` reports a full bounded queue and
  the frame is not enqueued — the producer decides whether to retry.
- Plugin panics are contained at the FFI boundary and surface as `Panic`.
- Hosts guarantee the lifecycle invariants (in-flight call guards, graceful
  unload/reload drain, timeouts, per-stream backpressure) regardless of
  plugin behavior; nothing in the ABI lets a plugin opt out of them.

## Changeable inside v1

- Everything host-internal: routing/shard structure, SID allocation strategy,
  completion state machines, lock layout. (Rounds 1–5 in
  PERFORMANCE_INVESTIGATION.md all fall in this class.)
- Additive Rust API on the ABI types (`NrStr::as_bytes` is the precedent) —
  new methods, never layout.
- `define_plugin!` codegen, provided observable dispatch semantics stay
  identical (byte-wise entry matching is the precedent: non-UTF-8 entries
  still fall through to `Invalid`).
- New entry-point *names* — the entry namespace belongs to applications.
- Appending fields to `NrPluginInfo` under the `struct_size` rule.

## ABI v2 proposals

Measured motivation: a non-empty unary round trip pays two alloc/free pairs
and two payload copies (~30 ns fixed + size-dependent copy; see rounds 2 and
4). Both pairs exist only because v1 cannot prove allocator provenance across
images. A one-copy probe measured end-to-end savings of ~23 ns at 128 B up to
~76 ns at 4 KiB on the reference M1 Pro.

### P1: `NrOwnedBytesV2` — callee-owned response without a host copy

**Status: implemented** (host probes `nylon_ring_get_plugin_v2` and falls
back to v1; `define_plugin!` exports both symbols from one binary; the host
API is `PluginHandle::call_response_bytes` returning a `ResponseBytes` view).
Contract as shipped: the consumer calls `release` exactly once, possibly
from a different thread than the producer; the producer keeps the bytes
valid and immutable until then; a null `release` means nothing to free.
The host holds an in-flight call guard for the view's lifetime, so
`unload()` defers the library drop until the last response view is gone.
Reference numbers (M1 Pro, 1 worker, avg-latency harness): v1 echo
53/89/149/224 ns at 0/128/1024/4096 B versus a flat ~55 ns for a v2
slab-backed owned response at every size; at 10 workers and 4 KiB,
18.3M -> 116.0M calls/s.

```c
typedef struct NrOwnedBytesV2 {
    const uint8_t *ptr;
    uint64_t       len;
    void          *owner_ctx;
    void         (*release)(void *owner_ctx, const uint8_t *ptr, uint64_t len);
} NrOwnedBytesV2;
```

The host returns a `ResponseBytes` view that calls `release` on drop instead
of copying into a host `Vec`. The host must hold a library guard for the view's
lifetime so the code behind `release` cannot be unloaded first — this extends
the in-flight guard's scope from "during the call" to "until the response is
dropped", which is the main lifecycle cost of the proposal.
Removes the host-side alloc/copy/free (the guarded half of the ~30 ns).

### P2: host-owned output buffers — zero-copy in the other direction

**Status: implemented** (as two further members of `NrHostVTableV2`, added
before any v2 release shipped, so no v3 was needed). The signature deviates
from the original sketch in one way: `acquire` takes the `sid`, which is
what ties the lease to the pending entry for reclamation.

```c
/* NrBufferLeaseV2 { uint8_t *ptr; uint64_t cap; uint64_t token; } */
NrBufferLeaseV2 acquire_result_buffer(void *host_ctx, uint64_t sid,
                                      uint64_t capacity);
NrStatus commit_result_buffer(void *host_ctx, uint64_t sid,
                              NrStatus status, uint64_t token,
                              uint64_t initialized_len);
```

The plugin writes its response directly into memory the host allocated, then
commits it; the host hands its own `Vec` to the caller with no provenance
question — the plain `call_response` Vec API becomes zero-copy with no
guard or deferred-release entanglement.

Contract as shipped:

- A failed acquire returns a null `ptr` (unknown sid, streaming sid, or a
  lease already outstanding — at most one per sid); the plugin must fall
  back to another response path.
- `commit` passes the lease's `token` back with `initialized_len <= cap`
  bytes written. On `Ok` the buffer belongs to the host; a failed commit
  (bad token, oversized length) keeps the lease valid for a corrected
  retry; committing a consumed lease reports `Invalid`.
- Responding through any other channel for the same sid consumes the lease;
  touching the buffer afterwards is undefined behavior.
- Leak/soundness handling: a lease whose call the host abandons (timeout,
  cancel, receiver drop) is parked, not freed — the plugin may still hold
  the pointer. It is freed on a late commit or at host-context drop, after
  the plugin's shutdown contract forbids further writes.

Reference numbers (M1 Pro, avg-latency harness, `NYRING_BENCH_OPERATION=
lease`): v1 echo 53/89/150/221 ns at 0/128/1024/4096 B versus lease echo
59/77/106/145 ns — ~+6 ns fixed for acquire+commit, break-even ≈ 64 B; at
10 workers and 4 KiB, 19.2M -> 50.1M calls/s.

### P3 (candidate): integer entry dispatch

`register_entries(names) -> ids` at init, plus
`handle_by_id(id: u32, sid, payload)` alongside the string-keyed `handle`.
Saves the per-call name compare and `NrStr` construction (single-digit ns per
call; measurable but the smallest of the three). Purely additive — a v2 host
falls back to `handle` when `handle_by_id` is null.

### Delivery mechanism

Ship v2 as a *second* export, `nylon_ring_get_plugin_v2`, so one plugin binary
can serve both hosts during migration. A v2 host probes v2 first, then falls
back to the v1 symbol; strict `abi_version` equality per symbol stays. The v1
symbol is never removed while v1 hosts exist.

Implemented layout (additive):

- `NrPluginInfoV2` / `NrPluginVTableV2`: identical shape to v1; only `init`
  differs, receiving `NrHostVTableV2`.
- `NrHostVTableV2` embeds the v1 table as its first field, so v2-aware
  plugin code can hand `&table.v1` to v1-style helpers unchanged; after it
  come `send_result_owned` (P1) and `acquire_result_buffer`/
  `commit_result_buffer` (P2).
- `define_plugin!` accepts an optional `init_v2:` handler; without one, the
  v2 export reuses the v1 init through the embedded table.
