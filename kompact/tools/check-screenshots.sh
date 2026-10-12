#!/bin/sh
# 9a/9b: after a run, what kompact logged for screenshots and screencopy, and the files it wrote. Read-only; run it
# after quitting kompact:  sh ~/Documents/kompact-de/kompact/tools/check-screenshots.sh [log]
cd "$(dirname "$0")/.." || exit 1
log=${1:-ktest.log}
esc=$(printf '\033')
plain() { sed "s/${esc}\[[0-9;]*m//g" "$log"; }
[ -f "$log" ] || { echo "no $log"; exit 1; }
echo "$log ($(plain | head -1 | cut -c1-19) .. $(plain | tail -1 | cut -c1-19)):"
plain | grep -E "screenshot(s)?: " | sed 's/^\(.\{19\}\)[^ ]* *[A-Z]* *[a-z_:]*: /  \1  /' | cut -c1-170
echo
echo "  keys: $(plain | grep -c 'screenshot: .* asked by key'), read back: $(plain | grep -c 'screenshot: .* read back in'), saved: $(plain | grep -c 'screenshot: saved'), refused (locked): $(plain | grep -c 'screenshot: refused')"
plain | grep -o "read back in [0-9.]* ms" | awk '{ms=$4; n++; s+=ms; if(ms>m)m=ms} END {if(n) printf "  readback: %d, average %.1f ms, slowest %.1f ms\n", n, s/n, m}'
echo
echo "9b screencopy:"
plain | grep -E "screencopy: " | sed 's/^\(.\{19\}\)[^ ]* *[A-Z]* *[a-z_:]*: /  \1  /' | cut -c1-170 | tail -30
echo "  offered: $(plain | grep -c 'screencopy: capture of'), copied: $(plain | grep -c 'screencopy: copied'), refused/failed: $(plain | grep -cE 'screencopy: (capture refused|refused|.* failed|.*gone)')"
echo
dir=${KOMPACT_SCREENSHOT_DIR:-$HOME/Pictures/Screenshots}
echo "Newest files in $dir:"
ls -lt "$dir" 2>/dev/null | head -7 | sed 's/^/  /'
if command -v file >/dev/null; then
    ls -t "$dir"/*.png 2>/dev/null | head -4 | while read -r f; do echo "  $(file -b "$f" | cut -c1-60)  $(basename "$f")"; done
fi
echo
echo "9c in the shell (selection, banners; shell.log):"
if [ -f shell.log ]; then
    sed "s/${esc}\[[0-9;]*m//g" shell.log | grep -E "overlay: (select|screenshot region)|select (opened|closed)|compositor: (key select|screenshot saved|sending shot)|notifications: .*(screenshot|opening|from kompact)" | sed 's/^\(.\{19\}\)[^ ]* *[A-Z]* *[a-z_:]*: /  \1  /' | cut -c1-170 | tail -25
else
    echo "  no shell.log"
fi
echo "  regions asked: $(plain | grep -c 'screenshot: region of'), waited for the selection too long: $(plain | grep -c 'selection is still up')"
echo
echo "Warnings and errors (kompact):"
plain | grep -E " (WARN|ERROR) " | grep -v "Unable to become drm master\|reset-failed\|Failed to reset crtc" | cut -c1-160 | tail -10
plain | grep -c "fatal DRM" | sed 's/^/  fatal DRM errors: /'
echo "(end)"
