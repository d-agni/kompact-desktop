#!/bin/sh
# Install the system packages needed to build kompact-shell (Phase 6) on Ubuntu 26.04.
# Safe to run again: apt skips what's already installed.
#
# Usage: sh install-deps.sh

set -eu

PACKAGES="pkg-config libwebkitgtk-6.0-dev libgtk4-layer-shell-dev"

echo "Installing: $PACKAGES"
sudo apt-get update
sudo apt-get install -y $PACKAGES

# Check that the build will find the libraries.
echo
fail=0
for lib in webkitgtk-6.0 gtk4 gtk4-layer-shell-0; do
    if version=$(pkg-config --modversion "$lib" 2>/dev/null); then
        echo "ok      $lib $version"
    else
        echo "MISSING $lib"
        fail=1
    fi
done

# WebKit runs its page process in a bubblewrap sandbox.
if command -v bwrap >/dev/null 2>&1; then
    echo "ok      bwrap $(command -v bwrap)"
else
    echo "MISSING bwrap (install the bubblewrap package)"
    fail=1
fi

exit $fail
