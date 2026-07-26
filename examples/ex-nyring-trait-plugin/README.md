# Trait-based Rust plugin example

Implements `nylon_ring::Plugin` — the
[Pingora-inspired](https://github.com/cloudflare/pingora/blob/main/docs/user_guide/ctx.md)
trait API — instead of the raw `define_plugin!` handlers:

- **Cross-call state** lives on the plugin struct (`Demo.calls`), shared by
  every call like Pingora's service struct.
- **Per-call state** lives in the `Ctx` associated type (`DemoCtx`), created
  fresh by `new_ctx()` for each call.
- Handlers see a safe `Session` (entry name, payload as `&[u8]`, stream
  helpers) and answer with a `Reply` — no `unsafe`, and panics are contained
  at the ABI boundary by the generated glue.

Entries: `echo`, `shout` (uppercases ASCII), `count` (calls served, u64 LE),
`notify` (fire-and-forget), `stream` (3 frames + end).

Async entries (`ASYNC_ENTRIES`): `async_echo`, `async_delay` — handled by a
plain `async fn on_async_call` (native `impl Future` in trait, no
`async_trait` crate). The ABI callback returns `Ok` immediately; the reply
is delivered when the future completes, through the host's cross-thread
async path. Futures run on `Plugin::spawn_async` — dependency-free
thread-per-call by default, overridable with `tokio::spawn`.

Build:

```bash
cargo build --release --package ex-nyring-trait-plugin
```

The raw `define_plugin!` example remains at `examples/ex-nyring-plugin` for
handlers that need the owned-response or buffer-lease paths directly.
