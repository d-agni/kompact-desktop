#!/bin/sh
# 6m-0: GNOME settings for the kompact session only (XDG_CURRENT_DESKTOP=kompact, which kompact
# sets on the DRM backend). Installs a GSettings "vendor override" that applies to the kompact
# desktop alone, so your GNOME/Ubuntu session is untouched:
#   - title-bar buttons: close, minimize, maximize, on the left like macOS (Ubuntu only turns
#     minimize/maximize on for its own "ubuntu" desktop, so under kompact GTK apps such as
#     Firefox and gedit showed a close button only).
#
#   sudo sh ~/Documents/kompact-de/kompact/tools/install-desktop-settings.sh          # install
#   sudo sh ~/Documents/kompact-de/kompact/tools/install-desktop-settings.sh remove   # undo
#
# Prefer buttons on the right? Change LAYOUT to ':minimize,maximize,close' and run it again.

set -eu

LAYOUT='close,minimize,maximize:'
DIR=/usr/share/glib-2.0/schemas
FILE=$DIR/90_kompact.gschema.override

if [ "$(id -u)" -ne 0 ]; then
    echo "run it with sudo: sudo sh $0 ${1:-}" >&2
    exit 1
fi

if [ "${1:-}" = remove ]; then
    rm -f "$FILE"
    glib-compile-schemas "$DIR"
    echo "removed $FILE"
    exit 0
fi

cat >"$FILE" <<OVERRIDE
# kompact (installed by kompact/tools/install-desktop-settings.sh); only for XDG_CURRENT_DESKTOP=kompact.
[org.gnome.desktop.wm.preferences:kompact]
button-layout='$LAYOUT'
OVERRIDE
glib-compile-schemas "$DIR"

got=$(XDG_CURRENT_DESKTOP=kompact GSETTINGS_BACKEND=memory gsettings get org.gnome.desktop.wm.preferences button-layout)
echo "installed $FILE"
echo "kompact session button-layout: $got"
user=$(sudo -u "${SUDO_USER:-root}" dconf read /org/gnome/desktop/wm/preferences/button-layout 2>/dev/null || true)
if [ -n "$user" ]; then
    echo "note: you have your own button-layout ($user), which wins over this; reset it with:"
    echo "  gsettings reset org.gnome.desktop.wm.preferences button-layout"
fi
