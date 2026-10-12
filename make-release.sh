#!/bin/sh
# Builds a kompact desktop release (15d):
#   ./make-release.sh            # the version the code has now (kompact/Cargo.toml)
#   ./make-release.sh 0.2.0      # set this version everywhere first, then build
# Result: dist/kompact-desktop-<version>-x86_64.tar.gz and its .sha256; unpack it and run
# `sudo ./install.sh` (README.md inside). Details: release/make-release.sh.
set -eu
root=$(cd "$(dirname "$0")" && pwd)
if [ $# -gt 1 ]; then
    echo "usage: $0 [version]   (e.g. $0 0.2.0)" >&2
    exit 2
fi
if [ $# = 1 ]; then
    v=$1
    if ! echo "$v" | grep -Eq '^[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.]+)?$'; then
        echo "'$v' isn't a version like 0.2.0 or 0.2.0-beta.1" >&2
        exit 2
    fi
    # The [package] version: the first `version = ` line of each Cargo.toml.
    for f in "$root/kompact/Cargo.toml" "$root/shell/src-tauri/Cargo.toml"; do
        sed -i "0,/^version = \".*\"/s//version = \"$v\"/" "$f"
    done
    sed -i "0,/\"version\": \".*\"/s//\"version\": \"$v\"/" "$root/shell/src-tauri/tauri.conf.json"
    echo "version set to $v (kompact, the shell, tauri.conf.json)"
fi
exec "$root/release/make-release.sh"
