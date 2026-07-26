//! A minimal nylon-ring plugin in Zig, mirroring c/nylon_ring.h with
//! extern structs. The comptime block below repeats the header's layout
//! contract, so any drift fails this build too.

const std = @import("std");

const NrStatus = u32;
const NR_OK: NrStatus = 0;
const NR_INVALID: NrStatus = 2;

const NR_ABI_VERSION: u32 = 2;

const NrStr = extern struct {
    ptr: ?[*]const u8,
    len: u32,
    reserved: u32,
};

const NrBytes = extern struct {
    ptr: ?[*]const u8,
    len: u64,
};

const NrVecU8DropFn = ?*const fn (?[*]u8, usize, usize) callconv(.c) void;

const NrVecU8 = extern struct {
    ptr: ?[*]u8,
    len: usize,
    cap: usize,
    owned: u8,
    reserved: [7]u8,
    drop_fn: NrVecU8DropFn,
};

const NrOwnedBytes = extern struct {
    ptr: ?[*]const u8,
    len: u64,
    owner_ctx: ?*anyopaque,
    release: ?*const fn (?*anyopaque, ?[*]const u8, u64) callconv(.c) void,
};

const NrBufferLease = extern struct {
    ptr: ?[*]u8,
    cap: u64,
    token: u64,
};

const NrHostVTable = extern struct {
    send_result: ?*const fn (?*anyopaque, u64, NrStatus, NrVecU8) callconv(.c) NrStatus,
    send_result_owned: ?*const fn (?*anyopaque, u64, NrStatus, NrOwnedBytes) callconv(.c) NrStatus,
    acquire_result_buffer: ?*const fn (?*anyopaque, u64, u64) callconv(.c) NrBufferLease,
    commit_result_buffer: ?*const fn (?*anyopaque, u64, NrStatus, u64, u64) callconv(.c) NrStatus,
};

const NrPluginVTable = extern struct {
    init: ?*const fn (?*anyopaque, ?*const NrHostVTable) callconv(.c) NrStatus,
    handle: ?*const fn (NrStr, u64, NrBytes) callconv(.c) NrStatus,
    shutdown: ?*const fn () callconv(.c) void,
    stream_data: ?*const fn (u64, NrBytes) callconv(.c) NrStatus,
    stream_close: ?*const fn (u64) callconv(.c) NrStatus,
    resolve_entry: ?*const fn (NrStr) callconv(.c) u32,
    handle_by_id: ?*const fn (u32, u64, NrBytes) callconv(.c) NrStatus,
};

const NrPluginInfo = extern struct {
    abi_version: u32,
    struct_size: u32,
    name: NrStr,
    version: NrStr,
    plugin_ctx: ?*anyopaque,
    vtable: ?*const NrPluginVTable,
};

// Layout contract — same numbers as the assert block in c/nylon_ring.h.
comptime {
    std.debug.assert(@sizeOf(usize) == 8); // 64-bit targets only
    std.debug.assert(@sizeOf(NrStatus) == 4);

    std.debug.assert(@sizeOf(NrStr) == 16);
    std.debug.assert(@offsetOf(NrStr, "ptr") == 0);
    std.debug.assert(@offsetOf(NrStr, "len") == 8);
    std.debug.assert(@offsetOf(NrStr, "reserved") == 12);

    std.debug.assert(@sizeOf(NrBytes) == 16);
    std.debug.assert(@offsetOf(NrBytes, "ptr") == 0);
    std.debug.assert(@offsetOf(NrBytes, "len") == 8);

    std.debug.assert(@sizeOf(NrVecU8) == 40);
    std.debug.assert(@offsetOf(NrVecU8, "ptr") == 0);
    std.debug.assert(@offsetOf(NrVecU8, "len") == 8);
    std.debug.assert(@offsetOf(NrVecU8, "cap") == 16);
    std.debug.assert(@offsetOf(NrVecU8, "owned") == 24);
    std.debug.assert(@offsetOf(NrVecU8, "reserved") == 25);
    std.debug.assert(@offsetOf(NrVecU8, "drop_fn") == 32);

    std.debug.assert(@sizeOf(NrOwnedBytes) == 32);
    std.debug.assert(@offsetOf(NrOwnedBytes, "ptr") == 0);
    std.debug.assert(@offsetOf(NrOwnedBytes, "len") == 8);
    std.debug.assert(@offsetOf(NrOwnedBytes, "owner_ctx") == 16);
    std.debug.assert(@offsetOf(NrOwnedBytes, "release") == 24);

    std.debug.assert(@sizeOf(NrBufferLease) == 24);
    std.debug.assert(@offsetOf(NrBufferLease, "ptr") == 0);
    std.debug.assert(@offsetOf(NrBufferLease, "cap") == 8);
    std.debug.assert(@offsetOf(NrBufferLease, "token") == 16);

    std.debug.assert(@sizeOf(NrHostVTable) == 32);
    std.debug.assert(@offsetOf(NrHostVTable, "send_result") == 0);
    std.debug.assert(@offsetOf(NrHostVTable, "send_result_owned") == 8);
    std.debug.assert(@offsetOf(NrHostVTable, "acquire_result_buffer") == 16);
    std.debug.assert(@offsetOf(NrHostVTable, "commit_result_buffer") == 24);

    std.debug.assert(@sizeOf(NrPluginVTable) == 56);
    std.debug.assert(@offsetOf(NrPluginVTable, "init") == 0);
    std.debug.assert(@offsetOf(NrPluginVTable, "handle") == 8);
    std.debug.assert(@offsetOf(NrPluginVTable, "shutdown") == 16);
    std.debug.assert(@offsetOf(NrPluginVTable, "stream_data") == 24);
    std.debug.assert(@offsetOf(NrPluginVTable, "stream_close") == 32);
    std.debug.assert(@offsetOf(NrPluginVTable, "resolve_entry") == 40);
    std.debug.assert(@offsetOf(NrPluginVTable, "handle_by_id") == 48);

    std.debug.assert(@sizeOf(NrPluginInfo) == 56);
    std.debug.assert(@offsetOf(NrPluginInfo, "abi_version") == 0);
    std.debug.assert(@offsetOf(NrPluginInfo, "struct_size") == 4);
    std.debug.assert(@offsetOf(NrPluginInfo, "name") == 8);
    std.debug.assert(@offsetOf(NrPluginInfo, "version") == 24);
    std.debug.assert(@offsetOf(NrPluginInfo, "plugin_ctx") == 40);
    std.debug.assert(@offsetOf(NrPluginInfo, "vtable") == 48);
}

var g_host_ctx: ?*anyopaque = null;
var g_host_vtable: ?*const NrHostVTable = null;

fn pluginInit(host_ctx: ?*anyopaque, host_vtable: ?*const NrHostVTable) callconv(.c) NrStatus {
    const vt = host_vtable orelse return NR_INVALID;
    if (host_ctx == null or vt.send_result == null) return NR_INVALID;
    g_host_ctx = host_ctx;
    g_host_vtable = vt;
    return NR_OK;
}

fn pluginHandle(entry: NrStr, sid: u64, payload: NrBytes) callconv(.c) NrStatus {
    const vt = g_host_vtable orelse return NR_INVALID;
    const send = vt.send_result orelse return NR_INVALID;
    const entry_ptr = entry.ptr orelse return NR_INVALID;
    if (!std.mem.eql(u8, entry_ptr[0..entry.len], "echo")) return NR_INVALID;

    // Borrowed response (owned = 0): the host copies the bytes before
    // send_result returns, so echoing the request memory back is safe.
    const response = NrVecU8{
        .ptr = @constCast(payload.ptr),
        .len = @intCast(payload.len),
        .cap = @intCast(payload.len),
        .owned = 0,
        .reserved = @splat(0),
        .drop_fn = null,
    };
    return send(g_host_ctx, sid, NR_OK, response);
}

fn pluginShutdown() callconv(.c) void {
    g_host_vtable = null;
    g_host_ctx = null;
}

const VTABLE = NrPluginVTable{
    .init = &pluginInit,
    .handle = &pluginHandle,
    .shutdown = &pluginShutdown,
    .stream_data = null,
    .stream_close = null,
    .resolve_entry = null,
    .handle_by_id = null,
};

const INFO = NrPluginInfo{
    .abi_version = NR_ABI_VERSION,
    .struct_size = @sizeOf(NrPluginInfo),
    .name = .{ .ptr = "nylon-ring-zig-example", .len = 22, .reserved = 0 },
    .version = .{ .ptr = "0.1.0", .len = 5, .reserved = 0 },
    .plugin_ctx = null,
    .vtable = &VTABLE,
};

export fn nylon_ring_get_plugin() ?*const NrPluginInfo {
    return &INFO;
}
