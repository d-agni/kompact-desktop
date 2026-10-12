//! 8d: the lock screen (ext-session-lock-v1). A lock client (swaylock by default) asks kompact
//! to lock; from then on nothing but its lock surfaces and the cursor is drawn (`render.rs`),
//! every key but VT switching goes to it and nothing else can take the keyboard (`input.rs`,
//! `shell.rs`). The lock is confirmed to the client once a cleared frame has had time to reach
//! the screen (`CONFIRM_AFTER`, from the 50 ms tick); until the client unlocks, the session
//! stays locked, even if it crashes (the screen just stays black). Then `Super+L`, or any lock
//! client, may take over (`accept_new_lock`).
//!
//! `Super+L` (or the shell's menu) runs `KOMPACT_LOCK_CMD`. 14b: by default that's our own lock
//! page, `kompact-lock` (on `$PATH`, or the shell's release build in this checkout; it falls
//! back to swaylock itself if it can't lock), else `swaylock -f` (`sudo apt install swaylock`).
//! `KOMPACT_LOCK_PAGE=off` keeps swaylock. `KOMPACT_LOCK=off` hides the protocol and the shortcut.

use std::time::{Duration, Instant};

use smithay::output::Output;
use smithay::reexports::wayland_protocols::ext::session_lock::v1::server::ext_session_lock_v1::ExtSessionLockV1;
use smithay::reexports::wayland_server::Resource;
use smithay::wayland::session_lock::{LockSurface, SessionLocker};

/// After a lock request, how long before telling the client the screen is locked: the next
/// frame (a black one) is drawn within about two refreshes at 60 Hz.
pub const CONFIRM_AFTER: Duration = Duration::from_millis(150);

pub const DEFAULT_CMD: &str = "swaylock -f";

/// 14b: where `kompact-lock` may be, in order: on `$PATH`, then the shell's release build in this
/// checkout (kompact at `<root>/kompact/target/release/kompact`), as for the shell.
pub fn page_candidates(path_var: Option<&str>, exe: Option<&std::path::Path>) -> Vec<std::path::PathBuf> {
    let mut out: Vec<std::path::PathBuf> = path_var.map(|p| std::env::split_paths(p).map(|d| d.join("kompact-lock")).collect()).unwrap_or_default();
    if let Some(root) = exe.and_then(|e| e.ancestors().nth(4)) {
        out.push(root.join("shell/src-tauri/target/release/kompact-lock"));
    }
    out
}

/// 14b: the lock command. `KOMPACT_LOCK_CMD` wins; then `kompact-lock` if one is there (and its
/// path has no space: kompact splits the command at spaces), unless `KOMPACT_LOCK_PAGE=off`;
/// else swaylock.
pub fn choose_cmd(cmd_env: Option<&str>, page_env: Option<&str>, candidates: &[std::path::PathBuf], is_file: impl Fn(&std::path::Path) -> bool) -> String {
    if let Some(c) = cmd_env.map(str::trim).filter(|c| !c.is_empty()) {
        return c.to_string();
    }
    let page_off = page_env.is_some_and(|v| matches!(v.trim().to_ascii_lowercase().as_str(), "off" | "0" | "no" | "false"));
    if !page_off {
        if let Some(p) = candidates.iter().find(|p| is_file(p) && !p.to_string_lossy().contains(char::is_whitespace)) {
            return p.display().to_string();
        }
    }
    DEFAULT_CMD.into()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    Unlocked,
    /// Asked for; drawn black already, confirmed to the client at `since + CONFIRM_AFTER`.
    Locking { since: Instant },
    Locked,
}

/// Whether a new lock request is taken, given the phase and whether the client holding the
/// current lock is still there.
pub fn accept_new_lock(phase: Phase, holder_alive: bool) -> bool {
    match phase {
        Phase::Unlocked => true,
        // A lock client that died leaves the session locked; a new one may take over.
        Phase::Locking { .. } | Phase::Locked => !holder_alive,
    }
}

pub fn confirm_due(phase: Phase, now: Instant) -> bool {
    matches!(phase, Phase::Locking { since } if now.duration_since(since) >= CONFIRM_AFTER)
}

pub struct Lock {
    pub enabled: bool,
    pub cmd: String,
    pub phase: Phase,
    /// The confirmation to send once `confirm_due`.
    pub pending: Option<SessionLocker>,
    /// The lock object of the client holding the lock (to tell if it's still alive).
    pub holder: Option<ExtSessionLockV1>,
    /// One lock surface per output, as the client made them.
    pub surfaces: Vec<(LockSurface, Output)>,
}

impl Lock {
    pub fn from_env() -> Lock {
        let enabled = crate::dmabuf::enabled_from_env(std::env::var("KOMPACT_LOCK").ok().as_deref());
        let exe = std::env::current_exe().ok();
        let candidates = page_candidates(std::env::var("PATH").ok().as_deref(), exe.as_deref());
        let cmd = choose_cmd(
            std::env::var("KOMPACT_LOCK_CMD").ok().as_deref(),
            std::env::var("KOMPACT_LOCK_PAGE").ok().as_deref(),
            &candidates,
            |p| p.is_file(),
        );
        Lock { enabled, cmd, phase: Phase::Unlocked, pending: None, holder: None, surfaces: Vec::new() }
    }

    pub fn is_locked(&self) -> bool {
        self.phase != Phase::Unlocked
    }

    pub fn holder_alive(&self) -> bool {
        self.holder.as_ref().is_some_and(|h| h.is_alive())
    }

    /// The lock surface on `output`, if the client made one.
    pub fn surface_on(&self, output: &Output) -> Option<&LockSurface> {
        self.surfaces.iter().find(|(s, o)| o == output && s.alive()).map(|(s, _)| s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn which_lock_command() {
        use std::path::{Path, PathBuf};
        let exe = Path::new("/home/u/kompact-de/kompact/target/release/kompact");
        let c = page_candidates(Some("/usr/local/bin:/usr/bin"), Some(exe));
        assert_eq!(
            c,
            [
                PathBuf::from("/usr/local/bin/kompact-lock"),
                "/usr/bin/kompact-lock".into(),
                "/home/u/kompact-de/shell/src-tauri/target/release/kompact-lock".into()
            ]
        );
        let built = |p: &Path| p.starts_with("/home");
        assert_eq!(choose_cmd(None, None, &c, built), "/home/u/kompact-de/shell/src-tauri/target/release/kompact-lock");
        assert_eq!(choose_cmd(Some("swaylock -f -c 000000"), None, &c, built), "swaylock -f -c 000000");
        assert_eq!(choose_cmd(Some("  "), Some("off"), &c, built), DEFAULT_CMD);
        assert_eq!(choose_cmd(None, None, &c, |_| false), DEFAULT_CMD, "not built: swaylock");
        let spaced = [PathBuf::from("/home/u/my de/kompact-lock")];
        assert_eq!(choose_cmd(None, None, &spaced, |_| true), DEFAULT_CMD);
    }

    #[test]
    fn a_second_lock_only_replaces_a_dead_one() {
        let t = Instant::now();
        assert!(accept_new_lock(Phase::Unlocked, false));
        assert!(!accept_new_lock(Phase::Locked, true), "a live lock client keeps it");
        assert!(accept_new_lock(Phase::Locked, false), "a crashed one can be replaced");
        assert!(!accept_new_lock(Phase::Locking { since: t }, true));
    }

    #[test]
    fn confirmed_after_a_cleared_frame() {
        let t = Instant::now();
        let locking = Phase::Locking { since: t };
        assert!(!confirm_due(locking, t + Duration::from_millis(100)));
        assert!(confirm_due(locking, t + CONFIRM_AFTER));
        assert!(!confirm_due(Phase::Locked, t + Duration::from_secs(1)));
        assert!(!confirm_due(Phase::Unlocked, t + Duration::from_secs(1)));
    }
}
