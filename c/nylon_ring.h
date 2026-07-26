#ifndef NYLON_RING_H
#define NYLON_RING_H

#include <stddef.h>
#include <stdint.h>

#if defined(_WIN32)
#define NYR_EXPORT __declspec(dllexport)
#else
#define NYR_EXPORT __attribute__((visibility("default")))
#endif

#if defined(__cplusplus)
#define NR_LAYOUT_ASSERT(cond, msg) static_assert(cond, msg)
extern "C" {
#else
#define NR_LAYOUT_ASSERT(cond, msg) _Static_assert(cond, msg)
#endif

#define NR_ABI_VERSION 2u

typedef uint32_t NrStatus;
enum {
    NR_OK = 0,
    NR_ERR = 1,
    NR_INVALID = 2,
    NR_UNSUPPORTED = 3,
    NR_STREAM_END = 4,
    NR_PANIC = 5,
    NR_BACKPRESSURE = 6,
};

/* resolve_entry result for a name the plugin does not export. */
#define NR_ENTRY_UNKNOWN UINT32_MAX

typedef struct {
    const uint8_t *ptr;
    uint32_t len;
    uint32_t reserved;
} NrStr;

typedef struct {
    const uint8_t *ptr;
    uint64_t len;
} NrBytes;

typedef void (*NrVecU8DropFn)(uint8_t *ptr, size_t len, size_t cap);

typedef struct {
    uint8_t *ptr;
    size_t len;
    size_t cap;
    uint8_t owned;
    uint8_t reserved[7];
    NrVecU8DropFn drop_fn;
} NrVecU8;

/* A callee-owned buffer the host consumes without copying; the host calls
 * release exactly once (possibly from another thread). NULL release means
 * nothing to free. */
typedef struct {
    const uint8_t *ptr;
    uint64_t len;
    void *owner_ctx;
    void (*release)(void *owner_ctx, const uint8_t *ptr, uint64_t len);
} NrOwnedBytes;

/* A host-owned output buffer leased to the plugin. A failed acquire returns
 * a NULL ptr; commit passes token back with initialized_len <= cap. */
typedef struct {
    uint8_t *ptr;
    uint64_t cap;
    uint64_t token;
} NrBufferLease;

typedef NrStatus (*NrSendResultFn)(
    void *host_ctx,
    uint64_t sid,
    NrStatus status,
    NrVecU8 payload
);

typedef NrStatus (*NrSendResultOwnedFn)(
    void *host_ctx,
    uint64_t sid,
    NrStatus status,
    NrOwnedBytes payload
);

typedef NrBufferLease (*NrAcquireResultBufferFn)(
    void *host_ctx,
    uint64_t sid,
    uint64_t capacity
);

typedef NrStatus (*NrCommitResultBufferFn)(
    void *host_ctx,
    uint64_t sid,
    NrStatus status,
    uint64_t token,
    uint64_t initialized_len
);

typedef struct {
    NrSendResultFn send_result;
    NrSendResultOwnedFn send_result_owned;
    NrAcquireResultBufferFn acquire_result_buffer;
    NrCommitResultBufferFn commit_result_buffer;
} NrHostVTable;

typedef NrStatus (*NrPluginInitFn)(void *host_ctx, const NrHostVTable *host_vtable);
typedef NrStatus (*NrPluginHandleFn)(NrStr entry, uint64_t sid, NrBytes payload);
typedef void (*NrPluginShutdownFn)(void);
typedef NrStatus (*NrPluginStreamDataFn)(uint64_t sid, NrBytes data);
typedef NrStatus (*NrPluginStreamCloseFn)(uint64_t sid);
typedef uint32_t (*NrPluginResolveEntryFn)(NrStr entry);
typedef NrStatus (*NrPluginHandleByIdFn)(uint32_t id, uint64_t sid, NrBytes payload);

typedef struct {
    NrPluginInitFn init;
    NrPluginHandleFn handle;
    NrPluginShutdownFn shutdown;
    NrPluginStreamDataFn stream_data;
    NrPluginStreamCloseFn stream_close;
    /* Optional integer entry dispatch: both NULL or both set. */
    NrPluginResolveEntryFn resolve_entry;
    NrPluginHandleByIdFn handle_by_id;
} NrPluginVTable;

typedef struct {
    uint32_t abi_version;
    uint32_t struct_size;
    NrStr name;
    NrStr version;
    void *plugin_ctx;
    const NrPluginVTable *vtable;
} NrPluginInfo;

NYR_EXPORT const NrPluginInfo *nylon_ring_get_plugin(void);

#if defined(__cplusplus)
} /* extern "C" */
#endif

/* --- Layout contract (64-bit) -----------------------------------------
 * Compile-time mirror of the #[repr(C)] definitions in crates/nylon-ring;
 * the Rust side asserts the same sizes and offsets in its test_layout.
 * Any drift — including same-size field reordering — fails right here in
 * every translation unit that includes this header. */

NR_LAYOUT_ASSERT(sizeof(void *) == 8, "nylon-ring requires a 64-bit target");
NR_LAYOUT_ASSERT(sizeof(NrStatus) == 4, "NrStatus layout mismatch");

NR_LAYOUT_ASSERT(sizeof(NrStr) == 16, "NrStr size mismatch");
NR_LAYOUT_ASSERT(offsetof(NrStr, ptr) == 0, "NrStr.ptr offset mismatch");
NR_LAYOUT_ASSERT(offsetof(NrStr, len) == 8, "NrStr.len offset mismatch");
NR_LAYOUT_ASSERT(offsetof(NrStr, reserved) == 12, "NrStr.reserved offset mismatch");

NR_LAYOUT_ASSERT(sizeof(NrBytes) == 16, "NrBytes size mismatch");
NR_LAYOUT_ASSERT(offsetof(NrBytes, ptr) == 0, "NrBytes.ptr offset mismatch");
NR_LAYOUT_ASSERT(offsetof(NrBytes, len) == 8, "NrBytes.len offset mismatch");

NR_LAYOUT_ASSERT(sizeof(NrVecU8) == 40, "NrVecU8 size mismatch");
NR_LAYOUT_ASSERT(offsetof(NrVecU8, ptr) == 0, "NrVecU8.ptr offset mismatch");
NR_LAYOUT_ASSERT(offsetof(NrVecU8, len) == 8, "NrVecU8.len offset mismatch");
NR_LAYOUT_ASSERT(offsetof(NrVecU8, cap) == 16, "NrVecU8.cap offset mismatch");
NR_LAYOUT_ASSERT(offsetof(NrVecU8, owned) == 24, "NrVecU8.owned offset mismatch");
NR_LAYOUT_ASSERT(offsetof(NrVecU8, reserved) == 25, "NrVecU8.reserved offset mismatch");
NR_LAYOUT_ASSERT(offsetof(NrVecU8, drop_fn) == 32, "NrVecU8.drop_fn offset mismatch");

NR_LAYOUT_ASSERT(sizeof(NrOwnedBytes) == 32, "NrOwnedBytes size mismatch");
NR_LAYOUT_ASSERT(offsetof(NrOwnedBytes, ptr) == 0, "NrOwnedBytes.ptr offset mismatch");
NR_LAYOUT_ASSERT(offsetof(NrOwnedBytes, len) == 8, "NrOwnedBytes.len offset mismatch");
NR_LAYOUT_ASSERT(offsetof(NrOwnedBytes, owner_ctx) == 16, "NrOwnedBytes.owner_ctx offset mismatch");
NR_LAYOUT_ASSERT(offsetof(NrOwnedBytes, release) == 24, "NrOwnedBytes.release offset mismatch");

NR_LAYOUT_ASSERT(sizeof(NrBufferLease) == 24, "NrBufferLease size mismatch");
NR_LAYOUT_ASSERT(offsetof(NrBufferLease, ptr) == 0, "NrBufferLease.ptr offset mismatch");
NR_LAYOUT_ASSERT(offsetof(NrBufferLease, cap) == 8, "NrBufferLease.cap offset mismatch");
NR_LAYOUT_ASSERT(offsetof(NrBufferLease, token) == 16, "NrBufferLease.token offset mismatch");

NR_LAYOUT_ASSERT(sizeof(NrHostVTable) == 32, "NrHostVTable size mismatch");
NR_LAYOUT_ASSERT(offsetof(NrHostVTable, send_result) == 0, "NrHostVTable.send_result offset mismatch");
NR_LAYOUT_ASSERT(offsetof(NrHostVTable, send_result_owned) == 8, "NrHostVTable.send_result_owned offset mismatch");
NR_LAYOUT_ASSERT(offsetof(NrHostVTable, acquire_result_buffer) == 16, "NrHostVTable.acquire_result_buffer offset mismatch");
NR_LAYOUT_ASSERT(offsetof(NrHostVTable, commit_result_buffer) == 24, "NrHostVTable.commit_result_buffer offset mismatch");

NR_LAYOUT_ASSERT(sizeof(NrPluginVTable) == 56, "NrPluginVTable size mismatch");
NR_LAYOUT_ASSERT(offsetof(NrPluginVTable, init) == 0, "NrPluginVTable.init offset mismatch");
NR_LAYOUT_ASSERT(offsetof(NrPluginVTable, handle) == 8, "NrPluginVTable.handle offset mismatch");
NR_LAYOUT_ASSERT(offsetof(NrPluginVTable, shutdown) == 16, "NrPluginVTable.shutdown offset mismatch");
NR_LAYOUT_ASSERT(offsetof(NrPluginVTable, stream_data) == 24, "NrPluginVTable.stream_data offset mismatch");
NR_LAYOUT_ASSERT(offsetof(NrPluginVTable, stream_close) == 32, "NrPluginVTable.stream_close offset mismatch");
NR_LAYOUT_ASSERT(offsetof(NrPluginVTable, resolve_entry) == 40, "NrPluginVTable.resolve_entry offset mismatch");
NR_LAYOUT_ASSERT(offsetof(NrPluginVTable, handle_by_id) == 48, "NrPluginVTable.handle_by_id offset mismatch");

NR_LAYOUT_ASSERT(sizeof(NrPluginInfo) == 56, "NrPluginInfo size mismatch");
NR_LAYOUT_ASSERT(offsetof(NrPluginInfo, abi_version) == 0, "NrPluginInfo.abi_version offset mismatch");
NR_LAYOUT_ASSERT(offsetof(NrPluginInfo, struct_size) == 4, "NrPluginInfo.struct_size offset mismatch");
NR_LAYOUT_ASSERT(offsetof(NrPluginInfo, name) == 8, "NrPluginInfo.name offset mismatch");
NR_LAYOUT_ASSERT(offsetof(NrPluginInfo, version) == 24, "NrPluginInfo.version offset mismatch");
NR_LAYOUT_ASSERT(offsetof(NrPluginInfo, plugin_ctx) == 40, "NrPluginInfo.plugin_ctx offset mismatch");
NR_LAYOUT_ASSERT(offsetof(NrPluginInfo, vtable) == 48, "NrPluginInfo.vtable offset mismatch");

#endif
