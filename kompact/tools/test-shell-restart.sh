#!/bin/sh
# 6d: check that kompact restarts the shell after a crash, backs off, and gives up.
# Run from a foot window inside kompact started with ./ktest-desktop.sh (KOMPACT_SHELL=on).
#   sh ~/Documents/kompact-de/kompact/tools/test-shell-restart.sh         crashes (ends with the shell stopped)
#   sh ~/Documents/kompact-de/kompact/tools/test-shell-restart.sh stop    a plain pkill must not restart it
# Restart kompact afterwards to get the shell back. Restart Shell (logo menu) is tested by hand.
# Crashes are simulated with SIGKILL (kompact counts it as a crash, as from the OOM killer):
# SIGSEGV would make the shell write a core dump first, which takes seconds.
log="$(cd "$(dirname "$0")/.." && pwd)/ktest.log"
pass=0
fail=0

pid() { pgrep -x kompact-shell | head -1; }
# A running shell other than pid $1 (a dying one can still be listed for a moment).
other() { pgrep -x kompact-shell | grep -vx "$1" | head -1; }
ok() { echo "  PASS $1"; pass=$((pass + 1)); }
bad() { echo "  FAIL $1"; fail=$((fail + 1)); }

# Wait up to $2 s for a shell other than $1 to be running; prints its pid.
new_pid() {
    i=0
    while [ "$i" -lt $(($2 * 10)) ]; do
        p=$(other "$1")
        if [ -n "$p" ]; then echo "$p"; return 0; fi
        sleep 0.1
        i=$((i + 1))
    done
    return 1
}

# Still no shell after $1 s?
stays_down() {
    sleep "$1"
    [ -z "$(pid)" ]
}

first=$(pid)
if [ -z "$first" ]; then
    echo "no kompact-shell running; start kompact with ./ktest-desktop.sh first"
    exit 1
fi
if ! grep -q "shell: supervised" "$log" 2>/dev/null; then
    echo "$log doesn't say the shell is supervised; was kompact started with KOMPACT_SHELL=on?"
    exit 1
fi
echo "shell running as pid $first"

if [ "$1" = "stop" ]; then
    echo "pkill (SIGTERM): kompact should leave it stopped"
    pkill -TERM -x kompact-shell
    if stays_down 4; then ok "still stopped after 4 s"; else bad "it was started again"; fi
else
    # The 1st, 2nd and 3rd crash in a minute: back after about 1, 2 and 4 s.
    cur=$first
    n=1
    for wait in 1 2 4; do
        echo "crash $n: expect it back in about $wait s"
        start=$(date +%s)
        kill -KILL "$cur"
        if next=$(new_pid "$cur" $((wait + 5))); then
            ok "back as pid $next after $(($(date +%s) - start)) s"
            cur=$next
        else
            bad "not back within $((wait + 5)) s"
            exit 1
        fi
        # Let it get going (and connect) before crashing it again.
        sleep 3
        n=$((n + 1))
    done
    echo "crash $n: the 4th within a minute, kompact should give up"
    kill -KILL "$cur"
    if stays_down 8; then ok "still stopped after 8 s"; else bad "it was started again"; fi
fi

echo
echo "ktest.log:"
esc=$(printf '\033')
sed "s/${esc}\[[0-9;]*m//g" "$log" | grep -E "shell: (started|crashed|exited|asked|another)" | tail -8 | sed -E 's/^[^ ]+ +[A-Z]+ +//'
echo
echo "$pass passed, $fail failed"
[ "$fail" -eq 0 ]
