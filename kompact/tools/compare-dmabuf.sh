#!/bin/sh
# 6p-f: compare apps with linux-dmabuf (default) and without (KOMPACT_DMABUF=off, shared memory).
# Run from a text console (not inside kompact):
#   sh ~/Documents/kompact-de/kompact/tools/compare-dmabuf.sh
# It starts kompact twice, one after the other. In each run do the same for about 2 minutes:
# open Calculator, Text Editor and Nautilus, type a few lines in Text Editor, scroll a page in Firefox,
# show the dock a few times, open and close the overview twice and
# Launchpad twice, then leave it alone for 20 s; quit with Super+Escape.
# Logs: ktest-dmabuf.log and ktest-shm.log, here in kompact/.
cd "$(dirname "$0")/.." || exit 1
esc=$(printf '\033')
plain() { sed "s/${esc}\[[0-9;]*m//g" "$1"; }

run() { # name, value of KOMPACT_DMABUF
    printf '\nrun %s (buffers: %s): press Enter to start kompact, Super+Escape when done ' "$1" "$1"
    read -r _
    KOMPACT_DMABUF=$2 ./ktest-desktop.sh
    cp ktest.log "ktest-$1.log"
}
run dmabuf on
run shm off

ok=1
plain ktest-dmabuf.log | grep -q "dmabuf: linux-dmabuf v4 offered" || { echo "ktest-dmabuf.log: dmabuf not offered"; ok=0; }
plain ktest-shm.log | grep -q "dmabuf: not offered" || { echo "ktest-shm.log: dmabuf was offered"; ok=0; }
[ "$ok" = 1 ] && echo "both paths ran as intended"

for f in ktest-dmabuf.log ktest-shm.log; do
    echo
    echo "== $f (every stats interval with frames: windows, flips, missed, cpu_ms, frame_ms)"
    plain "$f" | grep "stats: " | grep -v "flips=0 " \
        | grep -o "windows=[0-9]*\|flips=[0-9]*\|missed=[0-9]*\|cpu_ms=[^ ]*\|frame_ms=[^ ]*" | paste - - - - -
    sh tools/perf-baseline.sh "$f" | grep "^| [0-9]"
done

for f in ktest-dmabuf.log ktest-shm.log; do
    echo
    echo "== $f uploads: megapixels and commits per interval (shm share, then the busiest three owners)"
    plain "$f" | grep -o "uploads: .*" | cut -d" " -f2-6
done

echo
echo "== ktest-dmabuf.log: refused buffers and warnings"
plain ktest-dmabuf.log | grep -E "dmabuf: refused|WARN|ERROR" | grep -v "drm master\|Failed to reset crtc" | cut -c1-200 | head -20
