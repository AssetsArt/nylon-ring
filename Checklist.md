# Nylon Ring — Checklist สิ่งที่ควรแก้/เพิ่ม

## 🔴 Must-fix (UB / soundness)

- [ ] **`NrAny::clone` memcpy ดิบ** (`crates/nylon-ring/src/lib.rs:412-441`) — ห้าม clone หรือเพิ่ม `clone_fn` ใน vtable (ABI v2). ปัจจุบันพังกับ type ที่มี heap (String/Vec).
- [ ] **TLS slot aliasing ใน callback router** (`crates/nylon-ring-host/src/callbacks.rs:18-127`) — `CURRENT_UNARY_RESULT`/`CURRENT_UNARY_TX` เก็บ raw pointer ไป stack; ใส่ RAII drop guard เพื่อ clear slot บน unwind/early return.
- [ ] **`NrVec::reserve` ใช้ global allocator** (`crates/nylon-ring/src/lib.rs:967-1033`) — เพิ่ม `owned` flag, ห้าม resize บน borrowed view, กัน free/realloc ข้าม allocator.
- [ ] **`NrStr::push_str` / `Clone` ownership กำกวม** (`crates/nylon-ring/src/lib.rs:303-381`) — เขียน docstring ระบุชัดว่า pointer เก่าใช้ไม่ได้ + ใครเป็นเจ้าของบัฟเฟอร์ใหม่.
- [ ] **Plugin `unload()` / `reload()` ขณะมี in-flight call** (`crates/nylon-ring-host/src/lib.rs:297-324`) — เพิ่ม refcount + drain pending ก่อนปิด `.so`, ไม่งั้น UAF/null vtable.

## 🟠 Should-fix (concurrency / ABI)

- [ ] **TOCTOU ระหว่าง `get_pending_stream` (read) → `remove_pending` (write)** (`crates/nylon-ring-host/src/context.rs:39-74`) — ใช้ DashMap `entry()` API หรือ re-check หลัง upgrade lock.
- [ ] **Stream channel ไม่มี backpressure** (`crates/nylon-ring/src/lib.rs:157-184`) — เปลี่ยน `unbounded_channel` → `channel(N)` หรือ handle `TrySendError::Full`.
- [ ] **ไม่มี timeout ใน `call_response` / `call_response_fast`** — oneshot อาจ hang ถาวรถ้า plugin หาย; เพิ่ม `call_response_timeout(dur)`.
- [ ] **ไม่มีการตรวจ `struct_size` ของ `NrPluginInfo`** (`crates/nylon-ring/src/lib.rs:242-262`) — เช็คแค่ `compatible(1)`; เพิ่ม size check เพื่อกัน layout drift.
- [ ] **ล็อก `NrStatus` discriminant + เอกสาร padding ของ `NrStr`** (`crates/nylon-ring/src/lib.rs:14-21`) — กำหนด reserved range สำหรับ ABI v1 และระบุ 4B padding หลัง `len` ให้ภาษาอื่น.
- [ ] **Stream re-insert race** (`crates/nylon-ring-host/src/callbacks.rs:115-122`) — ระบุ "single-writer per SID" หรือใช้ atomic state กัน double-close.

## 🟡 ควรเพิ่ม (test / docs)

- [ ] **เพิ่ม `#[cfg(test)]` ใน `nylon-ring-host`** — ปัจจุบัน 0 tests; เพิ่มเคส TLS reentry, race remove+reinsert, stream cancel, unload-while-busy.
- [ ] **รัน `cargo +nightly miri` และ `loom`** กับ callback router + SID generator.
- [ ] **เอกสาร "ABI Evolution Plan v2"** — `clone_fn`, struct versioning, deprecation policy.
- [ ] **Error context ละเอียดขึ้น** (`crates/nylon-ring-host/src/error.rs`) — แยก `OneshotClosed` เป็น `PluginUnloaded` / `PluginPanicked` / `Timeout`.

## 🟢 Nice-to-have (perf / ergonomics)

- [ ] Lazy-init shard map ใน `HostContext` (เลี่ยง ~2KB overhead ตอน startup ของ host ที่ไม่ใช้ async path).
- [ ] เพิ่ม `reload_with_grace(Duration)` แทน `reload()` แบบ swap ดื้อ ๆ.
- [ ] Metrics hook (pending count, shard contention) สำหรับ debug production.
- [ ] ตัวอย่าง plugin เป็นภาษาอื่น (C/Zig) เพื่อ validate ABI ตามที่ README เคลม.

## ลำดับความสำคัญ

1. แก้ `NrAny::clone`, TLS slot guard, `NrVec` allocator → ปิดช่อง UB
2. แก้ unload/reload safety + stream TOCTOU → เสถียรในโปรดักชัน
3. เพิ่มชุดเทสต์ + miri/loom → ป้องกัน regression
4. ค่อยทำ ergonomics/metrics
