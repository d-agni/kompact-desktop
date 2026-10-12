#!/bin/sh
# 6p-b: check that SIGTERM shuts kompact down cleanly, like Super+Escape.
# Start kompact on one console (./ktest-desktop.sh), then run this from another (Ctrl+Alt+F3):
#   sh ~/Documents/kompact-de/kompact/tools/test-clean-quit.sh
log="$(cd "$(dirname "$0")/.." && pwd)/ktest.log"
pass=0
fail=0
ok() { echo "  PASS $1"; pass=$((pass + 1)); }
bad() { echo "  FAIL $1"; fail=$((fail + 1)); }

pid=$(pgrep -x kompact | head -1)
if [ -z "$pid" ]; then
    echo "kompact isn't running; start it with ./ktest-desktop.sh on another console first"
    exit 1
fi
echo "kompact is pid $pid; sending SIGTERM"
kill -TERM "$pid"

i=0
while kill -0 "$pid" 2>/dev/null && [ "$i" -lt 50 ]; do
    sleep 0.1
    i=$((i + 1))
done
if kill -0 "$pid" 2>/dev/null; then
    bad "kompact still running after 5 s (a second 'kill -TERM $pid' ends it at once)"
else
    ok "kompact exited after about $((i / 10)).$((i % 10)) s"
fi

sleep 1
plain() { sed "s/$(printf '\033')\[[0-9;]*m//g" "$log"; }
if plain | grep -q "SIGTERM received, shutting down"; then ok "it shut down on purpose"; else bad "no 'SIGTERM received, shutting down' in ktest.log"; fi
if plain | grep -q "remembered geometry for"; then ok "window geometry remembered"; else bad "no 'remembered geometry' (were any windows open?)"; fi
if plain | grep -q "stopped the portals before closing the display"; then ok "portals stopped"; else bad "portals not stopped"; fi
if pgrep -x kompact-shell >/dev/null; then bad "kompact-shell is still running"; else ok "no kompact-shell left"; fi
if systemctl --user show-environment 2>/dev/null | grep -q "^WAYLAND_DISPLAY="; then
    bad "WAYLAND_DISPLAY still exported to the user session"
else
    ok "WAYLAND_DISPLAY withdrawn from the user session"
fi

echo
echo "$pass passed, $fail failed"
echo "Switch back to kompact's console (Ctrl+Alt+F2 or wherever it ran); if it stays black, switch away and back."
[ "$fail" -eq 0 ]
