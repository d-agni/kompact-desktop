#!/bin/sh
# 14d-1: install kompact's login screen (greetd + kompact-greeter) and a "kompact" session,
# WITHOUT starting it at boot: the machine still boots to the text console. To try it, from a
# text console: `sudo systemctl start greetd` (it takes VT 7). Back: Ctrl+Alt+F2, then
# `sudo systemctl stop greetd`. 14d-2: `enable` shows it at every boot, `disable` goes back.
#
#   sudo kompact/tools/install-login.sh install   # greetd, the programs, the session, the config
#   sudo kompact/tools/install-login.sh update    # copy freshly built programs again
#   sudo kompact/tools/install-login.sh enable    # 14d-2: the login screen at boot
#   sudo kompact/tools/install-login.sh disable   # 14d-2: boot as before install (next boot)
#   sudo kompact/tools/install-login.sh remove    # undo all of it (greetd's package stays)
#        kompact/tools/install-login.sh status
#
# enable: display-manager.service → greetd (`systemctl enable --force greetd`), default target
# graphical.target. Text consoles stay on Ctrl+Alt+F1–F6. If the login screen ever fails at
# boot: Ctrl+Alt+F2, log in, `sudo kompact/tools/install-login.sh disable`, reboot; or at the
# GRUB menu press e and add `systemd.unit=multi-user.target` to the `linux` line (one boot).
# disable doesn't stop greetd (that would end a session it started); it applies at next boot.
#
# What install does:
#   - apt install greetd (its service is kept from starting while it installs: policy-rc.d)
#   - copies kompact, kompact-shell, kompact-lock, kompact-greeter to /usr/local/bin (the login
#     screen's user can't read your home), and the two start scripts from tools/login/
#   - /usr/local/share/wayland-sessions/kompact.desktop (the "kompact" session)
#   - /etc/greetd/config.toml → our login screen (the package's file kept as
#     config.toml.before-kompact)
#   - /var/lib/kompact/login (sticky, like /tmp): where each user's session publishes their
#     login look; the login screen only believes files their owner wrote
# It doesn't touch display-manager.service (GDM), the default boot target, or your files.
set -eu

here=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$here/../.." && pwd)
bin=/usr/local/bin
sessions=/usr/local/share/wayland-sessions
shared=/var/lib/kompact/login
state=/var/lib/kompact/install-state
conf=/etc/greetd/config.toml
backup=/etc/greetd/config.toml.before-kompact

programs="$root/kompact/target/release/kompact $root/shell/src-tauri/target/release/kompact-shell $root/shell/src-tauri/target/release/kompact-lock $root/shell/src-tauri/target/release/kompact-greeter"

need_root() {
    if [ "$(id -u)" != 0 ]; then
        echo "run it with sudo: sudo $0 $1" >&2
        exit 1
    fi
}

check_built() {
    for p in $programs; do
        if [ ! -x "$p" ]; then
            echo "not built: $p (cargo build --release in kompact/ and shell/src-tauri/)" >&2
            exit 1
        fi
    done
}

copy_programs() {
    for p in $programs; do
        install -m 755 "$p" "$bin/$(basename "$p")"
    done
    install -m 755 "$here/login/kompact-session" "$bin/kompact-session"
    install -m 755 "$here/login/kompact-greeter-session" "$bin/kompact-greeter-session"
    echo "copied kompact, kompact-shell, kompact-lock, kompact-greeter and the start scripts to $bin"
}

do_install() {
    need_root install
    check_built
    if ! dpkg -s greetd >/dev/null 2>&1; then
        # Keep the package from starting its service while it installs.
        made_policy=
        if [ ! -e /usr/sbin/policy-rc.d ]; then
            printf '#!/bin/sh\nexit 101\n' >/usr/sbin/policy-rc.d
            chmod 755 /usr/sbin/policy-rc.d
            made_policy=1
        fi
        ok=1
        DEBIAN_FRONTEND=noninteractive apt-get install -y greetd || ok=
        if [ -n "$made_policy" ]; then
            rm -f /usr/sbin/policy-rc.d
        fi
        if [ -z "$ok" ]; then
            echo "apt-get install greetd failed" >&2
            exit 1
        fi
        echo "installed greetd (not started)"
    fi
    if ! dpkg -s libgtk-session-lock0 >/dev/null 2>&1; then
        DEBIAN_FRONTEND=noninteractive apt-get install -y libgtk-session-lock0
    fi
    mkdir -p /var/lib/kompact
    # What boot looks like now, for 14d-2 to put back.
    if [ ! -f "$state" ]; then
        {
            echo "# kompact/tools/install-login.sh: how this machine booted before"
            echo "default_target=$(systemctl get-default)"
            echo "display_manager=$(readlink /etc/systemd/system/display-manager.service 2>/dev/null || true)"
        } >"$state"
    fi
    copy_programs
    mkdir -p "$sessions"
    install -m 644 "$here/login/kompact.desktop" "$sessions/kompact.desktop"
    echo "added the kompact session ($sessions/kompact.desktop)"
    if [ -f "$conf" ] && [ ! -f "$backup" ]; then
        cp -p "$conf" "$backup"
    fi
    install -m 644 "$here/login/greetd-config.toml" "$conf"
    echo "greetd now runs kompact's login screen ($conf; the package's own is $backup)"
    mkdir -p "$shared"
    chmod 1777 "$shared"
    echo "made $shared for each user's login look"
    echo
    echo "Boot is unchanged (text console). To try the login screen, from a text console:"
    echo "  sudo systemctl start greetd        (it opens on VT 7)"
    echo "Back to a console: Ctrl+Alt+F2; stop it: sudo systemctl stop greetd"
}

do_update() {
    need_root update
    check_built
    copy_programs
}

installed() {
    for p in kompact kompact-greeter kompact-greeter-session; do
        [ -x "$bin/$p" ] || return 1
    done
    grep -q kompact-greeter-session "$conf" 2>/dev/null
}

recorded() {
    sed -n "s/^$1=//p" "$state" 2>/dev/null
}

# 15c: at boot the login screen takes VT 1, where the machine boots, instead of the package's
# VT 7: else the text login shows on VT 1 for a few seconds first. (Ctrl+Alt+F2-F6 keep theirs.)
dropin=/etc/systemd/system/greetd.service.d/kompact-vt1.conf
use_vt1() {
    mkdir -p "$(dirname "$dropin")"
    printf '%s\n' "# kompact: the login screen on VT 1 in place of the text login there (removed by disable)" \
        "[Unit]" "After=getty@tty1.service" "Conflicts=getty@tty1.service" >"$dropin"
    sed -i 's/^vt = .*/vt = 1/' "$conf"
}

do_enable() {
    need_root enable
    if ! installed; then
        echo "not installed yet (or greetd's config isn't ours): sudo $0 install first" >&2
        exit 1
    fi
    if [ ! -f "$state" ]; then
        echo "no record of how this machine booted before ($state); run install again first" >&2
        exit 1
    fi
    use_vt1
    systemctl daemon-reload
    systemctl enable --force greetd
    systemctl set-default graphical.target
    echo "the login screen now starts at boot (display-manager -> greetd, graphical.target)"
    echo "back: sudo $0 disable (then reboot)"
}

do_disable() {
    need_root disable
    systemctl disable greetd 2>/dev/null || true
    dm=$(recorded display_manager)
    if [ -n "$dm" ] && [ -e "$dm" ]; then
        ln -sfn "$dm" /etc/systemd/system/display-manager.service
        echo "display-manager -> $dm again"
    else
        rm -f /etc/systemd/system/display-manager.service
        echo "no display manager (none was set before)"
    fi
    target=$(recorded default_target)
    systemctl set-default "${target:-multi-user.target}"
    # 15c: back to VT 7, for trying it by hand (sudo systemctl start greetd) from a console.
    rm -f "$dropin"
    rmdir "$(dirname "$dropin")" 2>/dev/null || true
    if [ -f "$conf" ]; then
        sed -i 's/^vt = .*/vt = 7/' "$conf"
    fi
    systemctl daemon-reload
    echo "next boot is as before install (${target:-multi-user.target}); greetd keeps running until then"
}

do_remove() {
    need_root remove
    if systemctl is-enabled --quiet greetd 2>/dev/null; then
        do_disable
    fi
    if systemctl is-active --quiet greetd; then
        systemctl stop greetd
        echo "stopped greetd"
    fi
    if [ -f "$backup" ]; then
        mv -f "$backup" "$conf"
        echo "put back greetd's own config"
    fi
    for p in kompact kompact-shell kompact-lock kompact-greeter kompact-session kompact-greeter-session; do
        rm -f "$bin/$p"
    done
    rm -f "$sessions/kompact.desktop"
    rm -rf "$shared"
    echo "removed the programs, the session and $shared (greetd's package stays: sudo apt remove greetd)"
    if [ -f "$state" ]; then
        echo "boot settings recorded before install: $(grep -v '^#' "$state" | tr '\n' ' ')"
        rm -f "$state"
    fi
}

do_status() {
    echo "greetd package: $(dpkg -s greetd 2>/dev/null | sed -n 's/^Status: //p' || echo 'not installed')"
    echo "greetd service: $(systemctl is-active greetd 2>/dev/null || true), $(systemctl is-enabled greetd 2>/dev/null || true)"
    echo "boot target: $(systemctl get-default)"
    echo "display-manager.service -> $(readlink /etc/systemd/system/display-manager.service 2>/dev/null || echo none)"
    for p in kompact kompact-shell kompact-lock kompact-greeter kompact-session kompact-greeter-session; do
        if [ -e "$bin/$p" ]; then echo "  $bin/$p"; else echo "  $bin/$p (missing)"; fi
    done
    [ -f "$sessions/kompact.desktop" ] && echo "session: $sessions/kompact.desktop"
    grep -q kompact-greeter-session "$conf" 2>/dev/null && echo "greetd config: kompact's login screen"
    [ -d "$shared" ] && echo "shared looks: $(ls "$shared" 2>/dev/null | tr '\n' ' ')"
    return 0
}

case "${1:-}" in
    install) do_install ;;
    update) do_update ;;
    enable) do_enable ;;
    disable) do_disable ;;
    remove) do_remove ;;
    status) do_status ;;
    *)
        echo "usage: sudo $0 install|update|enable|disable|remove   or   $0 status" >&2
        exit 2
        ;;
esac
