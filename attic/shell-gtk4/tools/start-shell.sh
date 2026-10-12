#!/bin/sh
# Restart kompact-shell in one of a few variants, to narrow down a display problem.
# Run it in foot, inside kompact:
#
#   sh ~/Documents/kompact-de/shell/tools/start-shell.sh [variant]
#
#   normal       everything (default)
#   nolauncher   panel + wallpaper, no launcher         (KOMPACT_SHELL_LAUNCHER=off)
#   nowallpaper  panel + launcher, no wallpaper         (KOMPACT_WALLPAPER=none)
#   panel        the panel only                         (both off)
#
# The shell's log goes to ~/Documents/kompact-de/kompact/shell-<variant>.log.

set -u
BASE=$HOME/Documents/kompact-de
BIN=$BASE/shell/target/release/kompact-shell
VARIANT=${1:-normal}
LOG=$BASE/kompact/shell-$VARIANT.log

[ -n "${WAYLAND_DISPLAY:-}" ] || { echo "run this in foot, inside kompact" >&2; exit 1; }

case $VARIANT in
    normal)      set -- ;;
    nolauncher)  set -- KOMPACT_SHELL_LAUNCHER=off ;;
    nowallpaper) set -- KOMPACT_WALLPAPER=none ;;
    panel)       set -- KOMPACT_SHELL_LAUNCHER=off KOMPACT_WALLPAPER=none ;;
    *) echo "unknown variant $VARIANT (normal, nolauncher, nowallpaper, panel)" >&2; exit 1 ;;
esac

pkill -x kompact-shell 2>/dev/null
# Wait for the old shell (and its lock) to go, up to 5 s.
i=0
while pgrep -x kompact-shell >/dev/null && [ $i -lt 50 ]; do sleep 0.1; i=$((i + 1)); done

env "$@" "$BIN" >"$LOG" 2>&1 &
echo "started kompact-shell ($VARIANT${1:+: $*}), log $LOG"
echo "to stop it: pkill kompact-shell"
