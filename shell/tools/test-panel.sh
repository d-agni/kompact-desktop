#!/bin/sh
# 6t-a: start the Tauri panel inside kompact and log to kompact/shell.log.
# Run from a foot window inside kompact (started with ./ktest.sh).
dir="$(cd "$(dirname "$0")/../.." && pwd)"
pkill -f target/release/kompact-shell 2>/dev/null
RUST_LOG=info "$dir/shell/src-tauri/target/release/kompact-shell" > "$dir/kompact/shell.log" 2>&1 &
echo "kompact-shell started (pid $!), log: $dir/kompact/shell.log"
