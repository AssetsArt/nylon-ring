#!/bin/bash
# Build script for simplified Go plugin using SDK

set -e

# Create target directory
mkdir -p ./../../target/go

# Detect OS for plugin library extension
UNAME_S=$(uname -s)
if [ "$UNAME_S" = "Linux" ]; then
	PLUGIN_EXT=".so"
elif [ "$UNAME_S" = "Darwin" ]; then
	PLUGIN_EXT=".dylib"
elif [ "$OS" = "Windows_NT" ]; then
	PLUGIN_EXT=".dll"
else
	PLUGIN_EXT=".so"
fi

PLUGIN_NAME="./../../target/go/nylon_ring_go_plugin_simple"

echo "Building Go plugin with SDK..."

# Build as C shared library
go build -buildmode=c-shared -o "${PLUGIN_NAME}${PLUGIN_EXT}" .

echo "✓ Plugin built: ${PLUGIN_NAME}${PLUGIN_EXT}"
ls -lh "${PLUGIN_NAME}${PLUGIN_EXT}"

