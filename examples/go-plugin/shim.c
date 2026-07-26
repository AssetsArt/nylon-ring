#include "shim.h"

#include <string.h>

static void *g_host_ctx;
static const NrHostVTable *g_host_vtable;

/* Implemented in Go (plugin.go). */
extern NrStatus goHandleEcho(uint64_t sid, const uint8_t *ptr, uint64_t len);

static NrStatus shim_init(void *host_ctx, const NrHostVTable *host_vtable) {
    if (host_ctx == NULL || host_vtable == NULL || host_vtable->send_result == NULL) {
        return NR_INVALID;
    }
    g_host_ctx = host_ctx;
    g_host_vtable = host_vtable;
    return NR_OK;
}

static NrStatus shim_handle(NrStr entry, uint64_t sid, NrBytes payload) {
    static const char echo[] = "echo";
    if (entry.len != sizeof(echo) - 1 || memcmp(entry.ptr, echo, sizeof(echo) - 1) != 0) {
        return NR_INVALID;
    }
    return goHandleEcho(sid, payload.ptr, payload.len);
}

static void shim_shutdown(void) {
    g_host_vtable = NULL;
    g_host_ctx = NULL;
}

NrStatus nyr_send_borrowed(uint64_t sid, NrStatus status, const uint8_t *ptr, uint64_t len) {
    NrVecU8 payload = {
        .ptr = (uint8_t *)ptr,
        .len = (size_t)len,
        .cap = (size_t)len,
        .owned = 0,
        .reserved = {0},
        .drop_fn = NULL,
    };
    return g_host_vtable->send_result(g_host_ctx, sid, status, payload);
}

static const NrPluginVTable VTABLE = {
    .init = shim_init,
    .handle = shim_handle,
    .shutdown = shim_shutdown,
    .stream_data = NULL,
    .stream_close = NULL,
    .resolve_entry = NULL,
    .handle_by_id = NULL,
};

static const NrPluginInfo INFO = {
    .abi_version = NR_ABI_VERSION,
    .struct_size = sizeof(NrPluginInfo),
    .name = {(const uint8_t *)"nylon-ring-go-example", 21, 0},
    .version = {(const uint8_t *)"0.1.0", 5, 0},
    .plugin_ctx = NULL,
    .vtable = &VTABLE,
};

NYR_EXPORT const NrPluginInfo *nylon_ring_get_plugin(void) {
    return &INFO;
}
