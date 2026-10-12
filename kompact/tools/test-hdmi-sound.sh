#!/bin/sh
# HDMI sound check: plays a 2-second 440 Hz beep to the HDMI output and shows whether the sound
# hardware played it, and whether the monitor reports it can take sound. Run it once on the text
# console (no kompact) and once inside kompact, and note where you hear the beep.
#   kompact/tools/test-hdmi-sound.sh
sink=$(wpctl status 2>/dev/null | sed -n 's/.*[* ]\s*\([0-9]\+\)\. .*(HDMI).*/\1/p' | head -1)
if [ -z "$sink" ]; then
    echo "no HDMI output in PipeWire (wpctl status shows none)"; exit 1
fi
name=$(wpctl inspect "$sink" | sed -n 's/.*node.name = "\(.*\)"/\1/p')
echo "HDMI output: $name, volume: $(wpctl get-volume "$sink")"
for f in /proc/asound/card*/eld*; do
    grep -q "monitor_present.*1" "$f" && echo "monitor on $f: $(sed -n 's/^monitor_name\s*//p' "$f"), sound formats: $(grep -c '^sad[0-9]*_coding_type' "$f")"
done
tone=${XDG_RUNTIME_DIR:-/tmp}/kompact-hdmi-tone.wav
python3 -I -c "
import wave,struct,math,sys
w=wave.open(sys.argv[1],'w');w.setnchannels(2);w.setsampwidth(2);w.setframerate(48000)
w.writeframes(b''.join(struct.pack('<hh',v,v) for v in (int(8000*math.sin(2*math.pi*440*i/48000)) for i in range(96000))))" "$tone"
echo "playing a 2-second beep now (session: ${XDG_CURRENT_DESKTOP:-text console})..."
pw-play --target "$name" "$tone" &
sleep 1
grep -l RUNNING /proc/asound/card*/pcm*p/sub0/status 2>/dev/null | sed 's|/sub0/status||; s|^|playing on: |'
wait
rm -f "$tone"
echo "heard it? If not: check the monitor's own volume/mute and its audio-input setting (HDMI)."
