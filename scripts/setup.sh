#!/bin/bash
# Move the xen binary from the current directory to a directory on PATH.
# Usage:
#   cd ~/Downloads && ./setup.sh
#   cd ~/Downloads/xen-v0.3.0-aarch64-apple-darwin && ./setup.sh
#
# After downloading a release tarball:
#   tar xzf xen-v*.tar.gz
#   cd xen-v*/
#   ./setup.sh

set -e

BINARY="xen"
INSTALL_DIR="/usr/local/bin"

if [ ! -f "$BINARY" ]; then
  echo "Error: '$BINARY' not found in the current directory."
  echo "Make sure you're in the directory containing the xen binary."
  exit 1
fi

if [ ! -x "$BINARY" ]; then
  chmod +x "$BINARY"
fi

echo "Installing xen to $INSTALL_DIR..."

if [ -w "$INSTALL_DIR" ]; then
  mv "$BINARY" "$INSTALL_DIR/$BINARY"
else
  sudo mv "$BINARY" "$INSTALL_DIR/$BINARY"
fi

echo "Done! Installed:"
xen --version
