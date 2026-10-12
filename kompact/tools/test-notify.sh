#!/usr/bin/env bash
# 8c: send test notifications to the shell's notification server. Run it in a terminal inside
# kompact. Watch the top-right corner (numbers go in typed, "int32 -1": a bare -1 is taken
# for a gdbus option); the replies the server sends (closed, actions) are
# recorded and checked at the end. Nothing is changed. Log: notify-test.log in kompact/.
set -u
cd "$(dirname "$0")/.." || exit 1
log=notify-test.log
[ -n "${WAYLAND_DISPLAY:-}" ] || { echo "Run this in a terminal inside kompact."; exit 1; }
dest=org.freedesktop.Notifications
path=/org/freedesktop/Notifications

say() { echo; echo ">>> $*"; }
notify() { # app summary body actions hints timeout [replaces]
    gdbus call --session --dest $dest --object-path $path --method $dest.Notify \
        "$1" "uint32 ${7:-0}" "dialog-information" "$2" "$3" "$4" "$5" "int32 $6" | sed -n 's/^(uint32 \([0-9]*\),)$/\1/p'
}

: > "$log"
dbus-monitor --session "type='signal',interface='$dest'" >> "$log" 2>&1 &
mon=$!
trap 'kill $mon 2>/dev/null' EXIT
sleep 0.5

info=$(gdbus call --session --dest $dest --object-path $path --method $dest.GetServerInformation 2>&1)
echo "Server: $info"
case "$info" in *kompact*) ;; *) echo "The shell isn't serving notifications (another daemon, or KOMPACT_SHELL_NOTIFICATIONS=off)."; exit 1 ;; esac

say "1/5 A plain banner (goes away by itself after 5 s)."
id1=$(notify "Test" "Hello from kompact" "A plain notification with <b>markup</b> &amp; text." "[]" "{}" -1)
sleep 7

say "2/5 With buttons: click 'Reply' (or the banner itself) within 10 s."
id2=$(notify "Mail" "New message" "Click Reply, or the banner." "['default','Open','reply','Reply']" "{}" 0)
sleep 10

say "3/5 Critical: stays until you click its close button (hover its top-left corner). 10 s."
id3=$(notify "Battery" "Battery low" "Critical notifications stay." "[]" "{'urgency': <byte 2>}" -1)
sleep 10

say "4/5 Replaced in place: the text changes after 2 s, then the app closes it after 4 s."
id4=$(notify "Build" "Building…" "step 1 of 2" "[]" "{}" 0)
sleep 2
notify "Build" "Build finished" "step 2 of 2" "[]" "{}" 0 "$id4" >/dev/null
sleep 2
gdbus call --session --dest $dest --object-path $path --method $dest.CloseNotification "$id4" >/dev/null
sleep 1

say "5/5 notify-send (libnotify), as apps send them."
command -v notify-send >/dev/null && notify-send -a "notify-send" "From notify-send" "Using the desktop-entry icon" -h string:desktop-entry:org.gnome.Nautilus
sleep 6

kill $mon 2>/dev/null
echo
echo "Ids: $id1 $id2 $id3 $id4"
# NotificationClosed carries (id, reason): the two uint32 lines after the signal line.
closed() {
    awk -v id="$1" '/member=NotificationClosed/ {n = 0; want = 1; next}
        want && /uint32/ {v[n++] = $2; if (n == 2) {if (v[0] == id) r = v[1]; want = 0}}
        END {print r}' "$log"
}
echo "1 closed with reason $(closed "$id1") (1 = expired)"
echo "2 closed with reason $(closed "$id2") (2 = dismissed), actions: $(grep -A2 'member=ActionInvoked' "$log" | grep -o 'string "[a-z]*"' | tr '\n' ' ')"
echo "3 closed with reason $(closed "$id3") (2 = dismissed; empty = still open)"
echo "4 closed with reason $(closed "$id4") (3 = closed by the app)"
echo "Signals: $PWD/$log"
