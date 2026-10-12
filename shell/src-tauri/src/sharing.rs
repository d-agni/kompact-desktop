//! 10d-5: System Settings → Screen Sharing. Screen sharing goes through xdg-desktop-portal-wlr
//! (9e-b), which reads `~/.config/xdg-desktop-portal-wlr/kompact` when it starts; the pane changes
//! its `max_fps` (keeping the rest of the file) and stops the wlr portal so the next share starts
//! it with the new rate. Never during a share: then the stop waits until sharing ends (the
//! `sharing` message from kompact, `on_sharing_changed`).
//!
//! Not set up yet (no portal files)? "Set Up" writes the same two files as
//! `kompact/tools/install-portals.sh` and restarts xdg-desktop-portal, which reads its file at
//! start. Both files are only read in kompact sessions (`XDG_CURRENT_DESKTOP=kompact`).

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};

use serde_json::{json, Value};
use tauri::State;
use tracing::{info, warn};

use crate::compositor::Compositor;

pub const FPS_CHOICES: [u32; 5] = [5, 10, 15, 20, 30];
pub const DEFAULT_FPS: u32 = 10;
const WLR_BINARY: &str = "/usr/libexec/xdg-desktop-portal-wlr";
const WLR_SERVICE: &str = "xdg-desktop-portal-wlr.service";

/// A stop of the wlr portal waiting for the share to end.
static RESTART_PENDING: AtomicBool = AtomicBool::new(false);

/// `max_fps` in the `[screencast]` section, if there is one.
pub fn max_fps(text: &str) -> Option<u32> {
    let mut section = "";
    for line in text.lines().map(str::trim) {
        if line.starts_with('[') {
            section = line;
        } else if section == "[screencast]" {
            if let Some((k, v)) = line.split_once('=') {
                if k.trim() == "max_fps" {
                    return v.trim().parse().ok();
                }
            }
        }
    }
    None
}

/// `text` with `max_fps=fps` in its `[screencast]` section: the line replaced, else added at the
/// top of the section, else a section added. Everything else stays as it was.
pub fn with_max_fps(text: &str, fps: u32) -> String {
    let mut out = Vec::new();
    let mut section = String::new();
    let mut done = false;
    for line in text.lines() {
        let t = line.trim();
        if t.starts_with('[') {
            section = t.to_string();
            out.push(line.to_string());
            if t == "[screencast]" && !done && max_fps(text).is_none() {
                out.push(format!("max_fps={fps}"));
                done = true;
            }
            continue;
        }
        if section == "[screencast]" && !done {
            if let Some((k, _)) = t.split_once('=') {
                if k.trim() == "max_fps" {
                    out.push(format!("max_fps={fps}"));
                    done = true;
                    continue;
                }
            }
        }
        out.push(line.to_string());
    }
    if !done {
        out.push("[screencast]".into());
        out.push(format!("max_fps={fps}"));
    }
    let mut s = out.join("\n");
    s.push('\n');
    s
}

/// The two files `install-portals.sh` writes, for "Set Up".
pub fn portals_conf() -> &'static str {
    "# kompact (9d, 9e-b, 10d-5), written by System Settings → Screen Sharing; only for XDG_CURRENT_DESKTOP=kompact.\n\
     [preferred]\n\
     default=gtk\n\
     org.freedesktop.impl.portal.Screenshot=wlr\n\
     org.freedesktop.impl.portal.ScreenCast=wlr\n"
}

pub fn wlr_conf(fps: u32) -> String {
    format!(
        "# kompact (9e-b, 10d-5), written by System Settings → Screen Sharing; only for XDG_CURRENT_DESKTOP=kompact.\n\
         # Every shared frame costs kompact ~17 ms of its main loop on the GT 710 (9e-a2), so cap the rate.\n\
         # Sharing asks which monitor: click it (slurp).\n\
         [screencast]\n\
         max_fps={fps}\n\
         chooser_type=simple\n\
         chooser_cmd=slurp -f %o -or\n"
    )
}

fn config_home() -> Option<PathBuf> {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
}

fn paths() -> Option<(PathBuf, PathBuf)> {
    let c = config_home()?;
    Some((c.join("xdg-desktop-portal/kompact-portals.conf"), c.join("xdg-desktop-portal-wlr/kompact")))
}

fn on_path(program: &str) -> bool {
    std::env::var_os("PATH").is_some_and(|p| std::env::split_paths(&p).any(|d| d.join(program).is_file()))
}

fn sharing_now(c: &Compositor) -> bool {
    c.sharing_message().is_some_and(|m| !crate::compositor::shared_outputs(&m).is_empty())
}

fn status(c: &Compositor) -> Value {
    let (portals, wlr) = paths().unzip();
    let portals_text = portals.as_ref().and_then(|p| std::fs::read_to_string(p).ok());
    let wlr_text = wlr.as_ref().and_then(|p| std::fs::read_to_string(p).ok());
    let routed = portals_text.as_deref().is_some_and(|t| t.lines().any(|l| l.trim().replace(' ', "") == "org.freedesktop.impl.portal.ScreenCast=wlr"));
    let sharing = c.sharing_message().map(|m| crate::compositor::shared_outputs(&m)).unwrap_or_default();
    json!({
        "installed": std::path::Path::new(WLR_BINARY).exists(),
        "slurp": on_path("slurp"),
        "configured": routed && wlr_text.is_some(),
        "fps": wlr_text.as_deref().and_then(max_fps),
        "choices": FPS_CHOICES,
        "sharing": sharing,
        "restart_pending": RESTART_PENDING.load(Ordering::Relaxed),
    })
}

fn systemctl(args: &[&str]) {
    match std::process::Command::new("systemctl").arg("--user").args(args).status() {
        Ok(s) if s.success() => info!("screen sharing: systemctl --user {}", args.join(" ")),
        Ok(s) => warn!("screen sharing: systemctl --user {} exited with {s}", args.join(" ")),
        Err(e) => warn!("screen sharing: systemctl: {e}"),
    }
}

/// Stop the wlr portal now, or (sharing) once the share ends. D-Bus starts it again, with the
/// new settings, for the next share.
fn restart_wlr(c: &Compositor) {
    if sharing_now(c) {
        RESTART_PENDING.store(true, Ordering::Relaxed);
        info!("screen sharing: new frame rate applies after this share (the wlr portal restarts when it ends)");
    } else {
        RESTART_PENDING.store(false, Ordering::Relaxed);
        // Off the main thread: commands run on it, and systemctl can take a moment.
        std::thread::spawn(|| systemctl(&["stop", WLR_SERVICE]));
    }
}

/// From `compositor.rs` on every `sharing` message: a waiting stop runs once nothing is shared.
pub fn on_sharing_changed(sharing: bool) {
    if !sharing && RESTART_PENDING.swap(false, Ordering::Relaxed) {
        info!("screen sharing: the share ended, restarting the wlr portal for the new frame rate");
        std::thread::spawn(|| systemctl(&["stop", WLR_SERVICE]));
    }
}

#[tauri::command]
pub fn sharing_settings(c: State<'_, Compositor>) -> Value {
    status(&c)
}

#[tauri::command]
pub fn set_sharing_fps(c: State<'_, Compositor>, fps: u32) -> Result<Value, String> {
    if !FPS_CHOICES.contains(&fps) {
        return Err(format!("{fps} frames per second isn't one of the choices"));
    }
    let (_, wlr) = paths().ok_or("no home directory")?;
    let old = std::fs::read_to_string(&wlr).map_err(|_| "screen sharing isn't set up yet".to_string())?;
    std::fs::write(&wlr, with_max_fps(&old, fps)).map_err(|e| format!("{}: {e}", wlr.display()))?;
    info!("screen sharing: max_fps={fps} in {}", wlr.display());
    restart_wlr(&c);
    Ok(status(&c))
}

/// "Set Up": the two portal files (as `install-portals.sh`), then xdg-desktop-portal again.
#[tauri::command]
pub fn set_up_sharing(c: State<'_, Compositor>) -> Result<Value, String> {
    if !std::path::Path::new(WLR_BINARY).exists() {
        return Err("xdg-desktop-portal-wlr isn't installed: sudo apt install xdg-desktop-portal-wlr slurp".into());
    }
    let (portals, wlr) = paths().ok_or("no home directory")?;
    for (path, text) in [(&portals, portals_conf().to_string()), (&wlr, wlr_conf(DEFAULT_FPS))] {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        }
        if path.exists() {
            let kept = path.with_extension("before-10d-5");
            if !kept.exists() {
                let _ = std::fs::copy(path, &kept);
            }
        }
        std::fs::write(path, text).map_err(|e| format!("{}: {e}", path.display()))?;
        info!("screen sharing: wrote {}", path.display());
    }
    std::thread::spawn(|| systemctl(&["restart", "xdg-desktop-portal.service"]));
    restart_wlr(&c);
    Ok(status(&c))
}

#[cfg(test)]
mod tests {
    use super::*;

    const XDPW: &str = "# comment\n[screencast]\nmax_fps=10\nchooser_type=simple\nchooser_cmd=slurp -f %o -or\n";

    #[test]
    fn reads_the_rate() {
        assert_eq!(max_fps(XDPW), Some(10));
        assert_eq!(max_fps("[other]\nmax_fps=5\n"), None);
        assert_eq!(max_fps("[screencast]\nmax_fps = 20\n"), Some(20));
        assert_eq!(max_fps(&wlr_conf(15)), Some(15));
    }

    #[test]
    fn changes_only_the_rate() {
        let out = with_max_fps(XDPW, 20);
        assert_eq!(out, XDPW.replace("max_fps=10", "max_fps=20"));
        // No max_fps yet: added at the top of the section.
        assert_eq!(with_max_fps("[screencast]\nchooser_type=simple\n", 5), "[screencast]\nmax_fps=5\nchooser_type=simple\n");
        // No section: one is added.
        assert_eq!(with_max_fps("# x\n", 30), "# x\n[screencast]\nmax_fps=30\n");
        // The same key elsewhere stays.
        assert_eq!(with_max_fps("[a]\nmax_fps=1\n[screencast]\nmax_fps=2\n", 15), "[a]\nmax_fps=1\n[screencast]\nmax_fps=15\n");
    }

    #[test]
    fn set_up_files() {
        assert!(portals_conf().contains("org.freedesktop.impl.portal.ScreenCast=wlr"));
        assert!(wlr_conf(10).contains("chooser_cmd=slurp -f %o -or"));
        assert!(FPS_CHOICES.contains(&DEFAULT_FPS));
    }
}
