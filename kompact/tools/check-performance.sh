#!/bin/sh
# 17b: one report on how the desktop is doing, from inside kompact. Read only: it starts nothing
# but `top`, `pw-top` and `pw-dump` for a few seconds. Best run while the thing that felt slow
# happens (a video playing, windows being moved), then send the output.
#   kompact/tools/check-performance.sh [seconds]     (default 10)
secs=${1:-10}
log=${XDG_STATE_HOME:-$HOME/.local/state}/kompact/kompact.log
here=$(cd "$(dirname "$0")" && pwd)
echo "== kompact performance check, $(date '+%F %T'), ${secs} s =="

echo
echo "-- compositor (last 3 stats: lines; Phase 4 budget: missed under ~60/min, cpu_ms p50 under 5) --"
if [ -r "$log" ]; then
    grep 'stats:' "$log" | tail -3 | sed 's/\x1b\[[0-9;]*m//g' |
        grep -oE '^[0-9T:.-]+|windows=[0-9]+|missed=[0-9]+|cpu_ms=[^ ]+|frame_ms=[^ ]+|rss_kib=[0-9]+' | paste -sd' ' |
        sed 's/ \([0-9]\{4\}-\)/\n\1/g'
else
    echo "no $log (is kompact running from the session script?)"
fi

echo
echo "-- processes started (all users): kompact itself starts none on its own --"
before=$(awk '/^processes/{print $2}' /proc/stat)
# top over the same time: what used the CPU.
top -b -d "$secs" -n 2 -o %CPU -w 200 | awk '/^top/{n++} n==2' >"${XDG_RUNTIME_DIR:-/tmp}/kompact-perf-top.$$"
after=$(awk '/^processes/{print $2}' /proc/stat)
started=$((after - before - 1))
echo "$started in ${secs} s ($(awk -v n="$started" -v s="$secs" 'BEGIN{printf "%.1f", n/s}')/s; a quiet desktop: under 1/s)"

echo
echo "-- busiest over those ${secs} s (%CPU of one core) --"
sed -n '3p;7,17p' "${XDG_RUNTIME_DIR:-/tmp}/kompact-perf-top.$$" | cut -c1-110
rm -f "${XDG_RUNTIME_DIR:-/tmp}/kompact-perf-top.$$"

echo
echo "-- kompact and the shell --"
for p in kompact kompact-shell WebKitWebProces; do  # names are cut at 15 characters
    for pid in $(pgrep -x "$p" | head -3); do
        ps -o pid=,pcpu=,rss=,etime=,comm= -p "$pid" | awk '{printf "%-18s pid %-7s cpu(avg) %5s%%  rss %6d MB  up %s\n", $5, $1, $2, $3/1024, $4}'
    done
done

echo
echo "-- memory --"
free -m | awk 'NR==1{print "            " $0} NR>1'

echo
echo "-- sound (17a) --"
if command -v pw-dump >/dev/null; then
    "$here/check-sound-churn.sh" 3
else
    echo "no PipeWire tools"
fi
