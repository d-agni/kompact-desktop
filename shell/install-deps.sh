#!/bin/sh
# Install the system packages needed to build the Tauri kompact-shell on Ubuntu 26.04.
# Safe to run again: apt skips what's already installed.
#
# Usage: sh install-deps.sh

set -eu

PACKAGES="pkg-config libwebkit2gtk-4.1-dev libjavascriptcoregtk-4.1-dev libgtk-3-dev libgtk-layer-shell-dev librsvg2-dev libsoup-3.0-dev"

echo "Installing: $PACKAGES"
sudo apt-get update
sudo apt-get install -y $PACKAGES

echo
fail=0
for lib in webkit2gtk-4.1 javascriptcoregtk-4.1 gtk+-3.0 gtk-layer-shell-0 librsvg-2.0 libsoup-3.0; do
    if version=$(pkg-config --modversion "$lib" 2>/dev/null); then
        echo "ok      $lib $version"
    else
        echo "MISSING $lib"
        fail=1
    fi
done
exit $fail
