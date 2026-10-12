#!/bin/sh
# 9d/9e-b: screenshots and screen sharing through the desktop portal in kompact sessions.
# Writes two files, both read only in kompact sessions (your GNOME session is untouched):
#   ~/.config/xdg-desktop-portal/kompact-portals.conf  (xdg-desktop-portal, XDG_CURRENT_DESKTOP=kompact)
#     everything as before (the GTK portal), except Screenshot and ScreenCast -> wlr
#   ~/.config/xdg-desktop-portal-wlr/kompact          (xdg-desktop-portal-wlr, same rule)
#     at most N frames per second (default 10), and slurp to click the monitor to share
# Run it as yourself, not with sudo, after:
#   sudo apt install xdg-desktop-portal-wlr slurp
# Other frame rate:  sh tools/install-portals.sh --fps 20
# Undo:              sh tools/install-portals.sh --remove
set -eu
dir=${XDG_CONFIG_HOME:-$HOME/.config}/xdg-desktop-portal
conf=$dir/kompact-portals.conf
wlr_dir=${XDG_CONFIG_HOME:-$HOME/.config}/xdg-desktop-portal-wlr
wlr_conf=$wlr_dir/kompact
fps=10

case "${1:-}" in
--remove)
    for f in "$conf" "$wlr_conf"; do
        if [ -f "$f" ]; then
            rm "$f"
            echo "removed $f"
        fi
    done
    echo "kompact sessions fall back to the GTK portal again: no portal screenshots, no screen sharing"
    exit 0
    ;;
--fps)
    fps=${2:-}
    case "$fps" in
    '' | *[!0-9]* | 0) echo "--fps needs a whole number above 0"; exit 1 ;;
    esac
    ;;
'') ;;
*) echo "usage: $0 [--fps N | --remove]"; exit 1 ;;
esac

[ "$(id -u)" -ne 0 ] || { echo "run this as yourself, not root"; exit 1; }
[ -x /usr/libexec/xdg-desktop-portal-wlr ] || echo "note: xdg-desktop-portal-wlr isn't installed yet: sudo apt install xdg-desktop-portal-wlr"
command -v slurp >/dev/null || echo "note: slurp isn't installed (it picks the monitor to share): sudo apt install slurp"

keep() { # file: keep the old one once, as .before-9e
    if [ -f "$1" ] && [ ! -f "$1.before-9e" ]; then
        cp "$1" "$1.before-9e"
        echo "kept the old one as $1.before-9e"
    fi
}

mkdir -p "$dir" "$wlr_dir"
keep "$conf"
cat > "$conf" <<'CONF'
# kompact (9d, 9e-b), written by kompact/tools/install-portals.sh; only for XDG_CURRENT_DESKTOP=kompact.
# Same as kompact sessions had without this file (the GTK portal as the fallback), plus
# screenshots and screen sharing through xdg-desktop-portal-wlr (kompact offers wlr-screencopy).
[preferred]
default=gtk
org.freedesktop.impl.portal.Screenshot=wlr
org.freedesktop.impl.portal.ScreenCast=wlr
CONF
echo "wrote $conf:"
sed 's/^/  /' "$conf"

keep "$wlr_conf"
cat > "$wlr_conf" <<CONF
# kompact (9e-b), written by kompact/tools/install-portals.sh; only for XDG_CURRENT_DESKTOP=kompact.
# Every shared frame costs kompact ~17 ms of its main loop on the GT 710 (9e-a2), so cap the rate.
# Sharing asks which monitor: click it (slurp).
[screencast]
max_fps=$fps
chooser_type=simple
chooser_cmd=slurp -f %o -or
CONF
echo "wrote $wlr_conf:"
sed 's/^/  /' "$wlr_conf"
echo "Both are read when the portals start. kompact restarts xdg-desktop-portal at its start and"
echo "stops the wlr portal when it quits, so start kompact again (or, inside kompact:"
echo "systemctl --user restart xdg-desktop-portal.service; systemctl --user stop xdg-desktop-portal-wlr.service)."
