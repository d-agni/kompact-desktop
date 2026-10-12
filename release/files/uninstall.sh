#!/bin/sh
# Removes kompact (installed as /usr/local/sbin/kompact-uninstall by install.sh):
#   sudo kompact-uninstall
# Puts the old login screen back first if kompact's was on. The packages install.sh added stay
# (other programs may use them; `sudo apt autoremove` tidies what nothing needs). Your home folder
# is left alone: ~/.config/kompact (settings) and ~/.local/state/kompact (logs) are yours to delete.
set -eu

state=/var/lib/kompact
manifest=$state/installed-files

[ "$(id -u)" = 0 ] || { echo "run it with sudo: sudo $0" >&2; exit 1; }
[ -f "$manifest" ] || { echo "kompact isn't installed (no $manifest)" >&2; exit 1; }

if systemctl is-enabled --quiet greetd 2>/dev/null && grep -q kompact-greeter-session /etc/greetd/config.toml 2>/dev/null; then
    /usr/local/sbin/kompact-login-screen disable
fi
while read -r f; do
    case "$f" in
        /usr/local/bin/* | /usr/local/sbin/* | /usr/share/wayland-sessions/kompact.desktop | /usr/share/glib-2.0/schemas/90_kompact.gschema.override)
            rm -f "$f" ;;
        *) echo "skipped $f (not one of kompact's places)" ;;
    esac
done <"$manifest"
glib-compile-schemas /usr/share/glib-2.0/schemas 2>/dev/null || true
rm -rf "$state"
echo "kompact is removed. Log out and pick another session; your files and settings are untouched."
