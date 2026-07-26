# C++ plugin example

A minimal C++17 plugin built against the canonical header in
[`c/nylon_ring.h`](../../c/nylon_ring.h) (the header wraps itself in
`extern "C"` and switches its layout contract to `static_assert` under C++).
Handlers are declared inside an `extern "C"` block so their types match the
C-linkage function pointers in the vtable. Build it with:

```bash
c++ -std=c++17 -Wall -Wextra -Werror -shared -fPIC \
  examples/cpp-plugin/plugin.cpp -o target/libnylon_ring_cpp_example.so
```

(On macOS use a `.dylib` output name; the compile line is otherwise the same.)

The `echo` entry returns a borrowed view (`owned = 0`); the host copies the
response before the handler returns, so no allocator crosses the boundary.
