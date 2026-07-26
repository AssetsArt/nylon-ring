#ifndef NYLON_RING_GO_SHIM_H
#define NYLON_RING_GO_SHIM_H

#include "../../c/nylon_ring.h"

/* Sends a borrowed response through the host's send_result; the host copies
 * the bytes before this returns, so Go-managed memory is safe to pass. */
NrStatus nyr_send_borrowed(uint64_t sid, NrStatus status, const uint8_t *ptr, uint64_t len);

#endif
