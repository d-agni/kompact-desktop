#!/bin/sh
# 6b test, points 5 and 6: start kompact-shell three times (no wallpaper, gradient, image) and
# record the memory of the shell and its WebKit processes after each start.
#
# Run it in foot, inside kompact:
#   sh ~/Documents/kompact-de/shell/tools/wallpaper-test.sh [image]
#
# Writes to ~/Documents/kompact-de/kompact/:
#   wallpaper-test.log         memory per run (the summary to send back)
#   shell-none.log, shell-gradient.log, shell-image.log   the shell's own log for each run
# The image run is left running at the end, so you can check the image on screen.

set -u

BASE=$HOME/Documents/kompact-de
SHELL_BIN=$BASE/shell/target/release/kompact-shell
OUT=$BASE/kompact
LOG=$OUT/wallpaper-test.log
IMAGE=${1:-/usr/share/backgrounds/cnusr25-Simple_Raccoon_Dark.png}

if [ -z "${WAYLAND_DISPLAY:-}" ]; then
    echo "WAYLAND_DISPLAY is not set: run this in foot, inside kompact." >&2
    exit 1
fi
[ -x "$SHELL_BIN" ] || { echo "can't run $SHELL_BIN (build it first)" >&2; exit 1; }
[ -r "$IMAGE" ] || { echo "can't read image $IMAGE" >&2; exit 1; }

# Memory of one process: RSS (counts shared libraries in full) and PSS (shares them out).
kib() { awk -v k="$2:" '$1 == k { print $2; exit }' "$1" 2>/dev/null; }

snapshot() {
    echo "  pid      rss_kib  pss_kib  process"
    total_rss=0
    total_pss=0
    for pid in $(pgrep -u "$(id -u)" -f 'kompact-shell|WebKitWebProcess|WebKitNetworkProcess|xdg-dbus-proxy'); do
        [ -r /proc/$pid/status ] || continue
        comm=$(cat /proc/$pid/comm 2>/dev/null)
        rss=$(kib /proc/$pid/status VmRSS)
        pss=$(kib /proc/$pid/smaps_rollup Pss)
        printf "  %-8s %-8s %-8s %s\n" "$pid" "${rss:-?}" "${pss:-?}" "$comm"
        total_rss=$((total_rss + ${rss:-0}))
        total_pss=$((total_pss + ${pss:-0}))
    done
    echo "  total    $total_rss  $total_pss  (KiB)"
    kpid=$(pgrep -x kompact | head -n 1)
    [ -n "$kpid" ] && echo "  kompact  rss_kib=$(kib /proc/$kpid/status VmRSS)"
}

stop_shell() {
    pkill -x kompact-shell 2>/dev/null
    # Wait for the shell and its WebKit helpers to be gone (up to 5 s).
    i=0
    while pgrep -u "$(id -u)" -f 'kompact-shell|WebKitWebProcess|WebKitNetworkProcess' >/dev/null && [ $i -lt 50 ]; do
        sleep 0.1
        i=$((i + 1))
    done
}

# run NAME WALLPAPER: start the shell, wait for its pages to load, wait 5 s more, snapshot.
run() {
    name=$1
    slog=$OUT/shell-$name.log
    stop_shell
    echo "== $name (KOMPACT_WALLPAPER=$2) ==" >>"$LOG"
    KOMPACT_WALLPAPER=$2 "$SHELL_BIN" >"$slog" 2>&1 &
    # The panel, plus the wallpaper unless it's off; give up after 20 s.
    want=1
    [ "$2" = none ] || want=2
    i=0
    while [ "$(grep -c 'page loaded' "$slog")" -lt $want ] && [ $i -lt 200 ]; do
        sleep 0.1
        i=$((i + 1))
    done
    if [ $i -ge 200 ]; then
        echo "  pages not loaded after 20 s (see shell-$name.log)" >>"$LOG"
    else
        echo "  pages loaded after $((i / 10)).$((i % 10)) s" >>"$LOG"
    fi
    sleep 5
    snapshot >>"$LOG"
    grep -E 'ERROR|WARN|CONSOLE' "$slog" | sed 's/\x1b\[[0-9;]*m//g; s/^/  /' >>"$LOG"
    echo >>"$LOG"
    echo "$name: done"
}

{
    echo "kompact 6b wallpaper memory test, $(date '+%F %T')"
    echo "image: $IMAGE ($(file -b "$IMAGE" 2>/dev/null))"
    echo
} >"$LOG"

run none none
run gradient ""
run image "$IMAGE"

echo "Results in $LOG; the image run is still up, check it on screen."
