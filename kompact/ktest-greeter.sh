#!/bin/sh
# 14c-2: kompact in greeter mode on this VT, showing only the login screen (kompact-greeter),
# against the greetd stand-in (tools/fake-greetd.py). Nothing is installed; your real password
# isn't used: the test password is "kompact" (FAKE_GREETD_PASSWORD=... to change it).
# Run it from a text console (Ctrl+Alt+F2, log in), like ktest-desktop.sh:
#   ./ktest-greeter.sh
# Logging in with the test password, or Ctrl+Q, ends the login screen and kompact quits back
# to this console; the stand-in prints what greetd would have started. Ctrl+Alt+F1–F12 still
# switch VTs; no other shortcut works on the login screen.
# Logs: greeter-vt.log (kompact and the login screen; ktest.log is left alone).
cd "$(dirname "$0")"
root=$(cd .. && pwd)
greeter="$root/shell/src-tauri/target/release/kompact-greeter"
if [ -n "${WAYLAND_DISPLAY:-}" ]; then
    echo "run this from a text console, not inside kompact (for that: tools/try-greeter.sh)" >&2
    exit 1
fi
for f in ./target/release/kompact "$greeter"; do
    if [ ! -x "$f" ]; then
        echo "no $f; build kompact and the shell (cargo build --release) first" >&2
        exit 1
    fi
done
case "$greeter" in
    *" "*) echo "kompact splits the greeter command at spaces; $greeter has one" >&2; exit 1 ;;
esac
run=${XDG_RUNTIME_DIR:-/tmp}/kompact-greeter-test
mkdir -p "$run"
sock="$run/greetd.sock"
log="$PWD/greeter-vt.log"
echo "$(date -u +%Y-%m-%dT%H:%M:%S) ktest-greeter: started from '$(ps -o args= -p $PPID | cut -c1-80)'" >"$log"
python3 tools/fake-greetd.py "$sock" 2>>"$log" &
fake=$!
trap 'kill $fake 2>/dev/null || true' EXIT
for _ in 1 2 3 4 5 6 7 8 9 10; do
    [ -S "$sock" ] && break
    sleep 0.2
done
echo "kompact greeter mode: the test password is ${FAKE_GREETD_PASSWORD:-kompact}; Ctrl+Q quits; log $log"
GREETD_SOCK="$sock" KOMPACT_GREETER_STATE="$run/state" RUST_LOG=${RUST_LOG:-info} \
    ./target/release/kompact --backend drm --greeter "$greeter --test" >>"$log" 2>&1
echo "kompact exited; the stand-in said:"
grep fake-greetd "$log" | tail -3
