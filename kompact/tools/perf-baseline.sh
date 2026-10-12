#!/bin/sh
# kompact performance baseline (Phase 4d).
#
# Reads a kompact log (RUST_LOG=info, DRM backend) and summarises the frame timing that perf.rs
# appends to every `stats:` line, grouped by workspace, plus every `popup shown:` line. Run one
# test scene per workspace (Super+1..9), so the log needs no hand-written markers:
#
#   workspace 1  idle            2-3 foot windows, hands off
#   workspace 2  typing          type steadily in foot, mouse still
#   workspace 3  pointer + drag  move the mouse, then Super+drag a window around
#   workspace 4  heavy           Firefox, keep scrolling a long page
#   (popups anywhere: right-click in gedit / Firefox a few times)
#
# The first stats interval after arriving on a workspace is skipped (it mixes two scenes).
# Intervals with no page flips (idle) count towards flips/s but have no timing to add.
#
# Usage: tools/perf-baseline.sh [ktest.log]
#
# Output: one row per workspace with the median of the intervals' p50s and p95s and the largest
# max (ms), flips per second, and missed vblanks / no_redraw per minute; then popup timings.

set -eu
LOG=${1:-ktest.log}
[ -r "$LOG" ] || { echo "can't read $LOG" >&2; exit 1; }

sed 's/\x1b\[[0-9;]*m//g' "$LOG" | awk '
function secs(ts,   h, m, s) {   # 2026-10-07T17:10:35.123Z -> seconds of the day
    h = substr(ts, 12, 2); m = substr(ts, 15, 2); s = substr(ts, 18, 9)
    return h * 3600 + m * 60 + s
}
function val(key,   i, kv) {
    for (i = 1; i <= NF; i++) { split($i, kv, "="); if (kv[1] == key) return kv[2] }
    return ""
}
# Append one value to list name[ws] (space separated).
function add(name, ws, v) { L[name, ws] = L[name, ws] " " v }
function median(list,   n, a, i, j, t) {
    n = split(list, a, " "); if (n == 0) return "-"
    for (i = 2; i <= n; i++) { t = a[i] + 0; for (j = i - 1; j >= 1 && a[j] + 0 > t; j--) a[j + 1] = a[j]; a[j + 1] = t }
    return a[int((n + 1) / 2)]
}
function maximum(list,   n, a, i, m) {
    n = split(list, a, " "); if (n == 0) return "-"
    m = a[1] + 0; for (i = 2; i <= n; i++) if (a[i] + 0 > m) m = a[i] + 0
    return m
}
# "p50/p95/max" field into three lists.
function triple(key, ws,   v, p) {
    v = val(key); if (v == "" || v == "-") return
    split(v, p, "/"); add(key "50", ws, p[1]); add(key "95", ws, p[2]); add(key "max", ws, p[3])
}
function cell(key, ws) {
    if (L[key "50", ws] == "") return "-"
    return sprintf("%.1f / %.1f / %.1f", median(L[key "50", ws]), median(L[key "95", ws]), maximum(L[key "max", ws]))
}
/ stats: / && / renders=/ {
    ws = val("workspace"); t = secs($1)
    if (ws != last_ws) { last_ws = ws; last_t = t; next }   # first interval on this workspace
    dt = t - last_t; last_t = t
    if (dt <= 0) next
    seen[ws] = 1; n[ws]++; secs_[ws] += dt
    flips[ws] += val("flips"); missed[ws] += val("missed"); noredraw[ws] += val("no_redraw")
    triple("cpu_ms", ws); triple("frame_ms", ws); triple("wait_ms", ws)
    triple("key_ms", ws); triple("ptr_ms", ws)
}
/ popup shown: / {
    pops++
    c = val("from_click_ms"); if (c != "-") add("click", 0, c)
    add("client", 0, val("client_ms")); add("comp", 0, val("compositor_ms"))
}
END {
    print "Frame timing per workspace (ms, p50 / p95 / max; p50 and p95 are medians over the intervals):"
    print ""
    print "| ws | intervals | flips/s | missed/min | cpu_ms | frame_ms | wait_ms | key_ms | ptr_ms | no_redraw/min |"
    print "|---|---|---|---|---|---|---|---|---|---|"
    for (ws = 1; ws <= 9; ws++) {
        if (!seen[ws]) continue
        m = secs_[ws] / 60
        printf "| %d | %d | %.0f | %.1f | %s | %s | %s | %s | %s | %.1f |\n", ws, n[ws], flips[ws] / secs_[ws], \
            missed[ws] / m, cell("cpu_ms", ws), cell("frame_ms", ws), cell("wait_ms", ws), \
            cell("key_ms", ws), cell("ptr_ms", ws), noredraw[ws] / m
    }
    print ""
    if (pops == 0) { print "Popups: none shown"; exit }
    printf "Popups: %d shown. from_click_ms median %.1f max %.1f; client_ms median %.1f max %.1f; compositor_ms median %.1f max %.1f\n", \
        pops, median(L["click", 0]), maximum(L["click", 0]), median(L["client", 0]), maximum(L["client", 0]), \
        median(L["comp", 0]), maximum(L["comp", 0])
}'
