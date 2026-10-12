#!/bin/sh
# 10a: what kompact's keymap loaded and which shortcuts ran, from ktest.log.
#   sh tools/check-keys.sh            print it
#   sh tools/check-keys.sh macos      print it and keep a copy in screenlog/keys-macos.log
here=$(cd "$(dirname "$0")/.." && pwd)
log=${KTEST_LOG:-$here/ktest.log}
report() {
    strip() { sed 's/\x1b\[[0-9;]*m//g' "$log"; }
    echo "== run: $(head -1 "$log" | cut -c1-120)"
    echo "== keymap at startup"
    strip | grep -E "kompact::keymap: keys:" | sed 's/.*keys: /  /'
    echo "== shortcuts used (in order)"
    strip | grep -E "kompact::input: key: " | sed 's/^\([0-9T:.-]*\)Z.*key: /  \1  /' | cut -c1-90
    echo "== warnings"
    strip | grep -E " WARN | ERROR " | grep -v "drm master\|reset-failed\|Failed to reset crtc" | sed 's/^/  /' | cut -c1-200
}
if [ -n "${1:-}" ]; then
    mkdir -p "$here/screenlog"
    report | tee "$here/screenlog/keys-$1.log"
    echo "(saved screenlog/keys-$1.log)"
else
    report
fi
