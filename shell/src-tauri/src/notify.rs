//! 8c: notifications. The shell owns `org.freedesktop.Notifications` on the session bus (the
//! Desktop Notifications spec 1.2, `zbus`) and shows each one as a macOS-style banner in the
//! top-right corner: its own layer surface (`notify.html`), on the top layer, shown only while
//! there are banners and sized to them.
//!
//! A banner goes away after its timeout (`expire_ms`), when clicked (the app's "default" action
//! runs), on its close button, or when the app closes it; each time the app gets
//! `NotificationClosed` with the spec's reason. `KOMPACT_SHELL_NOTIFICATIONS=off` leaves the bus
//! name to someone else.
//!
//! 12d: every notification also goes into the Notification Center (the clock's menu; the last
//! `CENTER_MAX`, newest first, until cleared). System Settings → Notifications (kept in
//! `~/.config/kompact/notifications`): Do Not Disturb (no banners, except critical ones; they
//! still go into the Center), how long banners stay, and which apps may notify at all (apps are
//! listed as they first notify).
//!
//! Pure rules first (tested); then the D-Bus server, the commands the page calls, and the window.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Mutex;

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State};
use tracing::{info, warn};

pub const LABEL: &str = "notify";
pub const BUS_NAME: &str = "org.freedesktop.Notifications";
const PATH: &str = "/org/freedesktop/Notifications";
/// Banner column width (the page lays the cards out inside it).
pub const WIDTH: i32 = 380;
/// How long a banner stays when the app leaves it to us (macOS: about 5 s).
pub const DEFAULT_MS: u32 = 5000;

/// Why a notification went away (the spec's `NotificationClosed` reasons).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reason {
    Expired = 1,
    Dismissed = 2,
    Closed = 3,
}

impl Reason {
    pub fn from_u32(n: u32) -> Option<Reason> {
        match n {
            1 => Some(Reason::Expired),
            2 => Some(Reason::Dismissed),
            3 => Some(Reason::Closed),
            _ => None,
        }
    }
}

/// How long the banner shows (`None`: until dismissed). The spec: -1 = the server decides,
/// 0 = never expires; critical notifications (urgency 2) stay until dismissed.
pub fn expire_ms(expire_timeout: i32, urgency: Option<u8>) -> Option<u32> {
    if urgency == Some(2) {
        return None;
    }
    match expire_timeout {
        t if t < 0 => Some(DEFAULT_MS),
        0 => None,
        t => Some(t as u32),
    }
}

/// The spec's body markup (`<b>`, `<i>`, `<u>`, `<a href>`, `<img>`) as plain text: tags
/// dropped, the five XML entities decoded. The page shows it as text, never as HTML.
pub fn plain_text(body: &str) -> String {
    let mut out = String::with_capacity(body.len());
    let mut in_tag = false;
    for c in body.chars() {
        match c {
            '<' => in_tag = true,
            '>' if in_tag => in_tag = false,
            c if !in_tag => out.push(c),
            _ => {}
        }
    }
    out.replace("&lt;", "<").replace("&gt;", ">").replace("&quot;", "\"").replace("&apos;", "'").replace("&amp;", "&")
}

/// `actions` comes as key, label, key, label, …; an odd one out is dropped.
pub fn action_pairs(actions: &[String]) -> Vec<(String, String)> {
    actions.chunks_exact(2).map(|p| (p[0].clone(), p[1].clone())).collect()
}

/// Where a notification's picture comes from, best first.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IconSource {
    /// A file (the `image-path` hint or `app_icon` as a path or `file://` URI).
    File(PathBuf),
    /// A theme icon name.
    Name(String),
    /// The icon of this desktop entry (`desktop-entry` hint), from the installed apps.
    DesktopEntry(String),
}

/// Candidates in order: `image-path`, `app_icon`, `desktop-entry`.
pub fn icon_sources(app_icon: &str, image_path: Option<&str>, desktop_entry: Option<&str>) -> Vec<IconSource> {
    let named = |s: &str| -> Option<IconSource> {
        let s = s.trim();
        if s.is_empty() {
            None
        } else if let Some(p) = s.strip_prefix("file://") {
            Some(IconSource::File(PathBuf::from(p)))
        } else if s.starts_with('/') {
            Some(IconSource::File(PathBuf::from(s)))
        } else {
            Some(IconSource::Name(s.to_string()))
        }
    };
    let mut out: Vec<IconSource> = image_path.and_then(named).into_iter().collect();
    out.extend(named(app_icon));
    if let Some(entry) = desktop_entry.map(str::trim).filter(|e| !e.is_empty()) {
        out.push(IconSource::DesktopEntry(entry.trim_end_matches(".desktop").to_string()));
    }
    out
}

/// Whether notifications are on (`KOMPACT_SHELL_NOTIFICATIONS=off`: no).
pub fn enabled_from_env(value: Option<&str>) -> bool {
    !matches!(value.map(|v| v.trim().to_ascii_lowercase()).as_deref(), Some("off" | "0" | "no" | "false"))
}

/// 12d: the Notification Center keeps at most this many.
pub const CENTER_MAX: usize = 50;
/// 12d: the banner times Settings offers (ms).
pub const BANNER_CHOICES: [u32; 3] = [3000, 5000, 10000];

/// 12d: the notification settings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct NotifyPrefs {
    pub dnd: bool,
    pub banner_ms: u32,
    /// Apps that may not notify.
    pub muted: Vec<String>,
    /// Apps seen so far (for the list in Settings).
    pub seen: Vec<String>,
}

impl Default for NotifyPrefs {
    fn default() -> Self {
        NotifyPrefs { dnd: false, banner_ms: DEFAULT_MS, muted: Vec::new(), seen: Vec::new() }
    }
}

/// Names joined with `;` in the file (an app name with `;` or a newline is left out).
fn list_text(list: &[String]) -> String {
    list.iter().filter(|a| !a.contains([';', '\n', '\r'])).cloned().collect::<Vec<_>>().join(";")
}

impl NotifyPrefs {
    pub fn parse(text: &str) -> NotifyPrefs {
        let mut p = NotifyPrefs::default();
        let list = |v: &str| v.split(';').map(str::trim).filter(|a| !a.is_empty()).map(str::to_string).collect::<Vec<_>>();
        for line in text.lines().map(str::trim).filter(|l| !l.starts_with('#')) {
            let Some((k, v)) = line.split_once('=') else { continue };
            match (k.trim(), v.trim()) {
                ("dnd", "on") => p.dnd = true,
                ("dnd", "off") => p.dnd = false,
                ("banner-ms", n) => {
                    if let Some(ms) = n.parse().ok().filter(|ms| BANNER_CHOICES.contains(ms)) {
                        p.banner_ms = ms;
                    }
                }
                ("muted", v) => p.muted = list(v),
                ("seen", v) => p.seen = list(v),
                _ => {}
            }
        }
        p
    }

    pub fn to_text(&self) -> String {
        format!(
            "# kompact notifications (written by System Settings → Notifications)\ndnd={}\nbanner-ms={}\nmuted={}\nseen={}\n",
            if self.dnd { "on" } else { "off" },
            self.banner_ms,
            list_text(&self.muted),
            list_text(&self.seen)
        )
    }

    /// Whether a notification from `app` shows a banner now.
    pub fn banner_for(&self, app: &str, critical: bool) -> bool {
        !self.muted.iter().any(|m| m == app) && (critical || !self.dnd)
    }
}

fn prefs_path() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
    Some(base.join("kompact").join("notifications"))
}

fn load_prefs() -> NotifyPrefs {
    prefs_path().and_then(|p| std::fs::read_to_string(p).ok()).map(|t| NotifyPrefs::parse(&t)).unwrap_or_default()
}

fn save_prefs(p: &NotifyPrefs) {
    let Some(path) = prefs_path() else { return };
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Err(e) = std::fs::write(&path, p.to_text()) {
        warn!("notifications: can't save {}: {e}", path.display());
    }
}

/// 12d: one notification in the Center.
#[derive(Debug, Clone, Serialize)]
pub struct Entry {
    pub id: u32,
    pub app: String,
    pub summary: String,
    pub body: String,
    /// Seconds since 1970, for "5 min ago".
    pub time: u64,
    /// A click does something (the app's default action, or our own screenshot).
    pub clickable: bool,
    /// Whether `kompact-icon://localhost/center-<id>` has a picture.
    pub icon: bool,
}

/// Newest first, at most `CENTER_MAX`; a replaced notification (same id) moves to the top.
pub fn add_entry(list: &mut Vec<Entry>, e: Entry) -> Vec<u32> {
    list.retain(|x| x.id != e.id);
    list.insert(0, e);
    let dropped: Vec<u32> = list.iter().skip(CENTER_MAX).map(|x| x.id).collect();
    list.truncate(CENTER_MAX);
    dropped
}

/// What the page gets for each banner (the `kompact-notify` event).
#[derive(Debug, Clone, Serialize)]
pub struct Banner {
    pub id: u32,
    pub app: String,
    pub summary: String,
    pub body: String,
    pub actions: Vec<(String, String)>,
    pub expire_ms: Option<u32>,
    pub critical: bool,
    /// Whether `kompact-icon://localhost/notify-<id>` has a picture (else the page draws a badge).
    pub icon: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
enum PageEvent {
    Show(Banner),
    Close { id: u32 },
}

/// Shared by the D-Bus server and the page's commands.
#[derive(Default)]
pub struct Notifications {
    conn: Mutex<Option<zbus::Connection>>,
    next: AtomicU32,
    /// Ids the page shows, so a close from the app for one that's gone is a no-op.
    open: Mutex<Vec<u32>>,
    /// Banners that came before the page had loaded (`notify_ready` takes them).
    queued: Mutex<Option<Vec<Banner>>>,
    /// 9c: our own banners (no app behind them): id → the screenshot a click opens.
    own: Mutex<HashMap<u32, PathBuf>>,
    /// 12d: the settings and the Notification Center.
    prefs: Mutex<Option<NotifyPrefs>>,
    center: Mutex<Vec<Entry>>,
}

impl Notifications {
    fn with_prefs<T>(&self, f: impl FnOnce(&mut NotifyPrefs) -> T) -> T {
        let mut g = self.prefs.lock().unwrap_or_else(|e| e.into_inner());
        f(g.get_or_insert_with(load_prefs))
    }
}

impl Notifications {
    pub fn new() -> Notifications {
        Notifications { queued: Mutex::new(Some(Vec::new())), ..Default::default() }
    }

    fn next_id(&self) -> u32 {
        // Ids start at 1 (0 means "new" in `replaces_id`).
        self.next.fetch_add(1, Ordering::Relaxed) + 1
    }

    fn signal(&self, name: &'static str, body: impl serde::Serialize + zbus::zvariant::DynamicType + Send + Sync + 'static) {
        let Some(conn) = self.conn.lock().unwrap_or_else(|e| e.into_inner()).clone() else { return };
        tauri::async_runtime::spawn(async move {
            if let Err(e) = conn.emit_signal(None::<()>, PATH, BUS_NAME, name, &body).await {
                warn!("notifications: can't send {name}: {e}");
            }
        });
    }
}

struct Server {
    app: AppHandle,
}

#[zbus::interface(name = "org.freedesktop.Notifications")]
impl Server {
    fn get_capabilities(&self) -> Vec<String> {
        ["actions", "body", "body-markup", "icon-static", "persistence"].iter().map(|s| s.to_string()).collect()
    }

    #[allow(clippy::too_many_arguments)]
    fn notify(
        &self,
        app_name: String,
        replaces_id: u32,
        app_icon: String,
        summary: String,
        body: String,
        actions: Vec<String>,
        hints: HashMap<String, zbus::zvariant::OwnedValue>,
        expire_timeout: i32,
    ) -> u32 {
        let state = self.app.state::<Notifications>();
        let id = if replaces_id != 0 { replaces_id } else { state.next_id() };
        let text = |k: &str| hints.get(k).and_then(|v| <&str>::try_from(&**v).ok().map(str::to_string));
        let urgency = hints.get("urgency").and_then(|v| u8::try_from(&**v).ok());
        let sources = icon_sources(&app_icon, text("image-path").or_else(|| text("image_path")).as_deref(), text("desktop-entry").as_deref());
        let banner = Banner {
            id,
            app: app_name,
            summary,
            body: plain_text(&body),
            actions: action_pairs(&actions),
            expire_ms: expire_ms(expire_timeout, urgency).map(|ms| if expire_timeout < 0 { state.with_prefs(|p| p.banner_ms) } else { ms }),
            critical: urgency == Some(2),
            icon: false,
        };
        // 12d: a new app goes into Settings' list.
        let app_name = banner.app.clone();
        let new_app = !app_name.is_empty()
            && state.with_prefs(|p| {
                let new = !p.seen.contains(&app_name);
                if new {
                    p.seen.push(app_name.clone());
                    save_prefs(p);
                }
                new
            });
        if new_app {
            info!("notifications: first from {app_name:?}");
        }
        if state.with_prefs(|p| p.muted.contains(&app_name)) {
            info!("notifications: {id} from {app_name:?} not shown (turned off in Settings)");
            return id;
        }
        info!(
            "notifications: {} from {:?}: {:?} ({}, {} actions)",
            id,
            banner.app,
            banner.summary,
            banner.expire_ms.map_or("until dismissed".to_string(), |ms| format!("{ms} ms")),
            banner.actions.len()
        );
        let app = self.app.clone();
        // GTK (the icon theme, the window) only on the main thread.
        let _ = self.app.run_on_main_thread(move || show(&app, banner, &sources));
        id
    }

    fn close_notification(&self, id: u32) {
        let state = self.app.state::<Notifications>();
        if !state.open.lock().unwrap_or_else(|e| e.into_inner()).contains(&id) {
            return;
        }
        info!("notifications: {id} closed by the app");
        // The page removes it and answers with `notify_closed` (reason 3).
        let _ = self.app.emit("kompact-notify", PageEvent::Close { id });
    }

    fn get_server_information(&self) -> (String, String, String, String) {
        ("kompact-shell".into(), "kompact".into(), env!("CARGO_PKG_VERSION").into(), "1.2".into())
    }
}

/// Resolve the picture and hand the banner to the page (main thread).
fn show(app: &AppHandle, mut banner: Banner, sources: &[IconSource]) {
    let overlay = app.state::<crate::overlay::Overlay>();
    let theme = gtk::IconTheme::default();
    let file = sources.iter().find_map(|s| match s {
        IconSource::File(p) => crate::overlay::icon_file(theme.as_ref(), &p.to_string_lossy()),
        IconSource::Name(n) => crate::overlay::icon_file(theme.as_ref(), n),
        IconSource::DesktopEntry(e) => overlay.icon_path(&format!("{e}.desktop")),
    });
    if let Some(file) = file {
        // 12d: the Center keeps its own copy of the picture (the banner's goes with the banner).
        overlay.add_icon(format!("center-{}", banner.id), file.clone());
        overlay.add_icon(format!("notify-{}", banner.id), file);
        banner.icon = true;
    }
    let state = app.state::<Notifications>();
    // 12d: into the Notification Center, banner or not.
    let clickable = banner.actions.iter().any(|(k, _)| k == "default") || state.own.lock().unwrap_or_else(|e| e.into_inner()).contains_key(&banner.id);
    let entry = Entry {
        id: banner.id,
        app: banner.app.clone(),
        summary: banner.summary.clone(),
        body: banner.body.clone(),
        time: std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs()),
        clickable,
        icon: banner.icon,
    };
    let dropped = add_entry(&mut state.center.lock().unwrap_or_else(|e| e.into_inner()), entry);
    for id in dropped {
        overlay.remove_icon(&format!("center-{id}"));
    }
    let _ = app.emit("kompact-notify-center", ());
    if !state.with_prefs(|p| p.banner_for(&banner.app, banner.critical)) {
        info!("notifications: {} in the Center only (Do Not Disturb)", banner.id);
        return;
    }
    state.open.lock().unwrap_or_else(|e| e.into_inner()).push(banner.id);
    if let Some(queue) = state.queued.lock().unwrap_or_else(|e| e.into_inner()).as_mut() {
        queue.push(banner);
        return;
    }
    if let Err(e) = app.emit("kompact-notify", PageEvent::Show(banner)) {
        warn!("notifications: can't reach the banner page: {e}");
    }
}

/// 9c: the body of a "Screenshot saved" banner: the file's name.
pub fn screenshot_body(path: &std::path::Path) -> String {
    path.file_name().map_or_else(|| path.to_string_lossy().into_owned(), |n| n.to_string_lossy().into_owned())
}

/// 9c: kompact saved a screenshot: a banner with it as the picture; a click opens it.
pub fn screenshot_saved(app: &AppHandle, path: PathBuf) {
    let state = app.state::<Notifications>();
    let id = state.next_id();
    state.own.lock().unwrap_or_else(|e| e.into_inner()).insert(id, path.clone());
    let banner = Banner {
        id,
        app: "Screenshot".into(),
        summary: "Screenshot saved".into(),
        body: screenshot_body(&path),
        actions: vec![("default".into(), "Open".into())],
        expire_ms: Some(DEFAULT_MS),
        critical: false,
        icon: false,
    };
    info!("notifications: {id} from kompact: screenshot {}", banner.body);
    let handle = app.clone();
    let _ = app.run_on_main_thread(move || show(&handle, banner, &[IconSource::File(path)]));
}

/// Take the bus name and serve, on Tauri's async runtime. Logs and gives up if someone else
/// (another notification daemon) has the name.
pub fn start(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        let server = Server { app: app.clone() };
        let built = async { zbus::connection::Builder::session()?.serve_at(PATH, server)?.name(BUS_NAME)?.build().await };
        match built.await {
            Ok(conn) => {
                info!("notifications: serving {BUS_NAME} on the session bus (8c; KOMPACT_SHELL_NOTIFICATIONS=off to leave it)");
                *app.state::<Notifications>().conn.lock().unwrap_or_else(|e| e.into_inner()) = Some(conn);
            }
            Err(e) => warn!("notifications: can't take {BUS_NAME} ({e}); another notification daemon may be running"),
        }
    });
}

/// The page has loaded: the banners that came before it (and from now on, events).
#[tauri::command]
pub fn notify_ready(state: State<'_, Notifications>) -> Vec<Banner> {
    let queued = state.queued.lock().unwrap_or_else(|e| e.into_inner()).take().unwrap_or_default();
    info!("notifications: page ready ({} waiting)", queued.len());
    queued
}

/// The page: a banner went away (`reason`: 1 expired, 2 dismissed, 3 closed by the app).
#[tauri::command]
pub fn notify_closed(app: AppHandle, state: State<'_, Notifications>, id: u32, reason: u32) {
    let Some(reason) = Reason::from_u32(reason) else { return };
    state.open.lock().unwrap_or_else(|e| e.into_inner()).retain(|&i| i != id);
    app.state::<crate::overlay::Overlay>().remove_icon(&format!("notify-{id}"));
    info!("notifications: {id} gone ({reason:?})");
    if state.own.lock().unwrap_or_else(|e| e.into_inner()).remove(&id).is_some() {
        return; // ours (9c): no app to tell
    }
    state.signal("NotificationClosed", (id, reason as u32));
}

/// The page: an action was chosen (`default` for a click on the banner).
#[tauri::command]
pub fn notify_action(app: AppHandle, state: State<'_, Notifications>, id: u32, key: String) {
    info!("notifications: {id} action {key:?}");
    // 9c: our own banner: open the screenshot (the page then closes the banner).
    let own = state.own.lock().unwrap_or_else(|e| e.into_inner()).get(&id).cloned();
    if let Some(path) = own {
        info!("notifications: opening {}", path.display());
        app.state::<crate::overlay::Overlay>().open_path(&app, path);
        return;
    }
    state.signal("ActionInvoked", (id, key));
}

// ---- 12d: the Notification Center and the settings ----

#[tauri::command]
pub fn notify_center(state: State<'_, Notifications>) -> serde_json::Value {
    let entries = state.center.lock().unwrap_or_else(|e| e.into_inner()).clone();
    serde_json::json!({"dnd": state.with_prefs(|p| p.dnd), "entries": entries})
}

/// Clear one (`id`) or all.
#[tauri::command]
pub fn notify_center_clear(app: AppHandle, state: State<'_, Notifications>, id: Option<u32>) {
    let removed: Vec<u32> = {
        let mut list = state.center.lock().unwrap_or_else(|e| e.into_inner());
        let gone: Vec<u32> = list.iter().filter(|e| id.is_none_or(|i| e.id == i)).map(|e| e.id).collect();
        list.retain(|e| !gone.contains(&e.id));
        gone
    };
    let overlay = app.state::<crate::overlay::Overlay>();
    for i in &removed {
        overlay.remove_icon(&format!("center-{i}"));
    }
    info!("notifications: {} cleared from the Center", removed.len());
    let _ = app.emit("kompact-notify-center", ());
}

/// A click on one in the Center: what a click on its banner does, then it goes.
#[tauri::command]
pub fn notify_center_open(app: AppHandle, state: State<'_, Notifications>, id: u32) {
    notify_action(app.clone(), state.clone(), id, "default".into());
    notify_center_clear(app, state, Some(id));
}

#[tauri::command]
pub fn notify_prefs(state: State<'_, Notifications>) -> NotifyPrefs {
    state.with_prefs(|p| p.clone())
}

/// Do Not Disturb on/off, the banner time, and the apps that may not notify.
#[tauri::command]
pub fn set_notify_prefs(app: AppHandle, state: State<'_, Notifications>, dnd: bool, banner_ms: u32, muted: Vec<String>) -> Result<NotifyPrefs, String> {
    if !BANNER_CHOICES.contains(&banner_ms) {
        return Err(format!("{banner_ms} ms isn't one of the banner times"));
    }
    let p = state.with_prefs(|p| {
        p.dnd = dnd;
        p.banner_ms = banner_ms;
        p.muted = muted;
        save_prefs(p);
        p.clone()
    });
    info!("notifications: Do Not Disturb {}, banners {} ms, {} apps off", if p.dnd { "on" } else { "off" }, p.banner_ms, p.muted.len());
    let _ = app.emit("kompact-notify-prefs", &p);
    let _ = app.emit("kompact-notify-center", ());
    Ok(p)
}

/// The page: its banners now take `height` px (0: none, hide the window).
#[tauri::command]
pub fn notify_size(app: AppHandle, height: i32) {
    use gtk::prelude::*;
    let Some(window) = app.get_webview_window(LABEL) else { return };
    let visible = window.is_visible().unwrap_or(false);
    if height <= 0 {
        if visible {
            let _ = window.hide();
            info!("notifications: no banners left, window hidden");
        }
        return;
    }
    if let Ok(gtk_window) = window.gtk_window() {
        gtk_window.set_size_request(WIDTH, height);
    }
    if !visible {
        match window.show() {
            Ok(()) => info!("notifications: window shown ({WIDTH}x{height})"),
            Err(e) => warn!("notifications: can't show the banners: {e}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn screenshot_banner_text() {
        let p = std::path::Path::new("/home/u/Pictures/Screenshots/Screenshot 2026-10-11 at 09.03.07.png");
        assert_eq!(screenshot_body(p), "Screenshot 2026-10-11 at 09.03.07.png");
    }

    #[test]
    fn timeouts() {
        assert_eq!(expire_ms(-1, None), Some(DEFAULT_MS));
        assert_eq!(expire_ms(0, None), None, "0: never expires");
        assert_eq!(expire_ms(8000, Some(1)), Some(8000));
        assert_eq!(expire_ms(-1, Some(2)), None, "critical stays");
        assert_eq!(expire_ms(3000, Some(2)), None);
    }

    #[test]
    fn markup_becomes_text() {
        assert_eq!(plain_text("<b>Build</b> done &amp; <a href=\"x\">tested</a>"), "Build done & tested");
        assert_eq!(plain_text("1 &lt; 2"), "1 < 2");
        assert_eq!(plain_text("no tags"), "no tags");
        assert_eq!(plain_text("<img src=\"a.png\" alt=\"pic\"/>"), "");
    }

    #[test]
    fn actions_in_pairs() {
        let a: Vec<String> = ["default", "Open", "reply", "Reply", "odd"].iter().map(|s| s.to_string()).collect();
        assert_eq!(action_pairs(&a), vec![("default".into(), "Open".into()), ("reply".into(), "Reply".into())]);
        assert!(action_pairs(&[]).is_empty());
    }

    #[test]
    fn icon_order() {
        assert_eq!(
            icon_sources("firefox", Some("file:///tmp/a.png"), Some("firefox_firefox.desktop")),
            vec![
                IconSource::File("/tmp/a.png".into()),
                IconSource::Name("firefox".into()),
                IconSource::DesktopEntry("firefox_firefox".into())
            ]
        );
        assert_eq!(icon_sources("/usr/share/icons/x.png", None, None), vec![IconSource::File("/usr/share/icons/x.png".into())]);
        assert!(icon_sources(" ", None, Some("")).is_empty());
    }

    #[test]
    fn notification_settings() {
        let p = NotifyPrefs { dnd: true, banner_ms: 10000, muted: vec!["Spotify".into()], seen: vec!["Firefox".into(), "Spotify".into()] };
        assert_eq!(NotifyPrefs::parse(&p.to_text()), p);
        assert_eq!(NotifyPrefs::parse(""), NotifyPrefs::default());
        assert_eq!(NotifyPrefs::parse("banner-ms=1234").banner_ms, DEFAULT_MS, "not a choice");
        assert!(!p.banner_for("Firefox", false), "Do Not Disturb");
        assert!(p.banner_for("Firefox", true), "critical ones still show");
        assert!(!p.banner_for("Spotify", true), "turned off: not even critical");
        let quiet = NotifyPrefs { dnd: false, ..p };
        assert!(quiet.banner_for("Firefox", false));
        // A name with the separator can't break the file.
        let odd = NotifyPrefs { muted: vec!["a;b".into(), "c".into()], ..NotifyPrefs::default() };
        assert_eq!(NotifyPrefs::parse(&odd.to_text()).muted, ["c"]);
    }

    #[test]
    fn center_list() {
        let e = |id: u32| Entry { id, app: "x".into(), summary: String::new(), body: String::new(), time: 0, clickable: false, icon: false };
        let mut list = Vec::new();
        for id in 1..=CENTER_MAX as u32 + 2 {
            add_entry(&mut list, e(id));
        }
        assert_eq!(list.len(), CENTER_MAX);
        assert_eq!(list[0].id, CENTER_MAX as u32 + 2, "newest first");
        // A replaced one moves to the top without a duplicate.
        add_entry(&mut list, e(10));
        assert_eq!(list[0].id, 10);
        assert_eq!(list.iter().filter(|x| x.id == 10).count(), 1);
        // Full: adding one drops the oldest, whose id comes back (its picture is removed).
        let oldest = list.last().unwrap().id;
        assert_eq!(add_entry(&mut list, e(500)), vec![oldest]);
        assert_eq!(list.len(), CENTER_MAX);
    }

    #[test]
    fn switch_and_reasons() {
        assert!(enabled_from_env(None));
        assert!(!enabled_from_env(Some("off")));
        assert_eq!(Reason::from_u32(2), Some(Reason::Dismissed));
        assert_eq!(Reason::from_u32(4), None);
    }
}
