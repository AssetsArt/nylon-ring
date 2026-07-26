# Zig plugin example

A minimal Zig plugin mirroring the canonical header
[`c/nylon_ring.h`](../../c/nylon_ring.h) with `extern struct` definitions.
A `comptime` block repeats the header's layout contract (sizes and
`@offsetOf` for every field), so drift between this file and the ABI fails
the Zig build the same way it fails a C build.

Build it with:

```bash
zig build-lib -dynamic -O ReleaseFast \
  -femit-bin=target/libnylon_ring_zig_example.so \
  examples/zig-plugin/plugin.zig
```

(On macOS use a `.dylib` output name; the compile line is otherwise the same.)

The `echo` entry returns a borrowed view (`owned = 0`); the host copies the
response before the handler returns, so no allocator crosses the boundary.
