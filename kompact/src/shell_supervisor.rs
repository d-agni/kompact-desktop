//! 6d: start kompact-shell from kompact and bring it back if it crashes.
//!
//! Off unless `KOMPACT_SHELL` is set: `on` finds the binary (`$PATH`, then the shell's release
//! build next to this kompact checkout), anything else is taken as its path. Polled from a
//! timer in `state.rs` (no event source of its own, the backend is untouched); the first tick
//! comes once the event loop runs, after the backend has opened the Wayland socket.
//!
//! What the shell's exit means (`classify`):
//! - 0, or SIGTERM/SIGINT/SIGHUP (someone stopped it, e.g. `pkill`): leave it stopped;
//! - `EXIT_RESTART` (the logo menu's Restart Shell when supervised): start it again now;
//! - `EXIT_ALREADY_RUNNING` (another copy has the display): leave it;
//! - anything else (a crash, an abort, SIGKILL from the OOM killer): start it again after 1,
//!   2, 4 s; after `MAX_CRASHES` crashes within `CRASH_WINDOW`, give up until kompact restarts.
//!
//! Its output goes to `KOMPACT_SHELL_LOG` (default `~/.local/state/kompact/shell.log`),
//! emptied when kompact starts and appended to across restarts, so a crash's last words stay.

use std::{
    fs::OpenOptions,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::Mutex,
    time::{Duration, Instant},
};

/// Exit codes shared with kompact-shell (`shell/src-tauri/src/main.rs`).
pub const EXIT_ALREADY_RUNNING: i32 = 3;
pub const EXIT_RESTART: i32 = 75;
/// Tells the shell it is supervised (so Restart Shell exits with `EXIT_RESTART`).
pub const SUPERVISED_ENV: &str = "KOMPACT_SHELL_SUPERVISED";

pub const MAX_CRASHES: usize = 3;
pub const CRASH_WINDOW: Duration = Duration::from_secs(60);

const SIGHUP: i32 = 1;
const SIGINT: i32 = 2;
const SIGTERM: i32 = 15;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Mode {
    Off,
    /// Find the binary.
    Auto,
    Path(PathBuf),
}

pub fn mode_from_env(value: Option<&str>) -> Mode {
    match value.map(str::trim) {
        None | Some("") => Mode::Off,
        Some(v) => match v.to_ascii_lowercase().as_str() {
            "off" | "0" | "no" | "false" => Mode::Off,
            "on" | "1" | "yes" | "true" => Mode::Auto,
            _ => Mode::Path(PathBuf::from(v)),
        },
    }
}

/// Where to look for the shell, in order: `kompact-shell` on `$PATH`, then the release build in
/// this checkout (kompact at `<root>/kompact/target/release/kompact`, the shell at
/// `<root>/shell/src-tauri/target/release/kompact-shell`).
pub fn candidates(path_var: Option<&str>, exe: Option<&Path>) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = path_var
        .map(|p| std::env::split_paths(p).map(|d| d.join("kompact-shell")).collect())
        .unwrap_or_default();
    if let Some(root) = exe.and_then(|e| e.ancestors().nth(4)) {
        out.push(root.join("shell/src-tauri/target/release/kompact-shell"));
    }
    out
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Exit {
    Stopped,
    Restart,
    AlreadyRunning,
    Crashed,
}

/// What an exit with `code` (normal exit) or `signal` (killed) means.
pub fn classify(code: Option<i32>, signal: Option<i32>) -> Exit {
    match (code, signal) {
        (Some(0), _) => Exit::Stopped,
        (Some(EXIT_RESTART), _) => Exit::Restart,
        (Some(EXIT_ALREADY_RUNNING), _) => Exit::AlreadyRunning,
        (_, Some(SIGTERM | SIGINT | SIGHUP)) => Exit::Stopped,
        _ => Exit::Crashed,
    }
}

/// Recent crashes, for the restart delay.
#[derive(Debug, Default)]
pub struct Backoff {
    crashes: Vec<Instant>,
}

impl Backoff {
    /// A crash at `now`: how long to wait before starting again, or `None` to give up.
    pub fn crashed(&mut self, now: Instant) -> Option<Duration> {
        self.crashes.retain(|t| now.duration_since(*t) < CRASH_WINDOW);
        self.crashes.push(now);
        let n = self.crashes.len();
        (n <= MAX_CRASHES).then(|| Duration::from_secs(1 << (n - 1)))
    }
}

/// The running shell's pid, for `cleanup` after the backend has returned.
static PID: Mutex<Option<u32>> = Mutex::new(None);

pub struct Supervisor {
    mode: Mode,
    child: Option<(Child, Instant)>,
    next_start: Option<Instant>,
    backoff: Backoff,
    log: PathBuf,
    started_once: bool,
    looked: bool,
}

impl Default for Supervisor {
    fn default() -> Self {
        let log = std::env::var_os("KOMPACT_SHELL_LOG").map(PathBuf::from).unwrap_or_else(default_log);
        Supervisor {
            mode: mode_from_env(std::env::var("KOMPACT_SHELL").ok().as_deref()),
            child: None,
            next_start: None,
            backoff: Backoff::default(),
            log,
            started_once: false,
            looked: false,
        }
    }
}

fn default_log() -> PathBuf {
    let state = std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| Path::new(&h).join(".local/state")))
        .unwrap_or_else(std::env::temp_dir);
    state.join("kompact/shell.log")
}

impl Supervisor {
    /// From a timer in the event loop.
    pub fn tick(&mut self) {
        if self.mode == Mode::Off {
            return;
        }
        if !self.looked {
            self.looked = true;
            tracing::info!("shell: supervised (KOMPACT_SHELL), log {}", self.log.display());
            self.next_start = Some(Instant::now());
        }
        if let Some((child, since)) = &mut self.child {
            let status = match child.try_wait() {
                Ok(None) => return,
                Ok(Some(s)) => s,
                Err(e) => {
                    tracing::warn!("shell: can't check on it: {e}");
                    return;
                }
            };
            let ran = since.elapsed();
            self.child = None;
            set_pid(None);
            use std::os::unix::process::ExitStatusExt;
            let (code, signal) = (status.code(), status.signal());
            match classify(code, signal) {
                Exit::Stopped => tracing::info!("shell: exited ({status}) after {} s; leaving it stopped", ran.as_secs()),
                Exit::AlreadyRunning => tracing::info!("shell: another kompact-shell is running on this display; leaving that one"),
                Exit::Restart => {
                    tracing::info!("shell: asked to restart");
                    self.next_start = Some(Instant::now());
                }
                Exit::Crashed => match self.backoff.crashed(Instant::now()) {
                    Some(wait) => {
                        tracing::warn!("shell: crashed ({status}) after {} s; starting it again in {} s", ran.as_secs(), wait.as_secs());
                        self.next_start = Some(Instant::now() + wait);
                    }
                    None => tracing::warn!(
                        "shell: crashed ({status}), more than {MAX_CRASHES} times in {} s; giving up (see {})",
                        CRASH_WINDOW.as_secs(),
                        self.log.display()
                    ),
                },
            }
        }
        if self.next_start.is_some_and(|t| Instant::now() >= t) {
            self.next_start = None;
            self.start();
        }
    }

    fn binary(&self) -> Option<PathBuf> {
        match &self.mode {
            Mode::Off => None,
            Mode::Path(p) => Some(p.clone()),
            Mode::Auto => {
                let exe = std::env::current_exe().ok();
                let path = std::env::var("PATH").ok();
                candidates(path.as_deref(), exe.as_deref()).into_iter().find(|p| p.is_file())
            }
        }
    }

    fn start(&mut self) {
        let Some(bin) = self.binary() else {
            tracing::warn!("shell: kompact-shell not found (put it on $PATH or set KOMPACT_SHELL to its path)");
            return;
        };
        // Portals before the shell's GTK asks them anything (6m-d5); a no-op after the first time.
        crate::session_env::export_once();
        if let Some(dir) = self.log.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let log = OpenOptions::new()
            .create(true)
            .write(true)
            .append(self.started_once)
            .truncate(!self.started_once)
            .open(&self.log);
        let (out, err) = match log.and_then(|f| Ok((f.try_clone()?, f))) {
            Ok((a, b)) => (Stdio::from(a), Stdio::from(b)),
            Err(e) => {
                tracing::warn!("shell: can't open {}: {e}; its output is dropped", self.log.display());
                (Stdio::null(), Stdio::null())
            }
        };
        self.started_once = true;
        let mut cmd = Command::new(&bin);
        cmd.env(SUPERVISED_ENV, "1").stdin(Stdio::null()).stdout(out).stderr(err);
        // Its own process group: a Ctrl+C meant for whatever started kompact doesn't reach it.
        std::os::unix::process::CommandExt::process_group(&mut cmd, 0);
        match cmd.spawn() {
            Ok(child) => {
                tracing::info!("shell: started {} (pid {})", bin.display(), child.id());
                set_pid(Some(child.id()));
                self.child = Some((child, Instant::now()));
            }
            Err(e) => tracing::warn!("shell: can't start {}: {e}", bin.display()),
        }
    }
}

fn set_pid(pid: Option<u32>) {
    if let Ok(mut p) = PID.lock() {
        *p = pid;
    }
}

/// On kompact's exit (called from `main` after the backend returns): stop the shell if it's
/// still running. It usually is gone already, having lost its display.
pub fn cleanup() {
    let Some(pid) = PID.lock().ok().and_then(|mut p| p.take()) else { return };
    let _ = Command::new("kill")
        .args(["-TERM", &pid.to_string()])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn switch() {
        assert_eq!(mode_from_env(None), Mode::Off);
        assert_eq!(mode_from_env(Some("")), Mode::Off);
        assert_eq!(mode_from_env(Some("off")), Mode::Off);
        assert_eq!(mode_from_env(Some("ON")), Mode::Auto);
        assert_eq!(mode_from_env(Some("/opt/kompact-shell")), Mode::Path("/opt/kompact-shell".into()));
    }

    #[test]
    fn finds_the_checkout_build() {
        let exe = Path::new("/home/u/kompact-de/kompact/target/release/kompact");
        assert_eq!(
            candidates(Some("/usr/local/bin:/usr/bin"), Some(exe)),
            [
                PathBuf::from("/usr/local/bin/kompact-shell"),
                "/usr/bin/kompact-shell".into(),
                "/home/u/kompact-de/shell/src-tauri/target/release/kompact-shell".into(),
            ]
        );
        assert!(candidates(None, None).is_empty());
    }

    #[test]
    fn what_exits_mean() {
        assert_eq!(classify(Some(0), None), Exit::Stopped);
        assert_eq!(classify(None, Some(SIGTERM)), Exit::Stopped, "pkill");
        assert_eq!(classify(None, Some(SIGHUP)), Exit::Stopped);
        assert_eq!(classify(Some(EXIT_RESTART), None), Exit::Restart);
        assert_eq!(classify(Some(EXIT_ALREADY_RUNNING), None), Exit::AlreadyRunning);
        assert_eq!(classify(Some(1), None), Exit::Crashed);
        assert_eq!(classify(None, Some(11)), Exit::Crashed, "SIGSEGV");
        assert_eq!(classify(None, Some(9)), Exit::Crashed, "SIGKILL (OOM killer)");
    }

    #[test]
    fn backs_off_then_gives_up() {
        let t0 = Instant::now();
        let mut b = Backoff::default();
        assert_eq!(b.crashed(t0), Some(Duration::from_secs(1)));
        assert_eq!(b.crashed(t0 + Duration::from_secs(5)), Some(Duration::from_secs(2)));
        assert_eq!(b.crashed(t0 + Duration::from_secs(10)), Some(Duration::from_secs(4)));
        assert_eq!(b.crashed(t0 + Duration::from_secs(15)), None, "4th within a minute");
        // Long after: the old crashes no longer count.
        let mut b = Backoff::default();
        b.crashed(t0);
        b.crashed(t0 + Duration::from_secs(1));
        assert_eq!(b.crashed(t0 + Duration::from_secs(120)), Some(Duration::from_secs(1)));
    }
}
