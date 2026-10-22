#!/bin/sh
# Builds a release, stops the running shell, installs the release, starts the new shell (17a).
# Run it from inside kompact, as yourself (it asks sudo for the installer only):
#   ./update-desktop.sh                  # build + install, shell restarted
#   ./update-desktop.sh --no-build       # install the release built last
#   ./update-desktop.sh --login-screen   # passed on to install.sh
# The new kompact itself takes over at the next login (the running one can't be replaced without
# ending the session); the new shell runs now. Before installing, the installed kompact and
# kompact-shell are kept in kompact/rollback/*-before-<date>.
#
# The shell started here isn't supervised (kompact leaves a shell stopped with SIGTERM alone, 6d):
# if it crashes it stays gone until the next login, or start it again with this script.
set -eu
root=$(cd "$(dirname "$0")" && pwd)
build=1
args=
for a in "$@"; do
    case "$a" in
        --no-build) build= ;;
        -h | --help) sed -n '2,12p' "$0"; exit 0 ;;
        *) args="$args $a" ;;
    esac
done
say() { printf '\n==> %s\n' "$*"; }
fail() { printf 'update-desktop: %s\n' "$*" >&2; exit 1; }

[ "$(id -u)" != 0 ] || fail "run it as yourself, not with sudo (it asks for sudo itself)"
in_kompact=
[ "${XDG_CURRENT_DESKTOP:-}" = kompact ] && [ -n "${WAYLAND_DISPLAY:-}" ] && in_kompact=1

if [ -n "$build" ]; then
    say "building the release (tests first)"
    "$root/make-release.sh"
fi
version=$(sed -n 's/^version = "\(.*\)"/\1/p' "$root/kompact/Cargo.toml" | head -1)
stage=$root/dist/kompact-desktop-$version-x86_64
[ -x "$stage/install.sh" ] || fail "no release in $stage (run without --no-build)"

say "keeping the installed programs as a rollback"
stamp=$(date +%Y%m%d-%H%M%S)
for p in kompact kompact-shell; do
    if [ -f "/usr/local/bin/$p" ]; then
        cp -p "/usr/local/bin/$p" "$root/kompact/rollback/$p-before-$stamp"
        echo "  kompact/rollback/$p-before-$stamp"
    fi
done

if [ -n "$in_kompact" ] && pgrep -x kompact-shell >/dev/null; then
    say "stopping the shell"
    pkill -TERM -x kompact-shell
    i=0
    while pgrep -x kompact-shell >/dev/null; do
        i=$((i + 1))
        [ $i -le 50 ] || { echo "  it didn't stop in 5 s, killing it"; pkill -KILL -x kompact-shell; sleep 0.5; break; }
        sleep 0.1
    done
fi

say "installing kompact desktop $version (sudo)"
# shellcheck disable=SC2086 # the options, split on purpose
if ! sudo "$stage/install.sh" $args; then
    echo "update-desktop: the installer failed; starting the shell that is installed now" >&2
    failed=1
fi

if [ -n "$in_kompact" ]; then
    say "starting the shell"
    log=${KOMPACT_SHELL_LOG:-${XDG_STATE_HOME:-$HOME/.local/state}/kompact/shell.log}
    mkdir -p "$(dirname "$log")"
    printf '\n--- update-desktop.sh: shell started %s ---\n' "$(date)" >>"$log"
    setsid -f /usr/local/bin/kompact-shell >>"$log" 2>&1 </dev/null
    sleep 2
    if pgrep -x kompact-shell >/dev/null; then
        echo "  running (log: $log)"
    else
        fail "the shell didn't start; see $log"
    fi
else
    echo
    echo "Not inside kompact: nothing to restart; log in to kompact to use it."
fi

[ -z "${failed:-}" ] || exit 1
say "done: the new shell runs now; log out and back in for the new kompact"
