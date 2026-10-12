#!/bin/sh
# 14c-1: try the login screen (kompact-greeter) inside your running kompact session, against a
# stand-in for greetd (tools/fake-greetd.py). Nothing is installed and your real password isn't
# used: the test password is "kompact" (FAKE_GREETD_PASSWORD=... to change it).
#
# Run it from a terminal (foot) in kompact:
#   ~/Documents/kompact-de/kompact/tools/try-greeter.sh
# The login screen covers both monitors. Ctrl+Q quits it; logging in with the test password
# ends it too (the stand-in prints what greetd would start). Log: kompact/greeter-test.log.
# If it ever hangs: Ctrl+Alt+F3, log in, `pkill kompact-greeter`, back to kompact's VT.
set -eu
here=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$here/../.." && pwd)
greeter="$root/shell/src-tauri/target/release/kompact-greeter"
if [ -z "${WAYLAND_DISPLAY:-}" ]; then
    echo "run this from a terminal inside kompact (no WAYLAND_DISPLAY here)" >&2
    exit 1
fi
if [ ! -x "$greeter" ]; then
    echo "no $greeter; build it: (cd $root/shell/src-tauri && cargo build --release --bin kompact-greeter)" >&2
    exit 1
fi
run=${XDG_RUNTIME_DIR:-/tmp}/kompact-greeter-test
mkdir -p "$run"
sock="$run/greetd.sock"
log="$root/kompact/greeter-test.log"
: >"$log"
python3 "$here/fake-greetd.py" "$sock" 2>>"$log" &
fake=$!
trap 'kill $fake 2>/dev/null || true' EXIT
for _ in 1 2 3 4 5 6 7 8 9 10; do
    [ -S "$sock" ] && break
    sleep 0.2
done
echo "login screen test: the password is ${FAKE_GREETD_PASSWORD:-kompact}; Ctrl+Q quits; log $log"
# Its memory (last user, sessions) in the test folder, not where the real one keeps it.
GREETD_SOCK="$sock" KOMPACT_GREETER_STATE="$run/state" "$greeter" --test 2>>"$log" || true
echo "done; the stand-in said:"
grep fake-greetd "$log" | tail -4
