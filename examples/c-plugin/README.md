# C plugin example

This example is the reference C layout for the nylon-ring ABI. Build it on Linux with:

```bash
cc -std=c11 -Wall -Wextra -Werror -shared -fPIC \
  examples/c-plugin/plugin.c -o target/libnylon_ring_c_example.so
```

The `echo` entry returns a borrowed view. The Rust host copies the response
before the C handler returns, and the `owned = 0` flag prevents allocator
mismatches.
