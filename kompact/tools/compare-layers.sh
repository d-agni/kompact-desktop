#!/bin/sh
# 6m-e2a: compare drawing each layer surface once (new) with twice (old, KOMPACT_LAYERS_ONCE=off).
# Run from a text console (not inside kompact):
#   sh ~/Documents/kompact-de/kompact/tools/compare-layers.sh
# It starts kompact twice, one after the other. In each run do the same for about 2 minutes:
# open foot and Nautilus, show the dock a few times, open and close the overview twice and
# Launchpad twice, then leave it alone for 20 s; quit with Super+Escape.
# Logs: ktest-once.log and ktest-twice.log, here in kompact/.
cd "$(dirname "$0")/.." || exit 1
esc=$(printf '\033')
plain() { sed "s/${esc}\[[0-9;]*m//g" "$1"; }

run() { # name, value of KOMPACT_LAYERS_ONCE
    printf '\nrun %s (layers drawn %s): press Enter to start kompact, Super+Escape when done ' "$1" "$1"
    read -r _
    KOMPACT_LAYERS_ONCE=$2 ./ktest-desktop.sh
    cp ktest.log "ktest-$1.log"
}
run once on
run twice off

ok=1
plain ktest-once.log | grep -q "render: each layer surface drawn once" || { echo "ktest-once.log: not the new path"; ok=0; }
plain ktest-twice.log | grep -q "render: old path" || { echo "ktest-twice.log: not the old path"; ok=0; }
[ "$ok" = 1 ] && echo "both paths ran as intended"

for f in ktest-once.log ktest-twice.log; do
    echo
    echo "== $f (every stats interval with frames: windows, flips, missed, cpu_ms, frame_ms)"
    plain "$f" | grep "stats: " | grep -v "flips=0 " \
        | grep -o "windows=[0-9]*\|flips=[0-9]*\|missed=[0-9]*\|cpu_ms=[^ ]*\|frame_ms=[^ ]*" | paste - - - - -
    sh tools/perf-baseline.sh "$f" | grep "^| [0-9]"
done
