#!/bin/sh
# kompact's own login screen (greetd + kompact-greeter) in place of the current one (GDM, …).
# Installed as /usr/local/sbin/kompact-login-screen by install.sh.
#   sudo kompact-login-screen enable    # from the next boot (installs greetd if needed)
#   sudo kompact-login-screen disable   # back to how the machine booted before (next boot)
#        kompact-login-screen status
# If the login screen ever fails at boot: Ctrl+Alt+F3, log in, `sudo kompact-login-screen
# disable`, reboot. Or, for one boot: at the GRUB menu press e, add `systemd.unit=multi-user.target`
# at the end of the `linux` line, Ctrl+X.
set -eu

state=/var/lib/kompact/boot-before
conf=/etc/greetd/config.toml
backup=/etc/greetd/config.toml.before-kompact
dm=/etc/systemd/system/display-manager.service
# 15c: at boot the login screen takes VT 1, where the machine boots, instead of the package's
# VT 7: else the text login shows on VT 1 for a few seconds first. (Ctrl+Alt+F2-F6 keep theirs.)
dropin=/etc/systemd/system/greetd.service.d/kompact-vt1.conf
use_vt1() {
    mkdir -p "$(dirname "$dropin")"
    printf '%s\n' "# kompact: the login screen on VT 1 in place of the text login there (removed by disable)" \
        "[Unit]" "After=getty@tty1.service" "Conflicts=getty@tty1.service" >"$dropin"
    sed -i 's/^vt = .*/vt = 1/' "$conf"
}

need_root() {
    [ "$(id -u)" = 0 ] || { echo "run it with sudo: sudo $0 $1" >&2; exit 1; }
}

enable() {
    need_root enable
    for p in kompact kompact-greeter kompact-greeter-session; do
        [ -x "/usr/local/bin/$p" ] || { echo "kompact isn't installed (no /usr/local/bin/$p): run install.sh first" >&2; exit 1; }
    done
    if ! dpkg -s greetd >/dev/null 2>&1; then
        # Keep the package from starting its service (on this VT) while it installs.
        made=
        if [ ! -e /usr/sbin/policy-rc.d ]; then
            printf '#!/bin/sh\nexit 101\n' >/usr/sbin/policy-rc.d
            chmod 755 /usr/sbin/policy-rc.d
            made=1
        fi
        ok=1
        DEBIAN_FRONTEND=noninteractive apt-get install -y greetd || ok=
        if [ -n "$made" ]; then rm -f /usr/sbin/policy-rc.d; fi
        [ -n "$ok" ] || { echo "greetd didn't install" >&2; exit 1; }
    fi
    mkdir -p /var/lib/kompact
    if [ ! -f "$state" ]; then
        {
            echo "# how this machine booted before kompact's login screen"
            echo "default_target=$(systemctl get-default)"
            echo "display_manager=$(readlink "$dm" 2>/dev/null || true)"
        } >"$state"
    fi
    if [ -f "$conf" ] && [ ! -f "$backup" ]; then
        cp -p "$conf" "$backup"
    fi
    cat >"$conf" <<'EOF'
# kompact's login screen (written by kompact-login-screen; greetd's own file is kept as
# config.toml.before-kompact and comes back with `kompact-login-screen disable`).
[terminal]
vt = 7

[default_session]
command = "/usr/local/bin/kompact-greeter-session"
user = "_greetd"
EOF
    use_vt1
    systemctl daemon-reload
    systemctl enable --force greetd
    systemctl set-default graphical.target
    echo "kompact's login screen starts at the next boot (back: sudo $0 disable)"
}

disable() {
    need_root disable
    systemctl disable greetd 2>/dev/null || true
    before_dm=$(sed -n 's/^display_manager=//p' "$state" 2>/dev/null)
    before_target=$(sed -n 's/^default_target=//p' "$state" 2>/dev/null)
    if [ -n "$before_dm" ] && [ -e "$before_dm" ]; then
        ln -sfn "$before_dm" "$dm"
        echo "display manager: $(basename "$before_dm") again"
    elif [ -f "$state" ]; then
        rm -f "$dm"
    fi
    systemctl set-default "${before_target:-graphical.target}"
    if [ -f "$backup" ]; then
        mv -f "$backup" "$conf"
    fi
    rm -f "$dropin"
    rmdir "$(dirname "$dropin")" 2>/dev/null || true
    rm -f "$state"
    systemctl daemon-reload
    # greetd isn't stopped: that would end a session it started. It applies at the next boot.
    echo "the next boot is as before (${before_target:-graphical.target})"
}

status() {
    echo "greetd: $(systemctl is-enabled greetd 2>/dev/null || echo not installed)"
    echo "boot target: $(systemctl get-default)"
    echo "display manager: $(readlink "$dm" 2>/dev/null || echo none)"
    if grep -q kompact-greeter-session "$conf" 2>/dev/null; then
        echo "greetd runs kompact's login screen"
    fi
}

case "${1:-}" in
    enable) enable ;;
    disable) disable ;;
    status) status ;;
    *) echo "usage: sudo $0 enable|disable   or   $0 status" >&2; exit 2 ;;
esac
