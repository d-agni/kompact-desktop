#!/bin/sh
# 10b: change shortcuts through the shell channel, the way the settings window will.
# Run it in foot inside kompact (any preset). It follows the steps and asks you to press keys.
# At the end the shortcuts are put back as they were (with no keys file, it's removed again).
#   sh ~/Documents/kompact-de/kompact/tools/test-keys.sh
if [ -z "${WAYLAND_DISPLAY:-}" ] || [ -z "${XDG_RUNTIME_DIR:-}" ]; then
    echo "Run this in a terminal inside kompact."
    exit 1
fi
keys=${XDG_CONFIG_HOME:-$HOME/.config}/kompact/keys
if [ -f "$keys" ]; then
    echo "You already have $keys; this test would change it. Move it away first (and back after)."
    exit 1
fi
exec python3 -I - "$XDG_RUNTIME_DIR/kompact-$WAYLAND_DISPLAY.sock" "$keys" <<'PY'
import json, os, socket, sys, time

path, keys_file = sys.argv[1], sys.argv[2]
import atexit
def put_back():
    # Whatever happened, leave the shortcuts as they were: the original preset, no keys file.
    try:
        s.sendall(f"keys preset {orig} drop\n".encode())
        time.sleep(0.5)
    except Exception:
        pass
    try:
        os.remove(keys_file)
    except FileNotFoundError:
        pass
orig = "macos"
atexit.register(put_back)
s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
s.connect(path)
s.settimeout(0.2)
buf = b""
passed = failed = 0

def messages(wait):
    """Messages kompact sends within `wait` seconds (only the keys ones)."""
    global buf
    end = time.monotonic() + wait
    out = []
    while time.monotonic() < end:
        try:
            chunk = s.recv(65536)
            if not chunk:
                break
            buf += chunk
        except socket.timeout:
            pass
        while b"\n" in buf:
            line, buf = buf.split(b"\n", 1)
            try:
                m = json.loads(line)
            except ValueError:
                continue
            if m.get("type") in ("keymap", "keys_result", "recorded"):
                out.append(m)
        if out and out[-1].get("type") in ("keymap", "recorded") and wait < 30:
            break
    return out

def send(line, wait=2.0):
    s.sendall((line + "\n").encode())
    return messages(wait)

def first(ms, kind):
    return next((m for m in ms if m.get("type") == kind), None)

def keys_of(keymap, action):
    return next((a["keys"] for a in keymap["actions"] if a["id"] == action), None)

def check(name, ok, detail=""):
    global passed, failed
    print(f"  {'OK  ' if ok else 'FAIL'}  {name}{('  ' + detail) if detail else ''}")
    if ok:
        passed += 1
    else:
        failed += 1

def ask(text):
    # The program itself comes in on stdin, so the answer is read from the terminal.
    print(f"  ..    {text} (then Enter here) ", end="", flush=True)
    with open("/dev/tty") as tty:
        tty.readline()

# Wait for what kompact sends a new client (the keymap among it).
km = first(messages(1.0), "keymap") or first(send("keys"), "keymap")
check("kompact sends the keymap", km is not None)
if km is None:
    sys.exit(1)
orig = km["preset"]
print(f"        preset {orig}, {len(km['actions'])} actions; window.close = {keys_of(km, 'window.close')}")

ms = send("keys bind window.close Super+W")
r, km = first(ms, "keys_result"), first(ms, "keymap") or km
check("bind window.close Super+W", r and r["ok"] and keys_of(km, "window.close") == ["Super+W"], r and r["message"])
check("the keys file was written", os.path.exists(keys_file))
ask("Press Super+W on a window: it should close")

r = first(send("keys bind app.files Super+W"), "keys_result")
check("a taken key is refused", r and not r["ok"], r and r["message"])
r = first(send("keys bind session.quit none"), "keys_result")
check("Quit can't be left without a key", r and not r["ok"], r and r["message"])
r = first(send("keys bind snap.left Ctrl+Alt+F3"), "keys_result")
check("Ctrl+Alt+F3 stays the console switch", r and not r["ok"], r and r["message"])

ask("Keep this terminal in front. Next: after Enter, press Ctrl+Super+K within 10 s")
s.sendall(b"keys record\n")
print("  ..    recording now: press Ctrl+Super+K")
rec = first(messages(12), "recorded")
check("recorded Ctrl+Super+K", rec and rec.get("keys") == "Ctrl+Super+K", json.dumps(rec))
ask("Next: after Enter, press Ctrl+Alt+Super+K within 10 s (if nothing comes, your keyboard may not send that one)")
s.sendall(b"keys record\n")
print("  ..    recording now: press Ctrl+Alt+Super+K")
rec4 = first(messages(12), "recorded")
check("recorded Ctrl+Alt+Super+K", rec4 and rec4.get("keys") == "Ctrl+Alt+Super+K", json.dumps(rec4))
rec = rec if rec and rec.get("keys") else rec4
if rec and rec.get("keys"):
    ms = send(f"keys bind app.files {rec['keys']}")
    r = first(ms, "keys_result")
    check(f"bind app.files {rec['keys']}", r and r["ok"], r and r["message"])
    ask(f"Press {rec['keys']}: Files should open")

ask("Next: after Enter, press Esc within 10 s")
s.sendall(b"keys record\n")
print("  ..    recording now: press Esc")
rec = first(messages(12), "recorded")
check("Esc cancels a recording", rec and rec.get("cancelled") == "Esc", json.dumps(rec))

ask("Next: after Enter, press NOTHING for 10 s")
s.sendall(b"keys record\n")
print("  ..    recording now: hands off for 10 s")
rec = first(messages(14), "recorded")
check("a recording gives up after 10 s", rec and rec.get("cancelled") == "timeout", json.dumps(rec))

ms = send("keys preset windows keep")
r, km = first(ms, "keys_result"), first(ms, "keymap") or km
check("preset windows, changes kept", r and r["ok"] and km["preset"] == "windows" and keys_of(km, "window.close") == ["Super+W"], r and r["message"])
ask("Press Alt+Tab: it should switch windows (Windows preset)")

ms = send("keys reset window.close")
r, km = first(ms, "keys_result"), first(ms, "keymap") or km
check("reset window.close to the preset's", r and r["ok"] and keys_of(km, "window.close") == ["Alt+F4"], r and r["message"])

ms = send(f"keys preset {orig} drop")
r, km = first(ms, "keys_result"), first(ms, "keymap") or km
check(f"back to preset {orig}, changes dropped", r and r["ok"] and km["preset"] == orig and not any(a["changed"] for a in km["actions"]), r and r["message"])
try:
    os.remove(keys_file)
    print(f"        removed {keys_file} again (there was none before)")
except FileNotFoundError:
    pass
print(f"{passed} passed, {failed} failed")
PY
