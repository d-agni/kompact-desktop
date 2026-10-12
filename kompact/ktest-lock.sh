#!/bin/sh
# 14a: the desktop (ktest-desktop.sh) with kompact's own lock page, kompact-lock, in place of
# swaylock: Super+L, the idle lock and the lock before sleep all use it. Since 14b kompact finds
# kompact-lock by itself; this script still checks it's built and its library is installed.
#   ./ktest-lock.sh [wallpaper image]
# Logs as ktest-desktop.sh: ktest.log (kompact and kompact-lock's lines) and shell.log.
# Stuck locked: Ctrl+Alt+F3, log in, `pkill kompact-lock`, then take over the lock with swaylock:
#   XDG_RUNTIME_DIR=/run/user/$(id -u) WAYLAND_DISPLAY=wayland-1 swaylock -f
# and go back to kompact's VT to unlock with it (KOMPACT_LOCK_PAGE=off next time).
cd "$(dirname "$0")"
lock="$(cd .. && pwd)/shell/src-tauri/target/release/kompact-lock"
if [ ! -x "$lock" ]; then
    echo "no $lock; build it: (cd ../shell/src-tauri && cargo build --release --bin kompact-lock)" >&2
    exit 1
fi
case "$lock" in
    *" "*) echo "kompact runs the lock command split at spaces; $lock has one" >&2; exit 1 ;;
esac
if ! ldconfig -p | grep -q 'libgtk-session-lock\.so\.0'; then
    echo "libgtk-session-lock isn't installed: sudo apt install libgtk-session-lock0" >&2
    echo "(without it kompact-lock falls back to swaylock -f)" >&2
    exit 1
fi
echo "kompact: lock page $lock"
KOMPACT_LOCK_CMD="$lock" exec ./ktest-desktop.sh "$@"
