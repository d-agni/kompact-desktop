#!/bin/sh
# Phase 5a: does the kernel notice a VGA (or HDMI) unplug at all? Run it from a text console
# with kompact NOT running. Records kernel DRM events while you unplug and replug one monitor,
# and asks the kernel for the connector's state after each step. Nothing is changed.
#   sh ~/Documents/kompact-de/kompact/tools/watch-hotplug.sh [VGA-1|HDMI-A-1]   (default VGA-1)
# Log: hotplug.log in kompact/.
cd "$(dirname "$0")/.." || exit 1
name=${1:-VGA-1}
conn=/sys/class/drm/card0-$name
[ -e "$conn/status" ] || { echo "no $conn"; exit 1; }
log=hotplug.log
command -v udevadm >/dev/null || { echo "udevadm not found"; exit 1; }

say() { echo "$(date +%T) $*" | tee -a "$log"; }

: > "$log"
udevadm monitor --kernel --subsystem-match=drm >> "$log" 2>&1 &
mon=$!
trap 'kill $mon 2>/dev/null' EXIT INT TERM
sleep 1
say "watching kernel DRM events; $name status now: $(cat "$conn/status")"
say ">>> UNPLUG $name now (20 s, events are logged as they come)"
sleep 20
say "events so far: $(grep -c change "$log") (before anything asked the kernel)"
say "after unplug (reading status makes the kernel check): status $(cat "$conn/status")"
say ">>> PLUG $name back in now (20 s)"
sleep 20
say "after replug (reading status makes the kernel check): status $(cat "$conn/status")"
sleep 1
kill $mon 2>/dev/null
echo
echo "Kernel events recorded (UDEV/KERNEL change lines = hotplug reported):"
grep -c "change" "$log" | sed 's/^/  change events: /'
echo "Full log: $PWD/$log"
