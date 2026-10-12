#!/bin/sh
# 6p-e: compare paced frame callbacks (new) with one every loop turn (old, KOMPACT_FRAME_PACING=off).
# Run from a text console (not inside kompact):
#   sh ~/Documents/kompact-de/kompact/tools/compare-pacing.sh
# It starts kompact twice, one after the other. In each run do the same for about 2 minutes:
# open Calculator, Text Editor and Nautilus, type a few lines in Text Editor, scroll a page in Firefox,
# show the dock a few times, open and close the overview twice and
# Launchpad twice, then leave it alone for 20 s; quit with Super+Escape.
# Logs: ktest-paced.log and ktest-unpaced.log, here in kompact/.
cd "$(dirname "$0")/.." || exit 1
esc=$(printf '\033')
plain() { sed "s/${esc}\[[0-9;]*m//g" "$1"; }

run() { # name, value of KOMPACT_FRAME_PACING
    printf '\nrun %s (frame callbacks %s): press Enter to start kompact, Super+Escape when done ' "$1" "$1"
    read -r _
    KOMPACT_FRAME_PACING=$2 ./ktest-desktop.sh
    cp ktest.log "ktest-$1.log"
}
run paced on
run unpaced off

ok=1
plain ktest-paced.log | grep -q "frames: callbacks paced" || { echo "ktest-paced.log: not paced"; ok=0; }
plain ktest-unpaced.log | grep -q "frames: old way" || { echo "ktest-unpaced.log: not the old way"; ok=0; }
[ "$ok" = 1 ] && echo "both paths ran as intended"

for f in ktest-paced.log ktest-unpaced.log; do
    echo
    echo "== $f (every stats interval with frames: windows, flips, missed, cpu_ms, frame_ms)"
    plain "$f" | grep "stats: " | grep -v "flips=0 " \
        | grep -o "windows=[0-9]*\|flips=[0-9]*\|missed=[0-9]*\|cpu_ms=[^ ]*\|frame_ms=[^ ]*" | paste - - - - -
    sh tools/perf-baseline.sh "$f" | grep "^| [0-9]"
done

for f in ktest-paced.log ktest-unpaced.log; do
    echo
    echo "== $f uploads: megapixels and commits per interval (the busiest three owners)"
    plain "$f" | grep -o "uploads: .*" | cut -d" " -f2,4-6
done
