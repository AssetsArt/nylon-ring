#ifndef NYLON_RING_H
#define NYLON_RING_H

#include <stddef.h>
#include <stdint.h>

#if defined(_WIN32)
#define NYR_EXPORT __declspec(dllexport)
#else
#define NYR_EXPORT __attribute__((visibility("default")))
#endif

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

typedef NrStatus (*NrSendResultFn)(
    void *host_ctx,
    uint64_t sid,
    NrStatus status,
    NrVecU8 payload
);

typedef struct {
    NrSendResultFn send_result;
} NrHostVTable;

typedef NrStatus (*NrPluginInitFn)(void *host_ctx, const NrHostVTable *host_vtable);
typedef NrStatus (*NrPluginHandleFn)(NrStr entry, uint64_t sid, NrBytes payload);
typedef void (*NrPluginShutdownFn)(void);
typedef NrStatus (*NrPluginStreamDataFn)(uint64_t sid, NrBytes data);
typedef NrStatus (*NrPluginStreamCloseFn)(uint64_t sid);

typedef struct {
    NrPluginInitFn init;
    NrPluginHandleFn handle;
    NrPluginShutdownFn shutdown;
    NrPluginStreamDataFn stream_data;
    NrPluginStreamCloseFn stream_close;
} NrPluginVTable;

typedef struct {
    uint32_t abi_version;
    uint32_t struct_size;
    NrStr name;
    NrStr version;
    void *plugin_ctx;
    const NrPluginVTable *vtable;
} NrPluginInfo;

NYR_EXPORT const NrPluginInfo *nylon_ring_get_plugin_v1(void);

#endif
