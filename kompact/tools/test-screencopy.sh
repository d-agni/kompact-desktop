#!/usr/bin/env bash
# 9b: check wlr-screencopy from inside kompact (run it in foot). Needs grim (and slurp for the
# last, interactive test): `sudo apt install grim slurp`. Writes PNGs to /tmp/kompact-9b only.
set -u

if [ -z "${WAYLAND_DISPLAY:-}" ]; then
    echo "Run this in a terminal inside kompact (WAYLAND_DISPLAY is not set)."
    exit 1
fi
command -v grim >/dev/null || { echo "grim not found: sudo apt install grim slurp"; exit 1; }

out=/tmp/kompact-9b
mkdir -p "$out"
rm -f "$out"/*.png
ok=0
fail=0
size() { # file -> "WxH"
    file -b "$1" 2>/dev/null | sed -n 's/^PNG image data, \([0-9]*\) x \([0-9]*\),.*/\1x\2/p'
}
check() { # name, expected size, file, grim args...
    local name=$1 want=$2 f=$out/$3
    shift 3
    if timeout 10 grim "$@" "$f" 2>"$f.err" && [ "$(size "$f")" = "$want" ]; then
        echo "  OK    $name ($want)"
        ok=$((ok + 1))
    else
        echo "  FAIL  $name (expected $want, got '$(size "$f")'; $(head -c 200 "$f.err"))"
        fail=$((fail + 1))
    fi
    rm -f "$f.err"
}

echo "Screencopy on $WAYLAND_DISPLAY (files in $out):"
check "whole desktop (both monitors side by side)" 2806x900 all.png
check "one monitor: VGA-1" 1440x900 vga.png -o VGA-1
check "one monitor: HDMIA-1" 1366x768 hdmi.png -o HDMIA-1
check "a region on VGA-1" 300x200 region.png -g "1466,100 300x200"
check "a region across both monitors" 200x100 across.png -g "1266,100 200x100"
check "with the cursor (-c)" 1440x900 cursor.png -c -o VGA-1
sleep 0.2
echo
echo "$ok passed, $fail failed."
echo
if command -v slurp >/dev/null; then
    echo "Now drag out a region with the mouse (Esc to skip):"
    if geo=$(slurp 2>/dev/null) && grim -g "$geo" "$out/slurp.png"; then
        echo "  saved $out/slurp.png ($(size "$out/slurp.png"), asked for $geo)"
    else
        echo "  skipped"
    fi
fi
echo
echo "Open the PNGs (e.g. 'xdg-open $out/vga.png') and check: right way up, right colours,"
echo "no cursor except in cursor.png, the region ones show what was at that spot."
echo "Then lock (Super+L), and from another VT or before locking run:"
echo "  sleep 5; grim $out/locked.png    # should fail while locked"
[ "$fail" -eq 0 ]
