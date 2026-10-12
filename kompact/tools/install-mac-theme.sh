#!/bin/sh
# 6m-f: a macOS-like look for apps in the kompact session only: the WhiteSur GTK theme, icons
# and cursors (github.com/vinceliuice, pinned below), installed in your home, no sudo.
#
#   sh ~/Documents/kompact-de/kompact/tools/install-mac-theme.sh          # dark (default)
#   sh ~/Documents/kompact-de/kompact/tools/install-mac-theme.sh light    # light, like stock macOS
#   sh ~/Documents/kompact-de/kompact/tools/install-mac-theme.sh remove   # undo all of it
#
# Then quit kompact (Super+Escape) and start it again.
#
# How it stays out of your GNOME/Ubuntu session: the choice goes into a dconf profile of its own,
# ~/.config/kompact/dconf/profile, which only kompact uses (it sets DCONF_PROFILE; see
# kompact/src/theme.rs). That profile reads all your usual settings, but locks the theme, icons,
# cursor, dark/light and the title-bar buttons (close, minimize, maximize on the left) to the
# values below. KOMPACT_THEME=off ./ktest-desktop.sh starts kompact without it.
#
# Snap apps (Firefox) can't see themes in your home and keep their own look.

set -eu

GTK_TAG=2026-09-10
ICON_TAG=2026-09-10
CURSOR_COMMIT=e190baf618ed95ee217d2fd45589bd309b37672b
BUTTONS='close,minimize,maximize:'

DATA=${XDG_DATA_HOME:-$HOME/.local/share}
CONF=${XDG_CONFIG_HOME:-$HOME/.config}/kompact/dconf
THEMES=$DATA/themes
ICONS=$DATA/icons

mode=${1:-dark}
case $mode in
    dark) GTK=WhiteSur-Dark ICON=WhiteSur-dark SCHEME=prefer-dark ;;
    light) GTK=WhiteSur-Light ICON=WhiteSur-light SCHEME=default ;;
    remove)
        rm -rf "$THEMES"/WhiteSur-Dark "$THEMES"/WhiteSur-Light \
            "$ICONS"/WhiteSur "$ICONS"/WhiteSur-dark "$ICONS"/WhiteSur-light "$ICONS"/WhiteSur-cursors \
            "$CONF"
        echo "removed the WhiteSur theme, icons, cursors and $CONF"
        echo "quit kompact (Super+Escape) and start it again"
        exit 0
        ;;
    *)
        echo "usage: sh $0 [dark|light|remove]" >&2
        exit 2
        ;;
esac

for tool in curl tar xz bash dconf gsettings; do
    command -v "$tool" >/dev/null || { echo "needs $tool" >&2; exit 1; }
done

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

fetch() { # repo ref dir
    echo "downloading $1 ($2)"
    mkdir -p "$tmp/$3"
    curl -fsSL --retry 2 "https://codeload.github.com/vinceliuice/$1/tar.gz/$2" | tar xz -C "$tmp/$3" --strip-components=1
}
fetch WhiteSur-gtk-theme "refs/tags/$GTK_TAG" gtk
fetch WhiteSur-icon-theme "refs/tags/$ICON_TAG" icon
fetch WhiteSur-cursors "$CURSOR_COMMIT" cursors

# GTK theme: the project's prebuilt archives (its installer would need sassc).
mkdir -p "$THEMES"
for t in WhiteSur-Dark WhiteSur-Light; do
    rm -rf "${THEMES:?}/$t"
    tar xJf "$tmp/gtk/release/$t.tar.xz" -C "$THEMES"
done
echo "installed WhiteSur-Dark and WhiteSur-Light in $THEMES"

# Icons: the project's installer, which copies into the folder given (WhiteSur, -light, -dark).
mkdir -p "$ICONS"
(cd "$tmp/icon" && bash ./install.sh -d "$ICONS" >/dev/null)
echo "installed the WhiteSur icons in $ICONS"

# Cursors: ready-made in dist/.
rm -rf "${ICONS:?}/WhiteSur-cursors"
cp -r "$tmp/cursors/dist" "$ICONS/WhiteSur-cursors"
echo "installed WhiteSur-cursors in $ICONS"

# The kompact-only dconf profile: your settings, with these keys locked.
mkdir -p "$CONF/kompact.d/locks"
cat >"$CONF/kompact.d/00-kompact" <<KEYS
# kompact session only (kompact/tools/install-mac-theme.sh $mode)
[org/gnome/desktop/interface]
gtk-theme='$GTK'
icon-theme='$ICON'
cursor-theme='WhiteSur-cursors'
color-scheme='$SCHEME'

[org/gnome/desktop/wm/preferences]
button-layout='$BUTTONS'
KEYS
cat >"$CONF/kompact.d/locks/kompact" <<LOCKS
/org/gnome/desktop/interface/gtk-theme
/org/gnome/desktop/interface/icon-theme
/org/gnome/desktop/interface/cursor-theme
/org/gnome/desktop/interface/color-scheme
/org/gnome/desktop/wm/preferences/button-layout
LOCKS
dconf compile "$CONF/kompact" "$CONF/kompact.d"
printf 'user-db:user\nfile-db:%s\n' "$CONF/kompact" >"$CONF/profile"

echo
echo "kompact session will use:"
for key in gtk-theme icon-theme cursor-theme color-scheme; do
    echo "  $key: $(DCONF_PROFILE="$CONF/profile" gsettings get org.gnome.desktop.interface $key)"
done
echo "  button-layout: $(DCONF_PROFILE="$CONF/profile" gsettings get org.gnome.desktop.wm.preferences button-layout)"
echo "your GNOME session keeps: $(gsettings get org.gnome.desktop.interface gtk-theme)"
echo
echo "quit kompact (Super+Escape) and start it again"
