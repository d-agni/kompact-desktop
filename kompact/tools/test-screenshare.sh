#!/bin/sh
# 9e-b: screen sharing in Firefox, the way a web video call does it (no internet needed).
# Serves tools/screenshare/ on http://localhost:8765 (localhost counts as secure, which
# getDisplayMedia needs) and opens it in Firefox. Press Enter here when done.
if [ -z "${WAYLAND_DISPLAY:-}" ]; then
    echo "Run this in a terminal inside kompact (WAYLAND_DISPLAY is not set)."
    exit 1
fi
here=$(cd "$(dirname "$0")" && pwd)
python3 -I -m http.server 8765 --bind 127.0.0.1 --directory "$here/screenshare" >/tmp/kompact-9e-http.log 2>&1 &
server=$!
sleep 0.5
echo "Opening http://localhost:8765/ in Firefox: click 'Share a screen', allow it, click a monitor."
firefox --new-window http://localhost:8765/ >/dev/null 2>&1 &
printf "Press Enter when done... "
read -r _
kill "$server" 2>/dev/null
echo "stopped the test server"
