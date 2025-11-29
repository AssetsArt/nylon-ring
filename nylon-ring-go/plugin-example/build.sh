#!/bin/bash
# Build script for Go plugin

set -e

PLUGIN_NAME="nylon_ring_go_plugin"

echo "Building Go plugin..."

# Build as C shared library
go build -buildmode=c-shared -o "${PLUGIN_NAME}.so" .

echo "✓ Plugin built: ${PLUGIN_NAME}.so"
ls -lh "${PLUGIN_NAME}.so"

