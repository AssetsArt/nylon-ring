#!/bin/bash
# Build script for simplified Go plugin using SDK

set -e

PLUGIN_NAME="nylon_ring_go_plugin_simple"

echo "Building Go plugin with SDK..."

# Build as C shared library
go build -buildmode=c-shared -o "${PLUGIN_NAME}.so" .

echo "✓ Plugin built: ${PLUGIN_NAME}.so"
ls -lh "${PLUGIN_NAME}.so"

