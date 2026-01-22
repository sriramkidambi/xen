#!/bin/bash
# Build and install xen binary
# Replaces the existing xen binary with the newly built one

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(dirname "$SCRIPT_DIR")"
INSTALL_DIR="${CARGO_HOME:-$HOME/.cargo}/bin"

cd "$PROJECT_DIR"

echo "Building xen (release)..."
cargo build --release

echo "Installing to $INSTALL_DIR/xen..."
cp target/release/xen "$INSTALL_DIR/xen"

echo "Done! Installed version:"
xen --version
