# C interface

`nylon_ring.h` is the canonical C header for the nylon-ring ABI, mirroring
the `#[repr(C)]` definitions in `crates/nylon-ring`. It is hand-written so it
doubles as readable reference documentation.

The bottom of the header carries a compile-time layout contract: size and
`offsetof` `_Static_assert`s for every ABI type, checked in every translation
unit that includes it. The Rust side asserts the same numbers in the
`test_layout` test of `crates/nylon-ring`, so drift in either direction —
including same-size field reordering — fails a build rather than corrupting
calls at runtime.

When the ABI changes, update all three together:

1. the `#[repr(C)]` types in `crates/nylon-ring/src/lib.rs`,
2. this header (declarations and the assert block), and
3. the `test_layout` assertions on the Rust side.

Minimal plugins built against this contract live under `examples/`:
`c-plugin` and `cpp-plugin` include the header directly, `go-plugin` wraps
it through a cgo shim (and must be loaded with `load_pinned` — the Go
runtime cannot be dlclosed), and `zig-plugin` mirrors it with `extern
struct` definitions plus the same layout asserts at `comptime`.
