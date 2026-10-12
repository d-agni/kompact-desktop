#!/bin/sh
# 9e-b: share a monitor through the desktop portal, as a video call app does, and count the
# frames that arrive over PipeWire. Run it in foot inside kompact, after tools/install-portals.sh.
#   sh ~/Documents/kompact-de/kompact/tools/test-screencast.sh          (max_fps as configured)
#   sh ~/Documents/kompact-de/kompact/tools/test-screencast.sh 20       (set max_fps=20 first)
# When the crosshair appears, click the monitor to share (slurp). Then 15 s: 5 s hands off,
# 10 s moving a window. A shared frame a second is saved to /tmp/kompact-9e/screencast-NN.png.
if [ -z "${WAYLAND_DISPLAY:-}" ]; then
    echo "Run this in a terminal inside kompact (WAYLAND_DISPLAY is not set)."
    exit 1
fi
here=$(dirname "$0")
if [ -n "${1:-}" ]; then
    sh "$here/install-portals.sh" --fps "$1" >/dev/null || exit 1
    # The wlr portal reads its config when it starts; it's started again on the next request.
    systemctl --user stop xdg-desktop-portal-wlr.service 2>/dev/null
    echo "max_fps=$1 (the wlr portal starts again with it)"
fi
wlr_conf=${XDG_CONFIG_HOME:-$HOME/.config}/xdg-desktop-portal-wlr/kompact
grep -h max_fps "$wlr_conf" 2>/dev/null | sed 's/^/config: /'
mkdir -p /tmp/kompact-9e
rm -f /tmp/kompact-9e/screencast*.png /tmp/kompact-9e/xdpw.log
# For this test the wlr portal runs here, with its debug log in /tmp/kompact-9e/xdpw.log
# (--replace takes over from the service; it's started as a service again on the next request).
systemctl --user stop xdg-desktop-portal-wlr.service 2>/dev/null
/usr/libexec/xdg-desktop-portal-wlr --replace --loglevel=TRACE --config="$wlr_conf" >/tmp/kompact-9e/xdpw.log 2>&1 &
xdpw=$!
sleep 1
python3 -I - <<'PY'
import os, sys, time
import gi
gi.require_version("Gio", "2.0")
gi.require_version("Gst", "1.0")
from gi.repository import Gio, GLib, Gst

PORTAL, PATH = "org.freedesktop.portal.Desktop", "/org/freedesktop/portal/desktop"
CAST = "org.freedesktop.portal.ScreenCast"
bus = Gio.bus_get_sync(Gio.BusType.SESSION, None)
sender = bus.get_unique_name()[1:].replace(".", "_")
loop = GLib.MainLoop()
n = 0

def prop(name):
    v = bus.call_sync(PORTAL, PATH, "org.freedesktop.DBus.Properties", "Get",
                      GLib.Variant("(ss)", (CAST, name)), GLib.VariantType("(v)"), 0, 10000, None)
    return v.unpack()[0]

def request(method, args, timeout=90):
    """Call a portal method that answers with a Request's Response signal."""
    global n
    n += 1
    token = f"kompact9e{os.getpid()}_{n}"
    handle = f"{PATH}/request/{sender}/{token}"
    answer = {}
    def on_response(_c, _s, _p, _i, _sig, params):
        answer["code"], answer["results"] = params.unpack()
        loop.quit()
    sub = bus.signal_subscribe(PORTAL, "org.freedesktop.portal.Request", "Response", handle,
                               None, Gio.DBusSignalFlags.NO_MATCH_RULE, on_response)
    *rest, options = args
    options["handle_token"] = GLib.Variant("s", token)
    sig = {"CreateSession": "(a{sv})", "SelectSources": "(oa{sv})", "Start": "(osa{sv})"}[method]
    bus.call_sync(PORTAL, PATH, CAST, method, GLib.Variant(sig, (*rest, options)), None, 0, 10000, None)
    timer = GLib.timeout_add_seconds(timeout, loop.quit)
    loop.run()
    GLib.source_remove(timer) if answer else None
    bus.signal_unsubscribe(sub)
    if not answer:
        print(f"  FAIL  {method}: no answer in {timeout} s")
        sys.exit(1)
    if answer["code"] != 0:
        print(f"  FAIL  {method}: answer {answer['code']} ({'cancelled' if answer['code'] == 1 else 'failed'})")
        sys.exit(1)
    return answer["results"]

def props():
    return prop("version"), prop("AvailableSourceTypes"), prop("AvailableCursorModes")

try:
    version, types, cursors = props()
except GLib.Error as e:
    # The portal reads its config when it starts: if tools/install-portals.sh ran after kompact
    # started, the running portal still has ScreenCast=none. Start both portals again.
    print(f"  ..    the running portal has no ScreenCast ({e.message.split(': ')[-1]}); starting the portals again")
    import subprocess
    subprocess.run(["systemctl", "--user", "stop", "xdg-desktop-portal-wlr.service"])
    subprocess.run(["systemctl", "--user", "restart", "xdg-desktop-portal.service"])
    for _ in range(20):
        time.sleep(0.5)
        try:
            version, types, cursors = props()
            break
        except GLib.Error as e2:
            err = e2.message
    else:
        print(f"  FAIL  the portal still offers no ScreenCast interface ({err})")
        print("        check: tools/install-portals.sh ran (ScreenCast=wlr) and xdg-desktop-portal-wlr is installed")
        sys.exit(1)
print(f"  OK    the portal offers ScreenCast (version {version}, sources {types}, cursor modes {cursors})")

t0 = time.monotonic()
session = request("CreateSession", [{"session_handle_token": GLib.Variant("s", f"kompact9e{os.getpid()}")}])["session_handle"]
opts = {"types": GLib.Variant("u", 1), "multiple": GLib.Variant("b", False)}
if cursors & 2:
    opts["cursor_mode"] = GLib.Variant("u", 2)  # embedded: the pointer is in the picture
request("SelectSources", [session, opts])
print("  ..    click the monitor to share")
streams = request("Start", [session, "", {}])["streams"]
node, props = streams[0]
print(f"  OK    sharing started in {time.monotonic() - t0:.1f} s: PipeWire node {node}, {props}")

msg = bus.call_with_unix_fd_list_sync(PORTAL, PATH, CAST, "OpenPipeWireRemote",
                                      GLib.Variant("(oa{sv})", (session, {})), GLib.VariantType("(h)"),
                                      0, 10000, None, None)
fd = msg[1].get(msg[0].unpack()[0])

Gst.init(None)
# always-copy: the frame is copied out and the PipeWire buffer goes back to the portal at once;
# the portal asks kompact for the next frame only when it has a free buffer.
# The video/x-raw caps tell PipeWire this is a video stream (media.type); with a bare fakesink
# WirePlumber can't link it ("target not found").
pipeline = Gst.parse_launch(f"pipewiresrc fd={fd} path={node} always-copy=true ! video/x-raw ! fakesink name=count signal-handoffs=true sync=false")
times, caps, white = [], [], []
size = []
saved = {}  # second -> (frame bytes): the first frame of each second, written as PNG at the end
def on_frame(sink, buf, pad):
    times.append(time.monotonic())
    if not caps:
        c = pad.get_current_caps()
        caps.append(c.to_string())
        st = c.get_structure(0)
        size.extend([st.get_value("width"), st.get_value("height")])
    # A sanity check of the pixels: how many of 64 spread-out samples are pure white.
    w, h = size
    hits = 0
    for gy in range(8):
        for gx in range(8):
            x, y = (2 * gx + 1) * w // 16, (2 * gy + 1) * h // 16
            px = buf.extract_dup((y * w + x) * 4, 3)
            hits += px == b"\xff\xff\xff"
    white.append(hits)
    sec = int(times[-1] - start)
    if sec not in saved:
        saved[sec] = buf.extract_dup(0, buf.get_size())
pipeline.get_by_name("count").connect("handoff", on_frame)
errors = []
def on_message(_bus, m):
    if m.type == Gst.MessageType.ERROR:
        errors.append(m.parse_error()[0].message)
        loop.quit()
    elif m.type == Gst.MessageType.EOS:
        errors.append(f"the stream ended after {time.monotonic() - start:.1f} s")
        loop.quit()
gbus = pipeline.get_bus()
gbus.add_signal_watch()
gbus.connect("message", on_message)
start = time.monotonic()
pipeline.set_state(Gst.State.PLAYING)
print("  ..    0-5 s: hands off")
GLib.timeout_add_seconds(5, lambda: print("  ..    5-15 s: move a window around the shared monitor") and False)
GLib.timeout_add_seconds(15, loop.quit)
loop.run()
pipeline.set_state(Gst.State.NULL)
bus.call_sync(PORTAL, session, "org.freedesktop.portal.Session", "Close", None, None, 0, 5000, None)

if errors:
    print(f"  FAIL  PipeWire stream: {errors[0]}")
    sys.exit(1)
if not times:
    print("  FAIL  no frames arrived in 15 s")
    sys.exit(1)
per_s = [0] * 15
for t in times:
    per_s[min(14, int(t - start))] += 1
gaps = sorted(b - a for a, b in zip(times, times[1:])) or [0]
pct = lambda q: gaps[min(len(gaps) - 1, int(q * len(gaps)))] * 1000
print(f"  OK    {len(times)} frames in 15 s; per second: {' '.join(map(str, per_s))}")
print(f"        moving part (5-15 s): {sum(per_s[5:]) / 10:.1f} frames/s; gaps p50 {pct(0.5):.0f} / p95 {pct(0.95):.0f} / max {gaps[-1] * 1000:.0f} ms")
print(f"        format: {caps[0] if caps else '?'}")
bad = [i for i, n in enumerate(white) if n > 16]
if bad:
    print(f"  WARN  {len(bad)} frames look mostly white (over 16 of 64 samples pure white): "
          f"frames {bad[:20]}{' ...' if len(bad) > 20 else ''}, at {', '.join(f'{times[i] - start:.2f} s' for i in bad[:5])}")
else:
    print(f"  OK    no frame looks mostly white (most white samples in a frame: {max(white)} of 64)")
import struct, zlib
def write_png(path, w, h, data):
    stride = len(data) // h
    rows = bytearray()
    for y in range(h):
        px = data[y * stride:y * stride + w * 4]  # B, G, R, x
        rgb = bytearray(w * 3)
        rgb[0::3], rgb[1::3], rgb[2::3] = px[2::4], px[1::4], px[0::4]
        rows += b"\0" + rgb
    chunk = lambda t, d: struct.pack(">I", len(d)) + t + d + struct.pack(">I", zlib.crc32(t + d))
    with open(path, "wb") as f:
        f.write(b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", w, h, 8, 2, 0, 0, 0))
                + chunk(b"IDAT", zlib.compress(bytes(rows), 3)) + chunk(b"IEND", b""))
for sec, data in sorted(saved.items()):
    write_png(f"/tmp/kompact-9e/screencast-{sec:02d}.png", size[0], size[1], data)
print(f"  {'OK  ' if saved else 'FAIL'}  {len(saved)} frames saved (the first of each second): "
      f"/tmp/kompact-9e/screencast-{min(saved, default=0):02d}.png ...")
PY
status=$?
kill "$xdpw" 2>/dev/null
log=/tmp/kompact-9e/xdpw.log
echo "wlr portal (debug log $log):"
for what in "out of buffers" "not streaming" "buffer already exported" "unable to export buffer" "buffer constraints changed" "frame failed" "Failed"; do
    n=$(grep -ci "$what" "$log")
    [ "$n" -gt 0 ] && echo "  $n x '$what'"
done
grep -iE "error|warn" "$log" | sort | uniq -c | sort -rn | head -5 | sed 's/^/  /'
exit $status
