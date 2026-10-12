#!/bin/sh
# kompact installer (15b). From the unpacked release folder:
#   sudo ./install.sh                  # the kompact desktop: pick "kompact" on your login screen
#   sudo ./install.sh --login-screen   # also kompact's own login screen at boot (greetd), in
#                                      # place of your current one (GDM, …); back: see below
# For Ubuntu 24.04 LTS or newer (and Debian-based systems of the same age), 64-bit PC (x86_64),
# on a real machine or in a virtual machine with a virtual GPU (virtio-gpu, VMware SVGA, …).
#
# Installs: the packages kompact needs (apt), its programs in /usr/local/bin, the "kompact"
# session in /usr/share/wayland-sessions, title-bar button defaults for GTK apps, and
# /usr/local/sbin/kompact-uninstall (+ kompact-login-screen) to undo it later.
# Nothing in your home folder is changed; your files and settings stay.
set -eu

here=$(cd "$(dirname "$0")" && pwd)
version=$(cat "$here/VERSION")
bin=/usr/local/bin
sbin=/usr/local/sbin
state=/var/lib/kompact
manifest=$state/installed-files
login_screen=

for a in "$@"; do
    case "$a" in
        --login-screen) login_screen=1 ;;
        -h | --help) sed -n '2,13p' "$0"; exit 0 ;;
        *) echo "unknown option: $a (try --help)" >&2; exit 2 ;;
    esac
done

say() { printf '\n==> %s\n' "$*"; }
fail() { printf 'kompact install: %s\n' "$*" >&2; exit 1; }

[ "$(id -u)" = 0 ] || fail "run it with sudo: sudo $0 $*"
[ "$(uname -m)" = x86_64 ] || fail "this release is for 64-bit PCs (x86_64); this is $(uname -m)"
[ -r /etc/os-release ] || fail "can't tell which Linux this is (/etc/os-release)"
. /etc/os-release
case " ${ID:-} ${ID_LIKE:-} " in
    *" ubuntu "* | *" debian "*) ;;
    *) fail "this release is for Ubuntu/Debian systems; this is ${PRETTY_NAME:-unknown}" ;;
esac
command -v apt-get >/dev/null || fail "apt-get not found"
# The programs need glibc 2.39 (Ubuntu 24.04) or newer.
glibc=$(getconf GNU_LIBC_VERSION 2>/dev/null | awk '{print $2}')
need=$(cat "$here/GLIBC")
if [ -n "$glibc" ] && [ "$(printf '%s\n%s\n' "$need" "$glibc" | sort -V | head -1)" != "$need" ]; then
    fail "${PRETTY_NAME:-this system} has glibc $glibc; kompact $version needs $need or newer (Ubuntu 24.04 or newer)"
fi

echo "kompact desktop $version for ${PRETTY_NAME:-this system}"

say "Installing the packages kompact needs"
apt-get update -qq || echo "(apt-get update failed; trying with the package lists there are)"
# shellcheck disable=SC2046
DEBIAN_FRONTEND=noninteractive apt-get install -y $(grep -v '^#' "$here/packages-required.txt") ||
    fail "the required packages didn't install (see apt's message above)"
missing=
for p in $(grep -v '^#' "$here/packages-optional.txt"); do
    if ! DEBIAN_FRONTEND=noninteractive apt-get install -y "$p" >/dev/null 2>&1; then
        missing="$missing $p"
    fi
done
[ -z "$missing" ] || echo "not available here (kompact works without them, with less):$missing"

say "Copying kompact's programs to $bin"
mkdir -p "$state"
: >"$manifest.new"
put() { # mode source destination
    install -D -m "$1" "$2" "$3"
    echo "$3" >>"$manifest.new"
}
for f in "$here"/bin/*; do
    put 755 "$f" "$bin/$(basename "$f")"
done
put 755 "$here/uninstall.sh" "$sbin/kompact-uninstall"
put 755 "$here/login-screen.sh" "$sbin/kompact-login-screen"
cp "$here/VERSION" "$state/version"

say "Adding the kompact session to the login screen"
put 644 "$here/share/kompact.desktop" /usr/share/wayland-sessions/kompact.desktop
# A developer install's copy (tools/install-login.sh) would list kompact twice.
rm -f /usr/local/share/wayland-sessions/kompact.desktop
put 644 "$here/share/90_kompact.gschema.override" /usr/share/glib-2.0/schemas/90_kompact.gschema.override
glib-compile-schemas /usr/share/glib-2.0/schemas
# Where each user's lock-screen look is shared with kompact's login screen (sticky, like /tmp).
mkdir -p "$state/login"
chmod 1777 "$state/login"
mv -f "$manifest.new" "$manifest"

if [ -n "$login_screen" ]; then
    say "Setting up kompact's login screen"
    "$sbin/kompact-login-screen" enable
fi

echo
echo "kompact desktop $version is installed."
echo
echo "  - Log out. On the login screen, pick the \"kompact\" session (GDM: after choosing your name,"
echo "    the gear button at the bottom right), then log in."
if [ -n "$login_screen" ]; then
    echo "  - From the next boot, kompact's own login screen shows instead. Back to the old one:"
    echo "      sudo kompact-login-screen disable"
fi
echo "  - Super+Space searches apps, Super+Return opens a terminal; the logo menu at the top left"
echo "    has System Settings, Lock Screen and Log Out."
echo "  - Screen stays black? Ctrl+Alt+F3, log in, and read ~/.local/state/kompact/kompact.log."
echo "  - Uninstall: sudo kompact-uninstall"
