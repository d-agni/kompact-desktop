//! 12i-1b: `org.freedesktop.ScreenSaver` on the session bus, so apps that keep the screen awake
//! over D-Bus reach kompact. The snap Firefox asks the Inhibit portal, and the GTK portal (with
//! no gnome-session) passes that on to this name; Chrome, mpv and VLC call it themselves. Each
//! `Inhibit` gets a cookie; `UnInhibit` or the caller leaving the bus ends it. The apps holding
//! one go to kompact as `keep-awake <app>,<app>`, which counts them like a Wayland idle inhibitor
//! (`idle_tick`). `KOMPACT_SHELL_SCREENSAVER=off` leaves the name alone.

use std::sync::Mutex;

use tauri::{AppHandle, Manager};
use tracing::{info, warn};

const BUS_NAME: &str = "org.freedesktop.ScreenSaver";
/// Both are in use: KDE's path, and the one older apps call.
const PATHS: [&str; 2] = ["/org/freedesktop/ScreenSaver", "/ScreenSaver"];

#[derive(Debug, Clone, PartialEq, Eq)]
struct Hold {
    cookie: u32,
    /// The caller's unique bus name (`:1.42`).
    sender: String,
    app: String,
}

/// Who holds the screen awake.
#[derive(Debug, Default)]
pub struct Holds {
    next: u32,
    holds: Vec<Hold>,
    /// 18c: Control Center → Keep Awake (until it's turned off or the shell restarts).
    pub manual: bool,
}

/// 18c: who holds it when Keep Awake is on.
pub const MANUAL: &str = "Control Center";

/// Most holds kept; an app asking more often than that loses its oldest.
const MAX_HOLDS: usize = 64;

impl Holds {
    pub fn inhibit(&mut self, sender: &str, app: &str) -> u32 {
        self.next = self.next.wrapping_add(1).max(1);
        if self.holds.len() >= MAX_HOLDS {
            self.holds.remove(0);
        }
        self.holds.push(Hold { cookie: self.next, sender: sender.to_string(), app: app_label(app) });
        self.next
    }

    /// Only the caller that got the cookie can end it. Whether it ended one.
    pub fn uninhibit(&mut self, sender: &str, cookie: u32) -> bool {
        let before = self.holds.len();
        self.holds.retain(|h| !(h.cookie == cookie && h.sender == sender));
        self.holds.len() != before
    }

    /// A caller left the bus. Whether it held any.
    pub fn gone(&mut self, sender: &str) -> bool {
        let before = self.holds.len();
        self.holds.retain(|h| h.sender != sender);
        self.holds.len() != before
    }

    /// The apps holding the screen awake, each once, in the order they asked.
    pub fn apps(&self) -> Vec<String> {
        let mut out: Vec<String> = if self.manual { vec![MANUAL.to_string()] } else { Vec::new() };
        for h in &self.holds {
            if !out.contains(&h.app) && out.len() < 16 {
                out.push(h.app.clone());
            }
        }
        out
    }
}

/// The name to show: the portal passes an app id (`snap.firefox`, `org.mozilla.firefox`), apps
/// their own name; no commas or control characters (kompact's list), at most 64 bytes.
pub fn app_label(app: &str) -> String {
    let app = app.trim().trim_start_matches("snap.");
    let app = if app.contains('.') && !app.contains(' ') { app.rsplit('.').next().unwrap_or(app) } else { app };
    let clean: String = app.chars().filter(|c| !c.is_control() && *c != ',').collect();
    let clean = clean.trim();
    if clean.is_empty() {
        return "an app".into();
    }
    let mut end = clean.len().min(64);
    while !clean.is_char_boundary(end) {
        end -= 1;
    }
    clean[..end].to_string()
}

/// `keep-awake <app>,<app>` (none: `keep-awake`).
pub fn keep_awake_line(apps: &[String]) -> String {
    if apps.is_empty() { "keep-awake\n".into() } else { format!("keep-awake {}\n", apps.join(",")) }
}

#[derive(Default)]
pub struct ScreenSaver(Mutex<Holds>);

impl ScreenSaver {
    fn with<T>(&self, f: impl FnOnce(&mut Holds) -> T) -> T {
        f(&mut self.0.lock().unwrap_or_else(|e| e.into_inner()))
    }

    /// Tell kompact who holds it now (also after the shell reconnects to kompact).
    pub fn send(&self, app: &AppHandle) {
        let line = keep_awake_line(&self.with(|h| h.apps()));
        if let Err(e) = app.state::<crate::compositor::Compositor>().send_line_pub("keep-awake", &line) {
            warn!("screensaver: can't tell kompact: {e}");
        }
    }
}

/// 18c: whether Keep Awake is on, and the apps holding the screen awake besides.
#[tauri::command]
pub fn keep_awake_state(state: tauri::State<'_, ScreenSaver>) -> serde_json::Value {
    state.with(|h| serde_json::json!({"on": h.manual, "apps": h.apps().into_iter().filter(|a| a != MANUAL).collect::<Vec<_>>()}))
}

/// 18c: Control Center → Keep Awake.
#[tauri::command]
pub fn set_keep_awake(app: AppHandle, state: tauri::State<'_, ScreenSaver>, on: bool) {
    state.with(|h| h.manual = on);
    info!("screensaver: Keep Awake {}", if on { "on (Control Center)" } else { "off" });
    state.send(&app);
}

struct Server {
    app: AppHandle,
}

#[zbus::interface(name = "org.freedesktop.ScreenSaver")]
impl Server {
    fn inhibit(&self, #[zbus(header)] header: zbus::message::Header<'_>, application_name: String, reason_for_inhibit: String) -> u32 {
        let sender = header.sender().map(|s| s.to_string()).unwrap_or_default();
        let state = self.app.state::<ScreenSaver>();
        let cookie = state.with(|h| h.inhibit(&sender, &application_name));
        info!("screensaver: {application_name:?} keeps the screen awake ({reason_for_inhibit:?}, {sender}, cookie {cookie})");
        state.send(&self.app);
        cookie
    }

    #[zbus(name = "UnInhibit")]
    fn un_inhibit(&self, #[zbus(header)] header: zbus::message::Header<'_>, cookie: u32) {
        let sender = header.sender().map(|s| s.to_string()).unwrap_or_default();
        let state = self.app.state::<ScreenSaver>();
        if state.with(|h| h.uninhibit(&sender, cookie)) {
            info!("screensaver: cookie {cookie} ended ({sender})");
            state.send(&self.app);
        }
    }

    /// Whether the lock screen shows.
    fn get_active(&self) -> bool {
        self.app.state::<crate::power::LockWatch>().locked()
    }

    fn get_active_time(&self) -> u32 {
        0
    }

    /// Apps call this to say the user is there; kompact sees the real input anyway.
    fn simulate_user_activity(&self) {}
}

pub fn enabled() -> bool {
    !std::env::var("KOMPACT_SHELL_SCREENSAVER").is_ok_and(|v| v.trim().eq_ignore_ascii_case("off"))
}

/// Take the name and serve both paths; drop the holds of callers that leave the bus.
pub fn start(app: AppHandle) {
    if !enabled() {
        return info!("screensaver: off (KOMPACT_SHELL_SCREENSAVER=off)");
    }
    tauri::async_runtime::spawn(async move {
        use futures_util::StreamExt;
        let built = async {
            zbus::connection::Builder::session()?
                .serve_at(PATHS[0], Server { app: app.clone() })?
                .serve_at(PATHS[1], Server { app: app.clone() })?
                .name(BUS_NAME)?
                .build()
                .await
        };
        let conn = match built.await {
            Ok(c) => c,
            Err(e) => return warn!("screensaver: can't take {BUS_NAME} ({e}); apps keeping the screen awake over D-Bus won't reach kompact"),
        };
        info!("screensaver: serving {BUS_NAME} on the session bus (12i-1b; KOMPACT_SHELL_SCREENSAVER=off to leave it)");
        let proxy = match zbus::fdo::DBusProxy::new(&conn).await {
            Ok(p) => p,
            Err(e) => return warn!("screensaver: can't watch callers leave: {e}"),
        };
        let mut changes = match proxy.receive_name_owner_changed().await {
            Ok(s) => s,
            Err(e) => return warn!("screensaver: can't watch callers leave: {e}"),
        };
        while let Some(signal) = changes.next().await {
            let Ok(args) = signal.args() else { continue };
            if args.new_owner().is_some() || !args.name().starts_with(':') {
                continue;
            }
            let name = args.name().to_string();
            let state = app.state::<ScreenSaver>();
            if state.with(|h| h.gone(&name)) {
                info!("screensaver: {name} left the bus, its keep-awake ends");
                state.send(&app);
            }
        }
        // Keep the connection (and the name) for the shell's life.
        drop(conn);
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn holds() {
        let mut h = Holds::default();
        let a = h.inhibit(":1.10", "snap.firefox");
        let b = h.inhibit(":1.10", "snap.firefox");
        let c = h.inhibit(":1.20", "mpv");
        assert!(a != b && b != c && a != 0);
        assert_eq!(h.apps(), ["firefox", "mpv"]);
        // Someone else's cookie: not theirs to end.
        assert!(!h.uninhibit(":1.20", a));
        assert!(h.uninhibit(":1.10", a));
        assert_eq!(h.apps(), ["firefox", "mpv"]);
        assert!(h.uninhibit(":1.10", b));
        assert_eq!(h.apps(), ["mpv"]);
        // mpv quits without UnInhibit.
        assert!(h.gone(":1.20"));
        assert!(!h.gone(":1.20"));
        assert!(h.apps().is_empty());
        assert_eq!(keep_awake_line(&h.apps()), "keep-awake\n");
        assert_eq!(keep_awake_line(&["firefox".into(), "mpv".into()]), "keep-awake firefox,mpv\n");
        // Bounded.
        for _ in 0..100 {
            h.inhibit(":1.30", "spam");
        }
        assert_eq!(h.holds.len(), MAX_HOLDS);
    }

    #[test]
    fn keep_awake_switch() {
        let mut h = Holds::default();
        h.inhibit(":1.10", "mpv");
        h.manual = true;
        assert_eq!(h.apps(), [MANUAL, "mpv"]);
        // mpv leaving doesn't end it; turning it off does.
        h.gone(":1.10");
        assert_eq!(keep_awake_line(&h.apps()), "keep-awake Control Center\n");
        h.manual = false;
        assert!(h.apps().is_empty());
    }

    #[test]
    fn labels() {
        assert_eq!(app_label("snap.firefox"), "firefox");
        assert_eq!(app_label("org.mozilla.firefox"), "firefox");
        assert_eq!(app_label("Google Chrome"), "Google Chrome");
        assert_eq!(app_label("mpv, the player\n"), "mpv the player");
        assert_eq!(app_label(""), "an app");
        assert_eq!(app_label(&"é".repeat(40)).len(), 64);
    }
}
