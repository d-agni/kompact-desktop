#!/bin/sh
# Start the whole shell (wallpaper, menu bar, Launchpad, Spotlight) inside kompact, logging to
# kompact/shell.log. Run it from a foot window inside kompact (started with ./ktest.sh).
#   sh ~/Documents/kompact-de/shell/tools/test-shell.sh [wallpaper image]
dir="$(cd "$(dirname "$0")/../.." && pwd)"
image=${1:-/usr/share/backgrounds/cnusr25-Simple_Raccoon_Dark.png}
pkill -f target/release/kompact-shell 2>/dev/null
sleep 1
rm -rf "$dir/kompact/snap-shell"
mkdir -p "$dir/kompact/snap-shell"
RUST_LOG=info KOMPACT_WALLPAPER="$image" KOMPACT_SHELL_SNAPSHOT="$dir/kompact/snap-shell" \
    "$dir/shell/src-tauri/target/release/kompact-shell" > "$dir/kompact/shell.log" 2>&1 &
echo "kompact-shell started (pid $!), log: $dir/kompact/shell.log"
