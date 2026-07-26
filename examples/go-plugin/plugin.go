// A minimal nylon-ring plugin in Go, built as a c-shared library.
//
// The C shim (shim.c) owns the vtable and entry-name dispatch; the handler
// body runs in Go. IMPORTANT: the Go runtime cannot be unloaded from a
// process, so hosts must load this plugin with load_pinned — a regular
// load would dlclose the library on unload/drop and crash.
package main

/*
#include "shim.h"
*/
import "C"

import "unsafe"

//export goHandleEcho
func goHandleEcho(sid C.uint64_t, ptr *C.uint8_t, length C.uint64_t) C.NrStatus {
	// Borrow the request bytes, then answer from a Go-owned buffer. Passing
	// a Go pointer into C here is allowed by the cgo rules because the host
	// copies the response before send_result returns.
	var response []byte
	if length > 0 {
		request := unsafe.Slice((*byte)(unsafe.Pointer(ptr)), int(length))
		response = append(response, request...)
	}

	var out *C.uint8_t
	if len(response) > 0 {
		out = (*C.uint8_t)(unsafe.Pointer(&response[0]))
	}
	return C.nyr_send_borrowed(sid, C.NrStatus(C.NR_OK), out, C.uint64_t(len(response)))
}

func main() {}
