#include "../../c/nylon_ring.h"

#include <cstdint>
#include <string_view>

namespace {
void *g_host_ctx = nullptr;
const NrHostVTable *g_host_vtable = nullptr;
} // namespace

extern "C" {

static NrStatus plugin_init(void *host_ctx, const NrHostVTable *host_vtable) {
    if (host_ctx == nullptr || host_vtable == nullptr || host_vtable->send_result == nullptr) {
        return NR_INVALID;
    }
    g_host_ctx = host_ctx;
    g_host_vtable = host_vtable;
    return NR_OK;
}

static NrStatus plugin_handle(NrStr entry, uint64_t sid, NrBytes payload) {
    const std::string_view name(reinterpret_cast<const char *>(entry.ptr), entry.len);
    if (name != "echo") {
        return NR_INVALID;
    }

    NrVecU8 response{};
    response.ptr = const_cast<uint8_t *>(payload.ptr);
    response.len = static_cast<size_t>(payload.len);
    response.cap = static_cast<size_t>(payload.len);
    response.owned = 0;
    response.drop_fn = nullptr;
    return g_host_vtable->send_result(g_host_ctx, sid, NR_OK, response);
}

static void plugin_shutdown(void) {
    g_host_vtable = nullptr;
    g_host_ctx = nullptr;
}

} // extern "C"

namespace {

constexpr NrPluginVTable VTABLE = {
    plugin_init,
    plugin_handle,
    plugin_shutdown,
    nullptr, // stream_data
    nullptr, // stream_close
    nullptr, // resolve_entry
    nullptr, // handle_by_id
};

const NrPluginInfo INFO = {
    NR_ABI_VERSION,
    static_cast<uint32_t>(sizeof(NrPluginInfo)),
    {reinterpret_cast<const uint8_t *>("nylon-ring-cpp-example"), 22, 0},
    {reinterpret_cast<const uint8_t *>("0.1.0"), 5, 0},
    nullptr,
    &VTABLE,
};

} // namespace

extern "C" NYR_EXPORT const NrPluginInfo *nylon_ring_get_plugin(void) {
    return &INFO;
}
