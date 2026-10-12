#!/bin/sh
# 15b: builds a kompact release: dist/kompact-<version>-x86_64.tar.gz (+ .sha256), with the four
# programs, the session scripts, the installer and the package lists. Run it as yourself:
#   release/make-release.sh
# The package lists come from this machine (dpkg -S on what the programs link to and run), so
# build on the oldest Ubuntu the release should support; GLIBC records the newest glibc the
# programs need (the installer refuses older systems).
set -eu

here=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$here/.." && pwd)
version=$(sed -n 's/^version = "\(.*\)"/\1/p' "$root/kompact/Cargo.toml" | head -1)
shell_version=$(sed -n 's/^version = "\(.*\)"/\1/p' "$root/shell/src-tauri/Cargo.toml" | head -1)
if [ "$version" != "$shell_version" ]; then
    echo "kompact is $version but the shell is $shell_version; give both the release's version" >&2
    exit 1
fi
name=kompact-desktop-$version-x86_64
dist=$root/dist
stage=$dist/$name

echo "==> building kompact desktop $version"
(cd "$root/kompact" && cargo test --quiet && cargo build --release --quiet)
(cd "$root/shell/src-tauri" && cargo test --quiet && cargo build --release --quiet)

echo "==> staging $stage"
rm -rf "$stage"
mkdir -p "$stage/bin" "$stage/share"
for p in "$root/kompact/target/release/kompact" \
         "$root/shell/src-tauri/target/release/kompact-shell" \
         "$root/shell/src-tauri/target/release/kompact-lock" \
         "$root/shell/src-tauri/target/release/kompact-greeter"; do
    install -m 755 "$p" "$stage/bin/"
    strip --strip-unneeded "$stage/bin/$(basename "$p")"
done
install -m 755 "$root/kompact/tools/login/kompact-session" "$stage/bin/"
install -m 755 "$root/kompact/tools/login/kompact-greeter-session" "$stage/bin/"
install -m 644 "$root/kompact/tools/login/kompact.desktop" "$stage/share/"
cat >"$stage/share/90_kompact.gschema.override" <<'EOF'
# kompact: macOS-style title-bar buttons on the left for GTK apps in kompact sessions only.
[org.gnome.desktop.wm.preferences:kompact]
button-layout='close,minimize,maximize:'
EOF
for f in install.sh uninstall.sh login-screen.sh README.md; do
    install -m "$(case "$f" in *.sh) echo 755 ;; *) echo 644 ;; esac)" "$here/files/$f" "$stage/$f"
done
echo "$version" >"$stage/VERSION"
install -m 644 "$root/LICENSE" "$stage/LICENSE"

# The newest glibc symbol version any program needs.
for p in "$stage"/bin/kompact*; do
    if file "$p" | grep -q ELF; then
        objdump -T "$p" | grep -oE 'GLIBC_[0-9.]+'
    fi
done | sed 's/GLIBC_//' | sort -V | tail -1 >"$stage/GLIBC"

pkg_of() { # a file path -> its Debian package (not -dev)
    [ -n "$1" ] || return 0
    dpkg -S "$(readlink -f "$1")" 2>/dev/null | grep -v -- '-dev:' | head -1 | cut -d: -f1 || true
}
{
    echo "# what kompact $version links to (from the build machine) and needs to run"
    for p in "$stage"/bin/kompact "$stage"/bin/kompact-shell "$stage"/bin/kompact-lock "$stage"/bin/kompact-greeter; do
        ldd "$p" | awk '/=>/ {print $1, $3}' | while read -r soname path; do
            if readelf -d "$p" | grep -q "NEEDED.*\[$soname\]"; then
                pkg_of "$path"
            fi
        done
    done
    # Loaded at run time: EGL/GL (kompact's renderer, Mesa's drivers), PAM (the lock page).
    for l in libEGL.so.1 libGLESv2.so.2 libEGL_mesa.so.0 libpam.so.0; do
        pkg_of "$(ldconfig -p | awk -v l="$l" '$1 == l {print $NF; exit}')"
    done
    pkg_of "$(ls /usr/lib/x86_64-linux-gnu/dri/*_dri.so /usr/lib/x86_64-linux-gnu/dri/libdril_dri.so 2>/dev/null | head -1)"
    # Programs kompact and the shell run.
    for c in foot dbus-update-activation-environment gsettings xdg-open fc-match wpctl pw-dump; do
        pkg_of "$(command -v "$c")"
    done
    echo xdg-desktop-portal
    echo xdg-desktop-portal-gtk
    echo pipewire
} | grep -v '^$' | awk '!seen[$0]++' >"$stage/packages-required.txt"
cat >"$stage/packages-optional.txt" <<'EOF'
# installed when the system has them; kompact works without, with less
# the lock page (else swaylock locks)
libgtk-session-lock0
swaylock
# screenshots to a file through the portal, screen sharing (slurp picks the monitor)
xdg-desktop-portal-wlr
slurp
# Settings → Network, Bluetooth; the Files app
network-manager
bluez
nautilus
EOF
if [ "$(grep -vc '^#' "$stage/packages-required.txt")" -lt 10 ]; then
    echo "the package list looks wrong:" >&2
    cat "$stage/packages-required.txt" >&2
    exit 1
fi
. /etc/os-release
echo "built on $PRETTY_NAME, $(date -u +%Y-%m-%d)" >"$stage/BUILT-ON"

echo "==> packing"
(cd "$dist" && tar -czf "$name.tar.gz" "$name" && sha256sum "$name.tar.gz" >"$name.tar.gz.sha256")
echo
echo "release: $dist/$name.tar.gz ($(du -h "$dist/$name.tar.gz" | cut -f1)), needs glibc $(cat "$stage/GLIBC")"
cat "$dist/$name.tar.gz.sha256"
