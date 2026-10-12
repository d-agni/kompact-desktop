#!/bin/sh
# Phase 5: after a run with KOMPACT_OUTPUTS set, show what kompact saw of the monitors.
# Read-only; run it after quitting kompact, from any console:
#   sh ~/Documents/kompact-de/kompact/tools/check-outputs.sh [log]     (default: ktest.log)
# 5a (observe): one "connector …" line per connector at startup, again after each
# "udev: display hotplug event" and each VT return; the driven output never changes.
cd "$(dirname "$0")/.." || exit 1
log=${1:-ktest.log}
esc=$(printf '\033')
plain() { sed "s/${esc}\[[0-9;]*m//g" "$log"; }
count() { plain | grep -c "$1"; }

[ -f "$log" ] || { echo "no $log"; exit 1; }
echo "$log ($(plain | head -1 | cut -c1-19) .. $(plain | tail -1 | cut -c1-19)):"
plain | grep -o "outputs mode: .*" | head -1 | sed 's/^/  /'
plain | grep "selected DRM output" | head -1 | sed 's/.*interface=\([A-Za-z]*\) interface_id=\([0-9]*\) crtc=\([^ ]*\).*size: (\([0-9]*\), \([0-9]*\)).*/  driving: \1-\2 on \3, \4x\5/'
plain | grep -o "primary monitor: .*" | head -1 | sed 's/^/  /'
plain | grep -o "pointer starts on .*" | head -1 | sed 's/^/  /'
plain | grep -o "output layout: .*" | tail -1 | sed 's/^/  last /'
plain | grep -q "watching udev for display hotplug" && echo "  udev watch: on" || echo "  udev watch: off (KOMPACT_OUTPUTS=first)"
echo "  hotplug events:        $(count 'udev: display hotplug event')"
echo "  connector scans:       startup $(count '(startup):' ) lines, hotplug $(count '(hotplug):') lines, VT return $(count '(VT return):') lines"
echo "  scans skipped (VT switch under way): $(count 'skipped: a VT switch is under way')"
echo "  failed scans:          $(count 'connector scan (.*) failed')"
echo "  fatal DRM errors:      $(count 'fatal DRM')"
echo "  outputs set up/shut:   $(count 'outputs (')"
echo
echo "Connector lines (time, connector, why, state):"
plain | grep -E "kompact::backend_udev: (connector |udev: |outputs \(|outputs unchanged|could not set up output|output .* (set up|shut down|failed)|output layout|primary monitor is now)" | sed 's/^\(.\{19\}\)[^ ]* *[A-Z]* *kompact::backend_udev: /  \1  /' | cut -c1-150
echo
echo "Windows (5i: moved off an unplugged monitor, and back when it returns):"
plain | grep -E "kompact::shell: output change: " | sed 's/^\(.\{19\}\)[^ ]* *[A-Z]* *kompact::shell: output change: /  \1  /' | cut -c1-150
echo "  moved back: $(count 'output change: moved window .* back to'), left where you put them: $(count 'was moved since, stays off')"
echo
if [ "$(count 'fatal DRM')" -eq 0 ] && [ "$(count 'connector scan (.*) failed')" -eq 0 ]; then
    echo "OK: no fatal DRM error, every scan worked"
else
    echo "PROBLEM: see the lines above"
fi
