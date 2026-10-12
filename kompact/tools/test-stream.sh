#!/usr/bin/env bash
# 9e-a: a screencopy stream (copy_with_damage frame after frame) from inside kompact, the way
# screen sharing will use it. Run it in foot. Needs wf-recorder: `sudo apt install wf-recorder`.
# Records VGA-1 for 12 s into /tmp/kompact-9e only: 6 s with nothing moving (kompact should
# hold the copies), then 6 s while you move a window around.
set -u

if [ -z "${WAYLAND_DISPLAY:-}" ]; then
    echo "Run this in a terminal inside kompact (WAYLAND_DISPLAY is not set)."
    exit 1
fi
command -v wf-recorder >/dev/null || { echo "wf-recorder not found: sudo apt install wf-recorder"; exit 1; }

out=/tmp/kompact-9e
mkdir -p "$out"
rm -f "$out"/stream.mkv "$out"/wf-recorder.log
output=${1:-VGA-1}

echo "Recording $output for 12 s into $out/stream.mkv."
echo "  0-6 s:  hands off, don't move the mouse."
echo "  6-12 s: drag a window around $output."
wf-recorder -y -o "$output" -c libx264 -p preset=ultrafast -f "$out/stream.mkv" >"$out/wf-recorder.log" 2>&1 &
pid=$!
sleep 6
echo "  -> now move a window (6 s)"
sleep 6
kill -INT "$pid" 2>/dev/null
wait "$pid"
echo "wf-recorder finished with exit code $? (log: $out/wf-recorder.log)"
if [ -s "$out/stream.mkv" ]; then
    echo "  OK    $out/stream.mkv, $(du -h "$out/stream.mkv" | cut -f1)"
    command -v ffprobe >/dev/null && ffprobe -v error -count_frames -select_streams v:0 \
        -show_entries stream=width,height,nb_read_frames -of default=nw=1 "$out/stream.mkv" | sed 's/^/        /'
else
    echo "  FAIL  no video written; last lines of the log:"
    tail -5 "$out/wf-recorder.log" | sed 's/^/        /'
fi
echo "Wait 3 s more for kompact's 'stream ... ended' line, then quit kompact."
