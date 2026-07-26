# C plugin example

A minimal plugin built against the canonical header in [`c/nylon_ring.h`](../../c/nylon_ring.h),
which also carries the compile-time layout contract. Build it with:

```bash
cc -std=c11 -Wall -Wextra -Werror -shared -fPIC \
  examples/c-plugin/plugin.c -o target/libnylon_ring_c_example.so
```

(On macOS use a `.dylib` output name; the compile line is otherwise the same.)

The `echo` entry returns a borrowed view. The Rust host copies the response
before the C handler returns, and the `owned = 0` flag prevents allocator
mismatches.
