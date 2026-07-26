# Go plugin example

A minimal Go plugin built as a c-shared library against the canonical header
in [`c/nylon_ring.h`](../../c/nylon_ring.h). The C shim (`shim.c`) owns the
vtable, plugin info, and entry-name dispatch; the `echo` handler body runs in
Go and answers from a Go-owned buffer through the borrowed `send_result` path
(safe under the cgo pointer-passing rules because the host copies the bytes
before the call returns).

Build it with:

```bash
go build -C examples/go-plugin -buildmode=c-shared \
  -o ../../target/libnylon_ring_go_example.so
```

(On macOS use a `.dylib` output name; the compile line is otherwise the same.)

## Load with `load_pinned` only

The Go runtime cannot be unloaded from a process — `dlclose` on a Go
c-shared library is undefined behavior. Hosts must load this plugin with
`load_pinned`, which never unloads and intentionally leaks the library
handle at host drop. A regular `load` would crash on unload or host drop.
