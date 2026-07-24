# ABI evolution policy

Nylon Ring's native ABI is versioned independently from the crates' semantic
versions. The exported entry point includes the ABI major version:
`nylon_ring_get_plugin_v1`.

## ABI v1

ABI v1 is the first published wire layout. Before the `0.1.0` crates are
published, safety-related layout changes are incorporated directly into v1:

- `NrStatus` is a transparent `u32`; values 7 and above are reserved.
- `NrStr` contains an explicit four-byte reserved field after its `u32` length.
- `NrVec<T>` carries an ownership byte and an allocator-specific drop callback.
- `NrAny` carries type-aware clone and drop callbacks.
- Every plugin reports `abi_version` and `struct_size` in `NrPluginInfo`.

Hosts reject a smaller `NrPluginInfo`. A larger structure is accepted so a
future ABI-compatible producer can append optional fields.

## Rules for compatible changes

An ABI v1-compatible release may:

- assign a currently reserved status value;
- add a new optional function table reached through an existing reserved field;
- clarify safety requirements without changing layout.

It must not reorder fields, change an existing function signature, reinterpret
an assigned status value, or require a previously optional callback.

## ABI v2

A breaking layout or calling-convention change will use new types and export
`nylon_ring_get_plugin_v2`. During a transition, a plugin may export both v1
and v2 entry points and a host will request the newest version it supports.

The v1 entry point and types will remain available for at least one minor
release after v2 is published. Removal requires a semver-major crate release.
Release notes must include a field-by-field migration table and update the C
reference header under `examples/c-plugin`.

Potential v2 work includes allocator-aware zero-copy receive buffers, explicit
plugin instance contexts on every callback, and capability-negotiated extension
tables.
