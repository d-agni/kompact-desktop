#!/bin/sh
# 6d: kompact with the shell started (and restarted after a crash) by kompact itself.
# Window thumbnails (dock previews, the workspace overview) and the shell's pages in one WebKit
# process with a small cache are the defaults since 6p-c; for the old way:
#   KOMPACT_THUMBNAILS=off KOMPACT_SHELL_WEB_PROCESS=separate KOMPACT_SHELL_CACHE=browser ./ktest-desktop.sh
# Logs: ktest.log (kompact) and shell.log (the shell), here in kompact/.
#   ./ktest-desktop.sh [wallpaper image]
# 10d-2: the wallpaper is chosen in System Settings → Wallpaper; an image named here holds it
# for this run (Settings can't change it), and the Raccoon one is only the default until a
# choice is saved.
cd "$(dirname "$0")"
default_image=/usr/share/backgrounds/cnusr25-Simple_Raccoon_Dark.png
echo "kompact: outputs ${KOMPACT_OUTPUTS:-all (default; KOMPACT_OUTPUTS=first for one screen)}, log ktest.log"
# First log line: how this run was started (Phase 5 runs kept starting in `first`).
echo "$(date -u +%Y-%m-%dT%H:%M:%S) ktest: started as '$0 $*' from '$(ps -o args= -p $PPID | cut -c1-80)', KOMPACT_OUTPUTS=${KOMPACT_OUTPUTS-<unset>}" > ktest.log
if [ -n "$1" ]; then export KOMPACT_WALLPAPER="$1"; fi
# 14d: this checkout's builds, not copies an install put in /usr/local/bin (found first on $PATH).
dev_shell="$(cd .. && pwd)/shell/src-tauri/target/release/kompact-shell"
dev_lock="$(cd .. && pwd)/shell/src-tauri/target/release/kompact-lock"
[ -x "$dev_lock" ] && [ -z "${KOMPACT_LOCK_CMD:-}" ] && export KOMPACT_LOCK_CMD="$dev_lock"
KOMPACT_SHELL="$( [ -x "$dev_shell" ] && echo "$dev_shell" || echo on )" KOMPACT_SHELL_LOG="$PWD/shell.log" KOMPACT_WALLPAPER_DEFAULT="$default_image" \
    KOMPACT_STATS_SECS=10 RUST_LOG=${RUST_LOG:-info} ./target/release/kompact --backend drm >> ktest.log 2>&1
# 10a: keep a copy of every run (ktest.log is overwritten by the next start), named after the
# keys preset, and print which shortcuts ran.
mkdir -p screenlog
run="screenlog/run-$(date +%H%M%S)-keys-${KOMPACT_KEYS:-default}.log"
cp ktest.log "$run"
echo "kompact: log kept as $run"
[ -x tools/check-keys.sh ] || [ -f tools/check-keys.sh ] && sh tools/check-keys.sh "${KOMPACT_KEYS:-default}" >/dev/null 2>&1 && echo "kompact: shortcut report in screenlog/keys-${KOMPACT_KEYS:-default}.log"
