#!/bin/sh
# Phase 8: after a run, what kompact and the shell logged for 8a-8d. Read-only; run it after
# quitting kompact:  sh ~/Documents/kompact-de/kompact/tools/check-phase8.sh
cd "$(dirname "$0")/.." || exit 1
esc=$(printf '\033')
plain() { [ -f "$1" ] && sed "s/${esc}\[[0-9;]*m//g" "$1"; }
k() { plain ktest.log | grep -E "$1" | sed 's/^\(.\{19\}\)[^ ]* *[A-Z]* *[a-z_:]*: /  \1  /' | cut -c1-160; }
s() { plain shell.log | grep -E "$1" | sed 's/^\(.\{19\}\)[^ ]* *[A-Z]* *[a-z_:]*: /  \1  /' | cut -c1-160; }

echo "ktest.log ($(plain ktest.log | head -1 | cut -c1-60))"
echo
echo "8a activation (apps bring each other forward):"
k "activation: (on|off|token|bringing|request|.* not brought)" | tail -20
s "with an activation token|no activation token" | tail -8
echo
echo "8b cursor shapes:"
k "cursor shapes:"
echo
echo "8c notifications:"
s "notifications: " | tail -25
echo
echo "8d lock screen:"
k "lock screen: " | tail -20
echo
echo "Warnings and errors (kompact):"
plain ktest.log | grep -E " (WARN|ERROR) " | grep -v "Unable to become drm master\|reset-failed\|Failed to reset crtc" | cut -c1-160 | tail -10
echo "(end)"
