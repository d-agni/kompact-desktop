#!/usr/bin/env bash
# 7c: check wlr-data-control from inside kompact (run it in foot). Needs wl-clipboard
# (`sudo apt install wl-clipboard`). It replaces what's on the clipboard and in the primary
# selection with a test marker; nothing else is changed.
set -u

if [ -z "${WAYLAND_DISPLAY:-}" ]; then
    echo "Run this in a terminal inside kompact (WAYLAND_DISPLAY is not set)."
    exit 1
fi
for tool in wl-copy wl-paste; do
    command -v "$tool" >/dev/null || { echo "$tool not found: sudo apt install wl-clipboard"; exit 1; }
done

ok=0
fail=0
check() { # name, expected, got
    if [ "$2" = "$3" ]; then
        echo "  OK    $1"
        ok=$((ok + 1))
    else
        echo "  FAIL  $1 (expected '$2', got '$3')"
        fail=$((fail + 1))
    fi
}

marker="kompact-7c-$$-$(date +%s)"
echo "Clipboard tools on $WAYLAND_DISPLAY:"

# wl-copy forks and serves the selection until something else takes it.
printf '%s' "$marker" | wl-copy
sleep 0.3
check "wl-copy then wl-paste (clipboard)" "$marker" "$(timeout 3 wl-paste --no-newline 2>&1)"

printf '%s' "$marker-primary" | wl-copy --primary
sleep 0.3
check "wl-copy then wl-paste (primary selection)" "$marker-primary" "$(timeout 3 wl-paste --primary --no-newline 2>&1)"
check "the clipboard kept its own value" "$marker" "$(timeout 3 wl-paste --no-newline 2>&1)"

wl-copy --clear
sleep 0.3
got=$(timeout 3 wl-paste --no-newline 2>&1)
case "$got" in
    "$marker") check "wl-copy --clear" "(empty)" "$got" ;;
    *) check "wl-copy --clear" "x" "x" ;;
esac

echo
echo "$ok passed, $fail failed."
echo "Now by hand: run 'echo hello | wl-copy' and paste with Ctrl+V in Firefox;"
echo "copy something in Firefox and run 'wl-paste' here."
[ "$fail" -eq 0 ]
