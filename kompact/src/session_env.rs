//! Make the user session (systemd --user and D-Bus activation) aware of our Wayland display.
//!
//! Services started on demand by D-Bus, such as `xdg-desktop-portal-gtk`, don't inherit
//! kompact's environment. Without `WAYLAND_DISPLAY` they fail with "cannot open display", and
//! every portal call (snap Firefox makes several at startup) then sits out D-Bus's 25 s timeout.
//! Measured on the GT 710 2026-10-07: Firefox windows took 26 s to appear, matching exactly.
//!
//! Only done for the DRM backend: under winit we're nested in someone else's session, and
//! exporting our display there would break that session's portals.
//!
//! 6m-0: on the DRM backend we also name our desktop, `XDG_CURRENT_DESKTOP=kompact` (unless it's
//! already set), and export it alongside. GNOME's settings have per-desktop values: Ubuntu gives
//! title bars their minimize and maximize buttons only for `ubuntu`, so with no desktop name GTK
//! apps (Firefox, gedit) got just a close button. `tools/install-desktop-settings.sh` adds the
//! `kompact` values.
//!
//! 6m-d5 (slow shell and Firefox starts, GT 710 2026-10-09): when kompact quit, the GTK portal
//! lost its display and died, and the still-running portal frontend asked D-Bus to start it
//! again with no display. D-Bus then held that request "pending" for its 120 s activation
//! timeout, and every portal call (the shell starting, each Firefox window) waited out 25 s
//! behind it; our own `try-restart` of the frontend sat out systemd's 90 s start timeout too.
//! So now: on quit, stop both portals while our display still exists (`stop_portals`); on start,
//! start the GTK portal ourselves with the new display (its taking the bus name also releases
//! any request D-Bus still has pending) and restart the frontend without waiting for it; and
//! every command has a time limit.
//!
//! 9d: with `tools/install-portals.sh`, screenshots through the portal go to
//! `xdg-desktop-portal-wlr`, which D-Bus starts on demand. It loses the display on quit just as
//! the GTK portal did, so it's stopped too (its own command, and only if it's installed:
//! stopping a unit systemd doesn't know fails the whole command).
//!
//! 6m-f: the theme variables `theme.rs` sets (`DCONF_PROFILE`, `GTK_THEME`) are exported and
//! withdrawn the same way.
//!
//! 9e-b2: `XDG_SESSION_TYPE=wayland` too. kompact is started from a console, so its apps
//! inherited `tty`, and Firefox's WebRTC (libwebrtc) shares the screen through PipeWire and the
//! portal only when the session type is `wayland`; with `tty` it offered just its own window.
//! `KOMPACT_SESSION_TYPE=off` leaves the variable alone.

use std::{
    process::{Command, Stdio},
    sync::{
        atomic::{AtomicBool, Ordering},
        Mutex, Once,
    },
    time::{Duration, Instant},
};

static ENABLED: AtomicBool = AtomicBool::new(false);
static EXPORT: Once = Once::new();

/// Portal services that may already be running (or failed) with the old environment.
const PORTAL_UNITS: [&str; 2] = ["xdg-desktop-portal-gtk.service", "xdg-desktop-portal.service"];
const GTK_PORTAL: &str = PORTAL_UNITS[0];
const PORTAL_FRONTEND: &str = PORTAL_UNITS[1];

/// 9d: the wlr portal (screenshots), when installed.
const WLR_PORTAL: &str = "xdg-desktop-portal-wlr.service";
const WLR_PORTAL_BINARY: &str = "/usr/libexec/xdg-desktop-portal-wlr";

/// No session command may hold us up longer than this (the GTK portal starts in ~0.1 s).
const COMMAND_LIMIT: Duration = Duration::from_secs(8);
/// At quit the screen is frozen while we wait, so less.
const QUIT_LIMIT: Duration = Duration::from_secs(3);

pub fn set_enabled(on: bool) {
    ENABLED.store(on, Ordering::Relaxed);
}

/// Our `XDG_CURRENT_DESKTOP`.
pub const DESKTOP: &str = "kompact";

/// What to set `XDG_CURRENT_DESKTOP` to, given its current value: ours if it's unset or empty,
/// otherwise leave it (`None`).
pub fn desktop_to_set(current: Option<&str>) -> Option<&'static str> {
    current.is_none_or(|c| c.trim().is_empty()).then_some(DESKTOP)
}

/// 9e-b2: what to set `XDG_SESSION_TYPE` to, given its current value and the
/// `KOMPACT_SESSION_TYPE` switch: `wayland`, unless it already is or the switch is `off`.
pub fn session_type_to_set(current: Option<&str>, switch: Option<&str>) -> Option<&'static str> {
    let off = switch.is_some_and(|v| v.trim().eq_ignore_ascii_case("off"));
    (!off && current.map(str::trim) != Some("wayland")).then_some("wayland")
}

/// Set `XDG_SESSION_TYPE=wayland` for our apps and the session (DRM backend only; from `main`,
/// before any thread or client starts). Exported and withdrawn with the display.
pub fn set_session_type() {
    if !ENABLED.load(Ordering::Relaxed) {
        return;
    }
    let current = std::env::var("XDG_SESSION_TYPE").ok();
    let switch = std::env::var("KOMPACT_SESSION_TYPE").ok();
    match session_type_to_set(current.as_deref(), switch.as_deref()) {
        Some(t) => {
            std::env::set_var("XDG_SESSION_TYPE", t);
            also_export(vec![("XDG_SESSION_TYPE", t.to_string())]);
            tracing::info!(
                "XDG_SESSION_TYPE={t} (was {:?}; apps such as Firefox share the screen through the portal; 9e-b2, KOMPACT_SESSION_TYPE=off to keep it)",
                current.unwrap_or_default()
            );
        }
        None => tracing::info!("XDG_SESSION_TYPE {:?} left as it is", current.unwrap_or_default()),
    }
}

/// Further variables kompact set for its session (6m-f: the theme), exported with the display.
static ALSO: Mutex<Vec<(&'static str, String)>> = Mutex::new(Vec::new());

/// Export these too (called from `main`, before the first client starts).
pub fn also_export(vars: Vec<(&'static str, String)>) {
    ALSO.lock().unwrap().extend(vars);
}

fn also_names() -> Vec<&'static str> {
    ALSO.lock().unwrap().iter().map(|(n, _)| *n).collect()
}

/// The commands that publish `display` (and the desktop name, if any, and `also`) to the session and
/// (re)start the portals so they pick them up: the GTK portal is started outright (which also
/// releases a start D-Bus may still have pending from before), the frontend only if it's running
/// (`try-restart`) and without waiting for it (`--no-block`).
/// 13e: `unset`: variables to withdraw from the systemd user session first (a `GTK_THEME` an
/// older kompact exported and a crash left behind).
pub fn export_commands(display: &str, desktop: Option<&str>, also: &[(&str, String)], unset: &[&str]) -> Vec<Vec<String>> {
    let mut first = Vec::new();
    if !unset.is_empty() {
        let mut cmd = ["systemctl", "--user", "unset-environment"].map(String::from).to_vec();
        cmd.extend(unset.iter().map(|n| n.to_string()));
        first.push(cmd);
    }
    let mut update = vec![
        "dbus-update-activation-environment".to_string(),
        "--systemd".to_string(),
        format!("WAYLAND_DISPLAY={display}"),
    ];
    if let Some(d) = desktop.filter(|d| !d.is_empty()) {
        update.push(format!("XDG_CURRENT_DESKTOP={d}"));
    }
    update.extend(also.iter().map(|(n, v)| format!("{n}={v}")));
    let systemctl = |args: &[&str]| -> Vec<String> {
        ["systemctl", "--user"].iter().chain(args).map(|a| a.to_string()).collect()
    };
    first.extend([
        update,
        systemctl(&["reset-failed", PORTAL_UNITS[0], PORTAL_UNITS[1]]),
        systemctl(&["restart", GTK_PORTAL]),
        systemctl(&["--no-block", "try-restart", PORTAL_FRONTEND]),
    ]);
    first
}

/// Stops both portals (frontend and GTK backend) before our display goes away, so nothing asks
/// D-Bus to start them again without one.
pub fn stop_command() -> Vec<String> {
    let mut cmd = ["systemctl", "--user", "stop"].map(String::from).to_vec();
    cmd.extend([PORTAL_FRONTEND, GTK_PORTAL].map(String::from));
    cmd
}

/// 9d: the commands run on quit: `stop_command`, then the wlr portal on its own if installed.
pub fn stop_commands(wlr_installed: bool) -> Vec<Vec<String>> {
    let mut cmds = vec![stop_command()];
    if wlr_installed {
        cmds.push(["systemctl", "--user", "stop", WLR_PORTAL].map(String::from).to_vec());
    }
    cmds
}

/// The command that withdraws it again on exit, so later sessions don't inherit a dead socket
/// (or our desktop name, if we set it, or `also`).
pub fn clear_command(desktop_set_by_us: bool, also: &[&str]) -> Vec<String> {
    let mut cmd = ["systemctl", "--user", "unset-environment", "WAYLAND_DISPLAY"].map(String::from).to_vec();
    if desktop_set_by_us {
        cmd.push("XDG_CURRENT_DESKTOP".into());
    }
    cmd.extend(also.iter().map(|n| n.to_string()));
    cmd
}

/// Whether `set_desktop_name` named the desktop.
static DESKTOP_SET: AtomicBool = AtomicBool::new(false);

/// Set `XDG_CURRENT_DESKTOP=kompact` if it isn't set (DRM backend only; called from `main`
/// before any thread or client starts, so changing the environment is safe).
pub fn set_desktop_name() {
    if !ENABLED.load(Ordering::Relaxed) {
        return;
    }
    let current = std::env::var("XDG_CURRENT_DESKTOP").ok();
    match desktop_to_set(current.as_deref()) {
        Some(d) => {
            std::env::set_var("XDG_CURRENT_DESKTOP", d);
            DESKTOP_SET.store(true, Ordering::Relaxed);
            tracing::info!("XDG_CURRENT_DESKTOP={d} (title-bar buttons: tools/install-desktop-settings.sh)");
        }
        None => tracing::info!("XDG_CURRENT_DESKTOP already {:?}, keeping it", current.unwrap_or_default()),
    }
}

/// Publish `$WAYLAND_DISPLAY` once, in the background (never blocks the compositor). Called
/// right before the first client is spawned, by which time the backend has set the variable.
pub fn export_once() {
    if !ENABLED.load(Ordering::Relaxed) {
        return;
    }
    EXPORT.call_once(|| {
        let Ok(socket) = std::env::var("WAYLAND_DISPLAY") else { return };
        let desktop = std::env::var("XDG_CURRENT_DESKTOP").ok();
        let also = ALSO.lock().unwrap().clone();
        std::thread::spawn(move || {
            let started = Instant::now();
            // 13e: kompact no longer sets GTK_THEME; one left in the session (an older kompact
            // that crashed) would still break libadwaita apps started over D-Bus. Yours is kept.
            let unset: &[&str] = if std::env::var_os("GTK_THEME").is_none() { &["GTK_THEME"] } else { &[] };
            for cmd in export_commands(&socket, desktop.as_deref(), &also, unset) {
                run(&cmd, COMMAND_LIMIT);
            }
            tracing::info!(
                "exported WAYLAND_DISPLAY={socket}, XDG_CURRENT_DESKTOP={}{} to the user session, portals restarted ({} ms)",
                desktop.as_deref().unwrap_or("(unset)"),
                also.iter().map(|(n, _)| format!(", {n}")).collect::<String>(),
                started.elapsed().as_millis()
            );
        });
    });
}

static STOPPED: AtomicBool = AtomicBool::new(false);

/// On quit, while the display is still up: stop the portals (once).
pub fn stop_portals() {
    if !ENABLED.load(Ordering::Relaxed) || !EXPORT.is_completed() || STOPPED.swap(true, Ordering::Relaxed) {
        return;
    }
    let started = Instant::now();
    let wlr = std::path::Path::new(WLR_PORTAL_BINARY).exists();
    for cmd in stop_commands(wlr) {
        run(&cmd, QUIT_LIMIT);
    }
    tracing::info!(
        "stopped the portals{} before closing the display ({} ms)",
        if wlr { " (and the wlr portal)" } else { "" },
        started.elapsed().as_millis()
    );
}

/// Undo `export_once` on shutdown (synchronous; we're exiting anyway).
pub fn clear() {
    if ENABLED.load(Ordering::Relaxed) && EXPORT.is_completed() {
        // Not stopped yet if we're leaving some other way than the quit key.
        stop_portals();
        run(&clear_command(DESKTOP_SET.load(Ordering::Relaxed), &also_names()), QUIT_LIMIT);
    }
}

/// Run `cmd`, giving up (and killing it) after `limit`.
fn run(cmd: &[String], limit: Duration) {
    let Some((prog, args)) = cmd.split_first() else { return };
    let mut child = match Command::new(prog)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    {
        Ok(c) => c,
        Err(e) => return tracing::warn!("could not run {prog}: {e}"),
    };
    let deadline = Instant::now() + limit;
    loop {
        match child.try_wait() {
            Ok(Some(s)) if s.success() => return,
            Ok(Some(s)) => return tracing::warn!("{} exited with {s}", cmd.join(" ")),
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(10)),
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                return tracing::warn!("{} took over {} s; gave up on it", cmd.join(" "), limit.as_secs());
            }
            Err(e) => return tracing::warn!("{}: {e}", cmd.join(" ")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exports_display_to_systemd_and_dbus() {
        let cmds = export_commands("wayland-1", None, &[], &[]);
        assert_eq!(
            cmds[0],
            ["dbus-update-activation-environment", "--systemd", "WAYLAND_DISPLAY=wayland-1"]
        );
        let cmds = export_commands("wayland-1", Some("kompact"), &[], &[]);
        assert_eq!(
            cmds[0],
            [
                "dbus-update-activation-environment",
                "--systemd",
                "WAYLAND_DISPLAY=wayland-1",
                "XDG_CURRENT_DESKTOP=kompact"
            ]
        );
    }

    #[test]
    fn desktop_name_only_when_unset() {
        assert_eq!(desktop_to_set(None), Some("kompact"));
        assert_eq!(desktop_to_set(Some("")), Some("kompact"));
        assert_eq!(desktop_to_set(Some("ubuntu:GNOME")), None);
    }

    #[test]
    fn restarts_only_the_portals() {
        let cmds = export_commands("wayland-1", Some("kompact"), &[], &[]);
        for c in &cmds[1..] {
            assert_eq!(&c[..2], ["systemctl", "--user"]);
            let units: Vec<_> = c[2..].iter().filter(|a| a.ends_with(".service")).collect();
            assert!(!units.is_empty() && units.iter().all(|u| u.starts_with("xdg-desktop-portal")), "{c:?}");
        }
        // The GTK portal is started outright, after the environment is updated; the frontend
        // is restarted only if running, and not waited for.
        assert_eq!(cmds[2], ["systemctl", "--user", "restart", "xdg-desktop-portal-gtk.service"]);
        assert_eq!(cmds[3], ["systemctl", "--user", "--no-block", "try-restart", "xdg-desktop-portal.service"]);
    }

    #[test]
    fn stops_the_portals_on_quit() {
        assert_eq!(
            stop_command(),
            ["systemctl", "--user", "stop", "xdg-desktop-portal.service", "xdg-desktop-portal-gtk.service"]
        );
    }

    #[test]
    fn stops_the_wlr_portal_on_its_own() {
        assert_eq!(stop_commands(false), vec![stop_command()]);
        let cmds = stop_commands(true);
        assert_eq!(cmds[0], stop_command()); // unchanged (6m-d5)
        assert_eq!(cmds[1], ["systemctl", "--user", "stop", "xdg-desktop-portal-wlr.service"]);
    }

    #[test]
    fn clears_on_exit() {
        assert_eq!(clear_command(false, &[]), ["systemctl", "--user", "unset-environment", "WAYLAND_DISPLAY"]);
        assert_eq!(
            clear_command(true, &[]),
            ["systemctl", "--user", "unset-environment", "WAYLAND_DISPLAY", "XDG_CURRENT_DESKTOP"]
        );
    }

    #[test]
    fn theme_variables_go_and_come_back() {
        let also = [("DCONF_PROFILE", "/h/.config/kompact/dconf/profile".to_string()), ("GTK_THEME", "WhiteSur-Light".to_string())];
        let cmds = export_commands("wayland-1", Some("kompact"), &also, &[]);
        assert_eq!(
            &cmds[0][3..],
            ["XDG_CURRENT_DESKTOP=kompact", "DCONF_PROFILE=/h/.config/kompact/dconf/profile", "GTK_THEME=WhiteSur-Light"]
        );
        assert_eq!(
            clear_command(true, &["DCONF_PROFILE", "GTK_THEME"]),
            ["systemctl", "--user", "unset-environment", "WAYLAND_DISPLAY", "XDG_CURRENT_DESKTOP", "DCONF_PROFILE", "GTK_THEME"]
        );
        // 13e: a leftover GTK_THEME goes first, before anything is exported.
        let cmds = export_commands("wayland-1", Some("kompact"), &[], &["GTK_THEME"]);
        assert_eq!(cmds[0], ["systemctl", "--user", "unset-environment", "GTK_THEME"]);
        assert_eq!(cmds[1][0], "dbus-update-activation-environment");
    }

    #[test]
    fn session_type_becomes_wayland() {
        assert_eq!(session_type_to_set(Some("tty"), None), Some("wayland"));
        assert_eq!(session_type_to_set(None, None), Some("wayland"));
        assert_eq!(session_type_to_set(Some("x11"), None), Some("wayland"));
        assert_eq!(session_type_to_set(Some("wayland"), None), None);
        assert_eq!(session_type_to_set(Some("tty"), Some("off")), None);
        assert_eq!(session_type_to_set(Some("tty"), Some("OFF ")), None);
        assert_eq!(session_type_to_set(Some("tty"), Some("on")), Some("wayland"));
        // Exported with the display and withdrawn on exit, like the theme variables.
        let also = [("XDG_SESSION_TYPE", "wayland".to_string())];
        assert_eq!(cmds_first(&also).last().unwrap(), "XDG_SESSION_TYPE=wayland");
        assert_eq!(clear_command(true, &["XDG_SESSION_TYPE"]).last().unwrap(), "XDG_SESSION_TYPE");
    }

    fn cmds_first(also: &[(&str, String)]) -> Vec<String> {
        export_commands("wayland-1", Some("kompact"), also, &[]).remove(0)
    }

    #[test]
    fn disabled_by_default() {
        // Nothing runs unless main enabled it for the DRM backend.
        assert!(!ENABLED.load(Ordering::Relaxed));
    }
}
