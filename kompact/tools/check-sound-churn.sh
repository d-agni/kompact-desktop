#!/bin/sh
# 17a check: does anything keep starting PipeWire clients (the shell's old sound-watcher loop ran
# ~8 `wpctl`/`pw-dump` a second)? Watches PipeWire for 10 seconds and counts the short-lived
# clients by program, plus PipeWire's error counts (xruns) for what's playing. Run it inside
# kompact, ideally with a video playing.
#   kompact/tools/check-sound-churn.sh [seconds]
secs=${1:-10}
python3 -I - "$secs" <<'EOF'
import collections, json, signal, subprocess, sys, time
secs = int(sys.argv[1])
signal.signal(signal.SIGALRM, lambda *_: p.kill())  # a quiet PipeWire prints nothing: stop anyway
p = subprocess.Popen(["pw-dump", "--monitor", "-N"], stdout=subprocess.PIPE, text=True)
seen, buf, t0, start = collections.Counter(), [], time.time(), None
signal.alarm(secs + 2)
for line in p.stdout:
    buf.append(line)
    if line.rstrip("\n") != "]":
        continue
    update = json.loads("".join(buf)); buf = []
    if start is None:  # the first update is the whole graph
        start = time.time(); continue
    for o in update:
        if o.get("type") == "PipeWire:Interface:Client":
            props = (o.get("info") or {}).get("props") or {}
            seen[props.get("application.process.binary") or props.get("application.name") or "?"] += 1
    if time.time() - start >= secs:
        break
p.kill()
total = sum(seen.values())
print(f"new PipeWire clients in {secs} s: {total} ({total / secs:.1f}/s)" + ("" if total else ", none: fine"))
for name, n in seen.most_common():
    if name != "pw-dump" or n > 1:  # our own pw-dump shows once
        print(f"  {name}: {n}")
EOF
echo "running processes: wpctl $(pgrep -cx wpctl), pw-dump $(pgrep -cx pw-dump) (1 is the shell's watcher)"
echo "PipeWire (ERR = xruns since it started):"
timeout 3 pw-top -b -n 2 2>/dev/null | awk '/^S +ID/{n++} n==2 && $1=="R"'
