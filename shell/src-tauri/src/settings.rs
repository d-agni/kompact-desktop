//! 10c: the System Settings window. Its page (`web/settings.*`) shows the keyboard shortcuts
//! kompact sends (`keymap`), and changes them with `keys …` requests over the shell channel
//! (`compositor.rs`); kompact checks, applies and saves every change (`kompact/src/keymap.rs`).

use std::sync::Mutex;

use tauri::{AppHandle, Emitter, State};
use tracing::warn;

use crate::compositor::Compositor;

pub const LABEL: &str = "settings";
/// Its entry in the app list (Spotlight, Launchpad).
pub const APP_ID: &str = "kompact-settings";

/// Open the window (or bring it forward), from any thread.
pub fn open(app: &AppHandle, why: &str) {
    tracing::info!("settings: open ({why})");
    let handle = app.clone();
    let _ = app.run_on_main_thread(move || {
        if let Err(e) = crate::build_settings(&handle) {
            warn!("settings: can't open the window: {e}");
        }
    });
}

/// 10d-1: the pane the window should show once open ("dock" from the dock's menu), taken by
/// the page (`take_settings_pane`) when it starts or hears `kompact-settings-pane`.
static PANE: Mutex<Option<String>> = Mutex::new(None);

/// A pane name a page may ask for: a short lowercase word.
pub fn valid_pane(p: &str) -> bool {
    !p.is_empty() && p.len() <= 24 && p.bytes().all(|b| b.is_ascii_lowercase() || b == b'-')
}

#[tauri::command]
pub fn open_settings(app: AppHandle, pane: Option<String>) {
    match pane.filter(|p| valid_pane(p)) {
        Some(p) => {
            tracing::info!("settings: open at the {p} pane");
            *PANE.lock().unwrap_or_else(|e| e.into_inner()) = Some(p);
            open(&app, "from a page");
            // Already open: it switches now (a new window takes it when its page starts).
            let _ = app.emit("kompact-settings-pane", ());
        }
        None => open(&app, "from a page"),
    }
}

/// 12b: open the window at `pane` (a Bluetooth device asking to pair, with nothing open).
pub fn open_at(app: &AppHandle, pane: &str, why: &str) {
    if valid_pane(pane) {
        *PANE.lock().unwrap_or_else(|e| e.into_inner()) = Some(pane.to_string());
    }
    open(app, why);
    let _ = app.emit("kompact-settings-pane", ());
}

#[tauri::command]
pub fn take_settings_pane() -> Option<String> {
    PANE.lock().unwrap_or_else(|e| e.into_inner()).take()
}

/// The line for a page's `keys …` request: one line, nothing that could start another.
pub fn keys_line(args: &str) -> Option<String> {
    let args = args.trim();
    if args.contains(['\n', '\r']) || args.len() > 200 {
        return None;
    }
    Some(if args.is_empty() { "keys\n".to_string() } else { format!("keys {args}\n") })
}

/// 10c: pass a `keys …` request to kompact (`bind window.close Super+W`, `record`, …); the
/// answer comes back as a `kompact-keys-result` / `kompact-recorded` / `kompact-keymap` event.
#[tauri::command]
pub fn keys_command(c: State<'_, Compositor>, args: String) -> Result<(), String> {
    let line = keys_line(&args).ok_or("not a keys request")?;
    c.send_keys_line(&line)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_lines() {
        assert_eq!(keys_line("").as_deref(), Some("keys\n"));
        assert_eq!(keys_line(" bind window.close Super+W ").as_deref(), Some("keys bind window.close Super+W\n"));
        assert_eq!(keys_line("record\nlogout"), None);
        assert_eq!(keys_line(&"x".repeat(300)), None);
    }

    #[test]
    fn pane_names() {
        assert!(valid_pane("dock"));
        assert!(valid_pane("keys"));
        assert!(!valid_pane(""));
        assert!(!valid_pane("Dock"));
        assert!(!valid_pane("../x"));
        assert!(!valid_pane(&"a".repeat(30)));
    }
}
