//! 14c-2: greeter mode, `kompact --greeter <command>`: kompact runs only the login screen
//! (`kompact-greeter`, for greetd) and quits when it exits, so greetd can start the session.
//! Nothing of the desktop runs: no shell (`KOMPACT_SHELL` dropped), no lock screen or idle lock
//! (`KOMPACT_LOCK=off`), nothing exported to the user session and no portals, no theme; and no
//! shortcut does anything (no terminal from the login screen) except Ctrl+Alt+F1–F12, handled
//! before the shortcut table.
//!
//! The command is kompact's startup command (`spawn_cmd`): `Kompact::spawn` hands it here, and
//! its exit is seen in the 50 ms tick (`take_done`).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::OnceLock;

static CMD: OnceLock<String> = OnceLock::new();
static DONE: AtomicBool = AtomicBool::new(false);

/// Set once from `main`, before anything starts.
pub fn set(cmd: &str) {
    let _ = CMD.set(cmd.trim().to_string());
}

pub fn active() -> bool {
    CMD.get().is_some()
}

/// Whether `cmd` is the login screen's command.
pub fn is_greeter_cmd(cmd: &str) -> bool {
    CMD.get().is_some_and(|c| c == cmd.trim())
}

/// The environment greeter mode changes before anything starts: `(name, Some(value))` to set,
/// `(name, None)` to remove.
pub fn env_changes() -> [(&'static str, Option<&'static str>); 3] {
    [("KOMPACT_SHELL", None), ("KOMPACT_LOCK", Some("off")), ("KOMPACT_THEME", Some("off"))]
}

/// Start the login screen; when it exits (however), kompact quits at the next tick.
pub fn spawn(cmd: &str) {
    let cmd = cmd.to_string();
    std::thread::spawn(move || {
        let mut parts = cmd.split_whitespace();
        let Some(program) = parts.next() else {
            DONE.store(true, Ordering::Relaxed);
            return;
        };
        match std::process::Command::new(program).args(parts).spawn() {
            Ok(mut child) => {
                tracing::info!("greeter: started {cmd}");
                match child.wait() {
                    Ok(status) => tracing::info!("greeter: the login screen exited ({status})"),
                    Err(e) => tracing::warn!("greeter: waiting for the login screen: {e}"),
                }
            }
            Err(e) => tracing::warn!("greeter: can't start {cmd}: {e}"),
        }
        DONE.store(true, Ordering::Relaxed);
    });
}

/// Whether the login screen has exited (once).
pub fn take_done() -> bool {
    DONE.swap(false, Ordering::Relaxed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn greeter_mode() {
        // One process-wide setting: set it once here.
        assert!(!is_greeter_cmd("foot"));
        set(" /usr/bin/kompact-greeter --test ");
        assert!(active());
        assert!(is_greeter_cmd("/usr/bin/kompact-greeter --test"));
        assert!(!is_greeter_cmd("foot"));
        assert_eq!(env_changes()[0], ("KOMPACT_SHELL", None));
        assert!(!take_done());
    }
}
