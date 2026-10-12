#!/bin/sh
# 9d: ask the desktop portal for a screenshot, as Flatpak apps do (run it in foot inside
# kompact, after tools/install-portals.sh). The portal answers with a file; nothing else changes.
#   sh ~/Documents/kompact-de/kompact/tools/test-portal.sh
if [ -z "${WAYLAND_DISPLAY:-}" ]; then
    echo "Run this in a terminal inside kompact (WAYLAND_DISPLAY is not set)."
    exit 1
fi
echo "XDG_CURRENT_DESKTOP=${XDG_CURRENT_DESKTOP:-(unset)}"
exec python3 -I - <<'PY'
import os, subprocess, sys, time
import gi
gi.require_version("Gio", "2.0")
from gi.repository import Gio, GLib

bus = Gio.bus_get_sync(Gio.BusType.SESSION, None)
try:
    v = bus.call_sync("org.freedesktop.portal.Desktop", "/org/freedesktop/portal/desktop",
                      "org.freedesktop.DBus.Properties", "Get",
                      GLib.Variant("(ss)", ("org.freedesktop.portal.Screenshot", "version")),
                      GLib.VariantType("(v)"), 0, 10000, None)
    print(f"  OK    the portal offers Screenshot (version {v.unpack()[0]})")
except GLib.Error as e:
    print(f"  FAIL  the portal offers no Screenshot interface ({e.message})")
    print("        check: tools/install-portals.sh ran, xdg-desktop-portal-wlr installed, kompact restarted")
    sys.exit(1)

token = f"kompact9d{os.getpid()}"
sender = bus.get_unique_name()[1:].replace(".", "_")
handle = f"/org/freedesktop/portal/desktop/request/{sender}/{token}"
loop = GLib.MainLoop()
result = {}

def on_response(conn, sender_name, path, iface, signal, params):
    code, results = params.unpack()
    result["code"], result["results"] = code, results
    loop.quit()

bus.signal_subscribe("org.freedesktop.portal.Desktop", "org.freedesktop.portal.Request", "Response",
                     handle, None, Gio.DBusSignalFlags.NO_MATCH_RULE, on_response)
# A match rule so the bus delivers the signal to us.
bus.call_sync("org.freedesktop.DBus", "/org/freedesktop/DBus", "org.freedesktop.DBus", "AddMatch",
              GLib.Variant("(s)", (f"type='signal',interface='org.freedesktop.portal.Request',path='{handle}'",)),
              None, 0, 5000, None)
started = time.monotonic()
bus.call_sync("org.freedesktop.portal.Desktop", "/org/freedesktop/portal/desktop",
              "org.freedesktop.portal.Screenshot", "Screenshot",
              GLib.Variant("(sa{sv})", ("", {"handle_token": GLib.Variant("s", token),
                                             "interactive": GLib.Variant("b", False)})),
              GLib.VariantType("(o)"), 0, 30000, None)
GLib.timeout_add_seconds(30, loop.quit)
loop.run()
took = (time.monotonic() - started) * 1000
if "code" not in result:
    print("  FAIL  no answer from the portal within 30 s")
    sys.exit(1)
codes = {0: "success", 1: "cancelled", 2: "failed"}
uri = result["results"].get("uri", "")
print(f"  {'OK  ' if result['code'] == 0 else 'FAIL'}  answer: {codes.get(result['code'], result['code'])} in {took:.0f} ms, {uri or 'no file'}")
if result["code"] == 0 and uri.startswith("file://"):
    path = GLib.filename_from_uri(uri)[0]
    info = subprocess.run(["file", "-b", path], capture_output=True, text=True).stdout.strip()
    print(f"        {info}")
    print(f"        open it: xdg-open '{path}'")
sys.exit(0 if result["code"] == 0 else 1)
PY
