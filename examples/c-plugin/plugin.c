#include "nylon_ring.h"

#include <string.h>

static void *g_host_ctx;
static const NrHostVTable *g_host_vtable;

static NrStatus plugin_init(void *host_ctx, const NrHostVTable *host_vtable) {
    if (host_ctx == NULL || host_vtable == NULL || host_vtable->send_result == NULL) {
        return NR_INVALID;
    }
    g_host_ctx = host_ctx;
    g_host_vtable = host_vtable;
    return NR_OK;
}

static NrStatus plugin_handle(NrStr entry, uint64_t sid, NrBytes payload) {
    static const char echo[] = "echo";
    if (entry.len != sizeof(echo) - 1 || memcmp(entry.ptr, echo, sizeof(echo) - 1) != 0) {
        return NR_INVALID;
    }

    NrVecU8 response = {
        .ptr = (uint8_t *)payload.ptr,
        .len = (size_t)payload.len,
        .cap = (size_t)payload.len,
        .owned = 0,
        .reserved = {0},
        .drop_fn = NULL,
    };
    return g_host_vtable->send_result(g_host_ctx, sid, NR_OK, response);
}

static void plugin_shutdown(void) {
    g_host_vtable = NULL;
    g_host_ctx = NULL;
}

static const NrPluginVTable VTABLE = {
    .init = plugin_init,
    .handle = plugin_handle,
    .shutdown = plugin_shutdown,
    .stream_data = NULL,
    .stream_close = NULL,
};

static const NrPluginInfo INFO = {
    .abi_version = 1,
    .struct_size = sizeof(NrPluginInfo),
    .name = {(const uint8_t *)"nylon-ring-c-example", 20, 0},
    .version = {(const uint8_t *)"0.1.0", 5, 0},
    .plugin_ctx = NULL,
    .vtable = &VTABLE,
};

_Static_assert(sizeof(NrStatus) == 4, "NrStatus layout mismatch");
_Static_assert(sizeof(NrStr) == 16, "NrStr layout mismatch");
_Static_assert(sizeof(NrBytes) == 16, "NrBytes layout mismatch");
_Static_assert(sizeof(NrVecU8) == 40, "NrVecU8 layout mismatch");

NYR_EXPORT const NrPluginInfo *nylon_ring_get_plugin_v1(void) {
    return &INFO;
}
