#!/bin/sh
# VT-switch fix (2026-10-10): after a run with VT switches, check kompact's log and seatd.
# Read-only; run it after quitting kompact, from any console:
#   sh ~/Documents/kompact-de/kompact/tools/check-vt.sh [log]     (default: ktest.log)
# Every switch away should show "seat paused" and every return "DRM device reactivated";
# a "vt: page flip refused" line means the race happened and was survived. "fatal DRM" or a
# seatd assertion means the fix didn't hold.
cd "$(dirname "$0")/.." || exit 1
log=${1:-ktest.log}
esc=$(printf '\033')
plain() { sed "s/${esc}\[[0-9;]*m//g" "$log"; }
count() { plain | grep -c "$1"; }

[ -f "$log" ] || { echo "no $log"; exit 1; }
echo "$log ($(plain | head -1 | cut -c1-19) .. $(plain | tail -1 | cut -c1-19)):"
echo "  switch requests (switching to VT): $(count 'switching to VT')"
echo "  seat paused:                        $(count 'seat paused')"
echo "  DRM device reactivated:             $(count 'DRM device reactivated')"
echo "  flips refused and survived:         $(count 'vt: page flip refused')"
echo "  gate timeouts (no seat answer):     $(count 'vt: no answer from the seat')"
echo "  fatal DRM errors:                   $(count 'fatal DRM')"
echo "  failed reactivations:               $(count 'failed to reactivate DRM device')"
crashes=$(journalctl -b -q --no-pager -u seatd 2>/dev/null | grep -c "Assertion")
echo "  seatd assertion failures this boot: $crashes"
if [ "$(count 'fatal DRM')" -eq 0 ] && [ "$crashes" -eq 0 ]; then
    echo "OK: no fatal DRM error, seatd didn't crash"
else
    echo "PROBLEM: see the lines above (and: journalctl -b -u seatd)"
fi
