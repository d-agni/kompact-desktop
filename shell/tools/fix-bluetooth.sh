#!/usr/bin/env bash
# fix/bluetooth: the system side of Bluetooth on this PC, found 2026-10-25 (BlueZ 5.85, a CSR
# 8510 USB dongle, Bluetooth 4.0, 00:02:5B:…):
#
#  1. USB autosuspend was on for the dongle (btusb enable_autosuspend=Y, power/control=auto):
#     asleep, it misses a headset calling back, so devices didn't come back by themselves.
#  2. BlueZ ran in dual mode (BR/EDR + LE): dual-mode earbuds (OnePlus Nord Buds 2r) can be tried
#     over LE, which a Bluetooth 4.0 dongle can't finish ("Pair device failed: Timeout").
#     Classic only (ControllerMode = bredr) pairs headphones the way their sound goes. LE-only
#     gadgets (some fitness bands, LE mice) stop working: keep LE with --keep-le.
#  3. FastConnectable = true (answers a device calling sooner), JustWorksRepairing = confirm (a
#     device that forgot this PC may pair again, after you confirm), and BlueZ's reconnect
#     policy written out (7 tries after a link drops).
#
#   sudo shell/tools/fix-bluetooth.sh            # all of it, then restarts Bluetooth (devices
#                                                # disconnect for a moment)
#   sudo shell/tools/fix-bluetooth.sh --keep-le  # the same, but BlueZ stays dual mode
#   shell/tools/fix-bluetooth.sh --check         # what's set now (no sudo needed)
#   sudo shell/tools/fix-bluetooth.sh --undo     # back as it was (main.conf from its backup)
set -euo pipefail

conf=/etc/bluetooth/main.conf
backup=/etc/bluetooth/main.conf.kompact-orig
modprobe=/etc/modprobe.d/kompact-btusb.conf
mode=fix
keep_le=
for a in "$@"; do
    case "$a" in
        --check) mode=check ;;
        --undo) mode=undo ;;
        --keep-le) keep_le=1 ;;
        -h | --help) sed -n '2,23p' "$0"; exit 0 ;;
        *) echo "unknown option: $a (try --help)" >&2; exit 2 ;;
    esac
done

# The dongle's USB device (the parent of the btusb interface).
dongle() {
    local i
    for i in /sys/bus/usb/drivers/btusb/*:1.0; do
        [[ -e "$i" ]] || continue
        readlink -f "$i/.."
        return
    done
}

conf_value() { # section key -> the value set (not commented), or "(default)"
    python3 -I - "$conf" "$1" "$2" <<'EOF'
import sys
path, section, key = sys.argv[1:]
cur, value = None, "(default)"
for line in open(path, encoding="utf-8", errors="replace"):
    s = line.strip()
    if s.startswith("[") and s.endswith("]"):
        cur = s[1:-1]
    elif cur == section and not s.startswith("#") and "=" in s and s.split("=", 1)[0].strip() == key:
        value = s.split("=", 1)[1].strip()
print(value)
EOF
}

check() {
    local d
    d=$(dongle)
    echo "adapter:        $(bluetoothctl show 2>/dev/null | sed -n 's/^Controller //p' | head -1)"
    echo "btusb autosuspend (module): $(cat /sys/module/btusb/parameters/enable_autosuspend 2>/dev/null || echo ?)"
    [[ -n "$d" ]] && echo "dongle power/control: $(cat "$d/power/control" 2>/dev/null) ($d)"
    [[ -f "$modprobe" ]] && echo "modprobe file:  $modprobe ($(cat "$modprobe"))" || echo "modprobe file:  none"
    for k in "General ControllerMode" "General FastConnectable" "General JustWorksRepairing" "Policy ReconnectAttempts" "Policy AutoEnable"; do
        set -- $k
        printf '%-15s %s\n' "$2:" "$(conf_value "$1" "$2")"
    done
    [[ -f "$backup" ]] && echo "backup:         $backup" || true
}

# set_conf section key value: uncomments / replaces the key in its section, or adds it there.
set_conf() {
    python3 -I - "$conf" "$1" "$2" "$3" <<'EOF'
import sys
path, section, key, value = sys.argv[1:]
lines = open(path, encoding="utf-8").read().split("\n")
out, cur, done, start = [], None, False, None
for line in lines:
    s = line.strip()
    if s.startswith("[") and s.endswith("]"):
        if cur == section and not done:
            out.append(f"{key} = {value}")
            done = True
        cur = s[1:-1]
        out.append(line)
        continue
    bare = s.lstrip("#").strip()
    if cur == section and not done and "=" in bare and bare.split("=", 1)[0].strip() == key:
        out.append(f"{key} = {value}")
        done = True
        continue
    out.append(line)
if not done:
    if cur != section:
        out += ["", f"[{section}]"]
    out.append(f"{key} = {value}")
open(path, "w", encoding="utf-8").write("\n".join(out))
EOF
}

if [[ $mode == check ]]; then
    check
    exit 0
fi
[[ "$(id -u)" == 0 ]] || { echo "run it with sudo: sudo $0 $*" >&2; exit 1; }

if [[ $mode == undo ]]; then
    if [[ -f "$backup" ]]; then
        cp -a "$backup" "$conf"
        echo "restored $conf from $backup"
    fi
    rm -f "$modprobe"
    d=$(dongle)
    [[ -n "$d" ]] && echo auto >"$d/power/control" || true
    systemctl restart bluetooth
    echo "Bluetooth is back as it was (autosuspend returns after the next boot)."
    exit 0
fi

echo "==> 1. USB autosuspend off for the Bluetooth dongle"
echo "options btusb enable_autosuspend=0" >"$modprobe"
echo N >/sys/module/btusb/parameters/enable_autosuspend 2>/dev/null || true
d=$(dongle)
if [[ -n "$d" ]]; then
    echo on >"$d/power/control"
    echo "   $d: power/control = $(cat "$d/power/control")"
else
    echo "   (no btusb dongle found now; the setting applies when it's plugged in)"
fi

echo "==> 2-3. BlueZ settings ($conf; the original kept as $backup)"
[[ -f "$backup" ]] || cp -a "$conf" "$backup"
if [[ -z "$keep_le" ]]; then
    set_conf General ControllerMode bredr
else
    echo "   keeping dual mode (--keep-le)"
fi
set_conf General FastConnectable true
set_conf General JustWorksRepairing confirm
set_conf Policy ReconnectAttempts 7
set_conf Policy ReconnectIntervals "1,2,4,8,16,32,64"
set_conf Policy AutoEnable true

echo "==> Restarting Bluetooth (devices disconnect for a moment)"
systemctl restart bluetooth
sleep 2
check
echo
echo "Done. Turn your headphones off and on: they should come back by themselves. To pair the"
echo "OnePlus: put it in pairing mode, then System Settings → Bluetooth → Pair."
