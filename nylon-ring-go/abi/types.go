package abi

/*
#include <stdint.h>
#include <stddef.h>
#include <stdlib.h>
#include <string.h>

// NrStatus enum values
#define NR_STATUS_OK 0
#define NR_STATUS_ERR 1
#define NR_STATUS_INVALID 2
#define NR_STATUS_UNSUPPORTED 3
#define NR_STATUS_STREAM_END 4

// ABI version
#define NR_ABI_VERSION 1
*/
import "C"
import (
	"unsafe"
)

// NrStatus represents the status codes for the Nylon Ring ABI.
type NrStatus uint32

const (
	NrStatusOk          NrStatus = C.NR_STATUS_OK
	NrStatusErr         NrStatus = C.NR_STATUS_ERR
	NrStatusInvalid     NrStatus = C.NR_STATUS_INVALID
	NrStatusUnsupported NrStatus = C.NR_STATUS_UNSUPPORTED
	NrStatusStreamEnd   NrStatus = C.NR_STATUS_STREAM_END
)

// NrStr represents a UTF-8 string slice with a pointer and length.
// This struct is C ABI-stable.
// Layout: ptr (8 bytes) + len (4 bytes) + padding (4 bytes) = 16 bytes on 64-bit
// Must match Rust: #[repr(C)] struct NrStr { ptr: *const u8, len: u32 }
type NrStr struct {
	Ptr unsafe.Pointer // *const u8 in Rust
	Len uint32
	_   [4]byte // padding to match C struct alignment (16 bytes total)
}

// NrBytes represents a byte slice with a pointer and length.
// This struct is C ABI-stable.
// Layout: ptr (8 bytes) + len (8 bytes) = 16 bytes on 64-bit
// Must match Rust: #[repr(C)] struct NrBytes { ptr: *const u8, len: u64 }
type NrBytes struct {
	Ptr unsafe.Pointer // *const u8 in Rust
	Len uint64
}

// NrHeader represents a key-value pair of strings.
// Layout: key (16) + value (16) = 32 bytes
type NrHeader struct {
	Key   NrStr
	Value NrStr
}

// NrRequest represents a request with metadata.
// Layout: path (16) + method (16) + query (16) + headers ptr (8) + headers_len (4) + reserved0 (4) + reserved1 (8) = 72 bytes
type NrRequest struct {
	Path       NrStr
	Method     NrStr
	Query      NrStr
	Headers    *NrHeader
	HeadersLen uint32
	Reserved0  uint32
	Reserved1  uint64
}

// NrHostVTable represents the host callback table.
type NrHostVTable struct {
	SendResult func(hostCtx unsafe.Pointer, sid uint64, status NrStatus, payload NrBytes)
}

// NrHostExt represents the host extension table for state management.
type NrHostExt struct {
	SetState func(hostCtx unsafe.Pointer, sid uint64, key NrStr, value NrBytes) NrBytes
	GetState func(hostCtx unsafe.Pointer, sid uint64, key NrStr) NrBytes
}

// NrPluginVTable represents the plugin function table.
type NrPluginVTable struct {
	Init     func(pluginCtx unsafe.Pointer, hostCtx unsafe.Pointer, hostVTable *NrHostVTable) NrStatus
	Handle   func(pluginCtx unsafe.Pointer, entry NrStr, sid uint64, req *NrRequest, payload NrBytes) NrStatus
	Shutdown func(pluginCtx unsafe.Pointer)
}

// NrPluginInfo represents metadata exported by the plugin.
type NrPluginInfo struct {
	AbiVersion uint32
	StructSize uint32
	Name       NrStr
	Version    NrStr
	PluginCtx  unsafe.Pointer
	VTable     *NrPluginVTable
}

// Helper functions

// String converts NrStr to Go string.
func (s NrStr) String() string {
	if s.Ptr == nil || s.Len == 0 {
		return ""
	}
	return C.GoStringN((*C.char)(s.Ptr), C.int(s.Len))
}

// Bytes converts NrBytes to Go []byte.
// Note: This creates a copy of the data.
func (b NrBytes) Bytes() []byte {
	if b.Ptr == nil || b.Len == 0 {
		return nil
	}
	return C.GoBytes(b.Ptr, C.int(b.Len))
}

// FromString creates NrStr from Go string.
// Note: The string must remain valid during the call.
// For plugin callbacks, copy the string first.
func FromString(s string) NrStr {
	if len(s) == 0 {
		return NrStr{}
	}
	// For callbacks, we need to keep the string alive
	// The caller should use a C.CString and keep it alive
	cstr := C.CString(s)
	return NrStr{
		Ptr: unsafe.Pointer(cstr),
		Len: uint32(len(s)),
	}
}

// FromBytes creates NrBytes from Go []byte.
// Note: The bytes must remain valid during the call.
// For plugin callbacks, copy the bytes first.
func FromBytes(b []byte) NrBytes {
	if len(b) == 0 {
		return NrBytes{}
	}
	// Allocate C memory for the bytes
	cbytes := C.malloc(C.size_t(len(b)))
	if cbytes == nil {
		return NrBytes{}
	}
	C.memcpy(cbytes, unsafe.Pointer(&b[0]), C.size_t(len(b)))
	return NrBytes{
		Ptr: cbytes,
		Len: uint64(len(b)),
	}
}

// GetHeaders returns a slice of headers from the request.
func (r *NrRequest) GetHeaders() []NrHeader {
	if r.Headers == nil || r.HeadersLen == 0 {
		return nil
	}
	// Convert C array to Go slice
	headers := (*[1 << 28]NrHeader)(unsafe.Pointer(r.Headers))[:r.HeadersLen:r.HeadersLen]
	result := make([]NrHeader, r.HeadersLen)
	copy(result, headers)
	return result
}
