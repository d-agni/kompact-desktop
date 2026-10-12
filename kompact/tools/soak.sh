#!/bin/sh
# kompact soak test (Phase 3b/3f).
#
# Run from a terminal *inside* kompact (e.g. the foot window it opens). For DURATION it:
#   - samples kompact's RSS, fd count and thread count every INTERVAL seconds into a CSV,
#   - every CHURN seconds opens a few short-lived foot windows (they close themselves), so
#     windows are created and destroyed continuously without you doing anything,
# then prints a verdict: PLATEAU (memory stable after warm-up) or GROWING.
#
# Meanwhile, do the manual parts by hand now and then: drag/snap/resize windows, switch
# workspaces, open menus, use Firefox.
#
# Usage:
#   tools/soak.sh [--minutes N] [--interval S] [--churn S] [--no-churn] [--pid PID] [--out FILE]
#   tools/soak.sh --summarize FILE.csv
#
# Defaults: 30 minutes, sample every 10 s, churn every 15 s (3 windows, 4 s each).

set -eu

MINUTES=30
INTERVAL=10
CHURN=15
CHURN_WINDOWS=3
PID=""
OUT=""
SUMMARIZE=""

while [ $# -gt 0 ]; do
    case "$1" in
        --minutes) MINUTES=$2; shift 2 ;;
        --interval) INTERVAL=$2; shift 2 ;;
        --churn) CHURN=$2; shift 2 ;;
        --no-churn) CHURN=0; shift ;;
        --pid) PID=$2; shift 2 ;;
        --out) OUT=$2; shift 2 ;;
        --summarize) SUMMARIZE=$2; shift 2 ;;
        -h|--help) sed -n '2,20p' "$0"; exit 0 ;;
        *) echo "unknown option: $1" >&2; exit 2 ;;
    esac
done

# Verdict from a CSV (elapsed_s,rss_kib,fds,threads). The first 2 minutes (or first fifth of a
# short run) are warm-up: caches, fonts, first textures.
#
# RSS moves in two ways: *steps* (a jump of >= 1 MiB between two samples) when load changes, e.g.
# you open another app or a workspace switch allocates something, and *creep*, the slow drift of
# all the small changes in between. A leak shows up as creep, and repeated identical steps;
# load changes show up as one-off steps. So the steps are listed with their times (match them
# against what you did), and the verdict only looks at creep: GROWING if it exceeds 64 KiB/min.
summarize() {
    awk -F, '
        NR == 1 { next }   # header
        { t[n] = $1; r[n] = $2; f[n] = $3; n++ }
        END {
            if (n < 5) { print "not enough samples (" n ")"; exit 1 }
            warm = 120
            if (t[n-1] < 600) warm = t[n-1] / 5
            s = 0; while (s < n && t[s] < warm) s++
            if (n - s < 5) s = 0
            step = 1024
            creep = 0; nsteps = 0; stepsum = 0
            for (i = s + 1; i < n; i++) {
                d = r[i] - r[i-1]
                if (d >= step || d <= -step) {
                    nsteps++; stepsum += d
                    steps = steps sprintf("  at %4.1f min: %+d KiB\n", t[i] / 60, d)
                } else creep += d
            }
            mins = (t[n-1] - t[s]) / 60; if (mins <= 0) mins = 1
            rate = creep / mins
            min = r[s]; max = r[s]
            for (i = s; i < n; i++) { if (r[i] < min) min = r[i]; if (r[i] > max) max = r[i] }
            printf "samples: %d over %.1f min (warm-up %.0f s excluded)\n", n, t[n-1] / 60, warm
            printf "rss_kib: %d after warm-up -> %d at end (min %d, max %d)\n", r[s], r[n-1], min, max
            printf "steps (>= 1 MiB between samples): %d, total %+d KiB\n", nsteps, stepsum
            printf "%s", steps
            printf "creep (everything between steps): %+d KiB = %+.1f KiB/min\n", creep, rate
            printf "fds: %d after warm-up -> %d at end (new apps add sockets; check stats: lines)\n", f[s], f[n-1]
            if (rate > 64) { print "verdict: GROWING (creep)"; exit 3 }
            print "verdict: PLATEAU (no creep; review the steps above against what you did)"
        }' "$1"
}

if [ -n "$SUMMARIZE" ]; then
    summarize "$SUMMARIZE"
    exit $?
fi

if [ -z "$PID" ]; then
    PID=$(pgrep -n -x kompact || true)
fi
if [ -z "$PID" ] || [ ! -d "/proc/$PID" ]; then
    echo "kompact is not running (or pass --pid)" >&2
    exit 1
fi
[ -n "$OUT" ] || OUT="$HOME/kompact-soak-$(date +%Y%m%d-%H%M%S).csv"

echo "elapsed_s,rss_kib,fds,threads" > "$OUT"
echo "soak: pid $PID, ${MINUTES} min, sample every ${INTERVAL}s, churn every ${CHURN}s -> $OUT"

CHILDREN=""
cleanup() {
    for c in $CHILDREN; do kill "$c" 2>/dev/null || true; done
}
trap 'cleanup; echo; echo "interrupted"; summarize "$OUT" || true; exit 130' INT TERM

start=$(date +%s)
end=$((start + MINUTES * 60))
next_churn=$((start + CHURN))
while :; do
    now=$(date +%s)
    [ "$now" -lt "$end" ] || break
    if [ ! -d "/proc/$PID" ]; then
        echo "kompact (pid $PID) exited during the soak" >&2
        break
    fi
    rss=$(awk '/^VmRSS:/ { print $2 }' "/proc/$PID/status")
    threads=$(awk '/^Threads:/ { print $2 }' "/proc/$PID/status")
    fds=$(ls "/proc/$PID/fd" 2>/dev/null | wc -l)
    echo "$((now - start)),$rss,$fds,$threads" >> "$OUT"

    if [ "$CHURN" -gt 0 ] && [ "$now" -ge "$next_churn" ] && command -v foot >/dev/null; then
        i=0
        while [ $i -lt $CHURN_WINDOWS ]; do
            foot sh -c 'sleep 4' >/dev/null 2>&1 &
            CHILDREN="$CHILDREN $!"
            i=$((i + 1))
        done
        next_churn=$((now + CHURN))
        # Forget children that already finished, so the list stays short.
        alive=""
        for c in $CHILDREN; do [ -d "/proc/$c" ] && alive="$alive $c"; done
        CHILDREN=$alive
    fi
    sleep "$INTERVAL"
done

cleanup
echo "done: $OUT"
summarize "$OUT"
