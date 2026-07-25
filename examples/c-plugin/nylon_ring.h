#ifndef NYLON_RING_H
#define NYLON_RING_H

#include <stddef.h>
#include <stdint.h>

#if defined(_WIN32)
#define NYR_EXPORT __declspec(dllexport)
#else
#define NYR_EXPORT __attribute__((visibility("default")))
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

#endif
