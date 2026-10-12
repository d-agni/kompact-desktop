//! 6m-d: the dock, macOS style: Launchpad, one icon per app (pinned, then other running apps),
//! then Trash, along the bottom. Which apps are pinned, and what a click on an app does, are pure
//! and tested here; the page draws it from `dock_items` and kompact's window list (6m-c).
//!
//! Pinned apps come from `~/.config/kompact/dock` (one desktop id per line, `#` comments) or,
//! without that file, a default set of what's installed. The right-click menu's "Keep in Dock"
//! and dragging icons around write that file.
//!
//! 6m-d2: the surface is taller than the space it reserves (`DOCK_SURFACE_HEIGHT` vs
//! `DOCK_HEIGHT`), so icons can grow under the pointer and window previews fit above them; the
//! page sets the input region to what it shows (`dock_surface`), so the see-through part doesn't
//! take clicks. It hides (reserving nothing) while the focused window is maximized or full
//! screen, and comes back when the pointer touches the bottom edge (`~/.config/kompact/dock-prefs`).

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tauri::{AppHandle, Emitter, State};
use tracing::{info, warn};

use crate::compositor::Compositor;
use crate::overlay::{AppEntry, Overlay};

/// Height in logical pixels reserved at the bottom (exclusive zone) while the dock shows, like
/// the macOS dock when it isn't hidden; `dock.css` lays the bar out in it. This is for the
/// default size; 10d-1b: `DockPrefs::zone` for the one set.
pub const DOCK_HEIGHT: i32 = 70;

/// 10d-1b: icon size at rest (px), and the most an icon grows under the pointer (percent).
/// Capped so magnified icons, labels and window previews still fit in the surface.
pub const SIZE_RANGE: (u32, u32) = (32, 64);
pub const ZOOM_RANGE: (u32, u32) = (110, 200);

/// The whole surface: the bar plus room above it for magnified icons, labels and window
/// previews. Only the input region (set by the page) takes the pointer.
pub const DOCK_SURFACE_HEIGHT: i32 = 250;

/// 10d-1c: on the left or right edge the surface is this wide: the bar, room for magnified
/// icons and labels, and the windows of an app stacked beside it.
pub const DOCK_SURFACE_SIDE: i32 = 320;

/// `kompact-icon://` ids of the Trash icons (looked up with the apps, `overlay.rs`).
pub const TRASH_ICON: &str = "@trash";
pub const TRASH_FULL_ICON: &str = "@trash-full";

/// Default pinned apps: for each slot the first one installed.
const DEFAULTS: &[&[&str]] = &[
    &["firefox_firefox.desktop", "firefox.desktop", "org.mozilla.firefox.desktop"],
    &["org.gnome.Nautilus.desktop"],
    &["org.gnome.Ptyxis.desktop", "org.gnome.Terminal.desktop", "foot.desktop"],
    &["org.gnome.TextEditor.desktop", "org.gnome.gedit.desktop"],
    &["org.gnome.Calculator.desktop"],
    &["snap-store_snap-store.desktop", "org.gnome.Software.desktop"],
];

/// The pinned desktop ids, in order: from the dock file if there is one, else the defaults;
/// only installed apps, each once.
pub fn pinned(file: Option<&str>, installed: &[&str]) -> Vec<String> {
    let is_installed = |id: &str| installed.contains(&id);
    let mut out: Vec<String> = Vec::new();
    let mut add = |id: &str| {
        if is_installed(id) && !out.iter().any(|o| o == id) {
            out.push(id.to_string());
        }
    };
    match file {
        Some(text) => {
            for line in text.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('#')) {
                add(line);
            }
        }
        None => {
            for slot in DEFAULTS {
                if let Some(id) = slot.iter().find(|id| is_installed(id)) {
                    add(id);
                }
            }
        }
    }
    out
}

/// One window, as the dock needs it (from a `kompact-windows` message).
#[derive(Debug, Clone, PartialEq)]
pub struct DockWindow {
    pub id: u64,
    pub desktop_id: Option<String>,
    pub workspace: u64,
    pub minimized: bool,
    pub focused: bool,
}

/// Active workspace and windows (bottom-to-top, then parked and minimized) of a message.
pub fn windows_of(msg: &Value) -> (u64, Vec<DockWindow>) {
    let active = msg.get("workspace").and_then(Value::as_u64).unwrap_or(1);
    let list = msg.get("windows").and_then(Value::as_array).map(Vec::as_slice).unwrap_or(&[]);
    let windows = list
        .iter()
        .filter_map(|w| {
            Some(DockWindow {
                id: w.get("id")?.as_u64()?,
                desktop_id: w.get("desktop_id").and_then(Value::as_str).map(str::to_string),
                workspace: w.get("workspace").and_then(Value::as_u64).unwrap_or(active),
                minimized: w.get("minimized").and_then(Value::as_bool).unwrap_or(false),
                focused: w.get("focused").and_then(Value::as_bool).unwrap_or(false),
            })
        })
        .collect();
    (active, windows)
}

/// `list` with `id` pinned (added at the end) or unpinned.
pub fn with_pin(list: &[String], id: &str, pin: bool) -> Vec<String> {
    let mut out: Vec<String> = list.iter().filter(|p| p.as_str() != id).cloned().collect();
    if pin {
        out.push(id.to_string());
    }
    out
}

/// The dock file's text for a pinned list.
pub fn dock_file_text(list: &[String]) -> String {
    let mut text = String::from("# kompact dock: pinned apps, one desktop id per line (written by the dock's \"Keep in Dock\")\n");
    for id in list {
        text.push_str(id);
        text.push('\n');
    }
    text
}

#[derive(Debug, PartialEq, Eq)]
pub enum Click {
    /// Not running: start it.
    Launch,
    /// Bring this window forward (kompact switches workspace / restores it as needed).
    Activate(u64),
    /// 10d-1: the app is in front: minimize its focused window (as the Windows taskbar and
    /// Ubuntu's dock do); the next click brings it back.
    Minimize(u64),
}

/// What clicking app `desktop_id` in the dock does: not running → launch; in front → minimize
/// its focused window; else its top-most window here, else one on another workspace, else its
/// most recently minimized one. (The scroll wheel goes round its windows, `cycle_target`.)
pub fn click_target(active: u64, windows: &[DockWindow], desktop_id: &str) -> Click {
    let mine: Vec<&DockWindow> = windows.iter().filter(|w| w.desktop_id.as_deref() == Some(desktop_id)).collect();
    if mine.is_empty() {
        return Click::Launch;
    }
    let here: Vec<&&DockWindow> = mine.iter().filter(|w| w.workspace == active && !w.minimized).collect();
    if let Some(w) = here.iter().find(|w| w.focused) {
        return Click::Minimize(w.id);
    }
    if let Some(w) = here.last() {
        return Click::Activate(w.id);
    }
    if let Some(w) = mine.iter().find(|w| !w.minimized) {
        return Click::Activate(w.id);
    }
    Click::Activate(mine.last().map(|w| w.id).unwrap_or(0))
}

/// Whether the focused window (on the active workspace, not minimized) is maximized or full
/// screen, from a `windows` message: what makes the dock hide (with `Hide::Smart`).
/// 5j-2: and on the dock's monitor (`dock_output`); one filling another monitor leaves the dock
/// alone. Without those fields (an older kompact) any monitor counts, as before.
pub fn focused_fills(msg: &Value) -> bool {
    let active = msg.get("workspace").and_then(Value::as_u64).unwrap_or(1);
    let dock = msg.get("dock_output").and_then(Value::as_str);
    let flag = |w: &Value, k: &str| w.get(k).and_then(Value::as_bool).unwrap_or(false);
    msg.get("windows").and_then(Value::as_array).is_some_and(|list| {
        list.iter().any(|w| {
            flag(w, "focused")
                && !flag(w, "minimized")
                && w.get("workspace").and_then(Value::as_u64).unwrap_or(active) == active
                && (flag(w, "maximized") || flag(w, "fullscreen"))
                && match (dock, w.get("output").and_then(Value::as_str)) {
                    (Some(dock), Some(on)) => dock == on,
                    _ => true,
                }
        })
    })
}

/// Scrolling over an app's icon: the next (or previous) of its windows on this workspace, in a
/// fixed order (by id, so bringing one forward doesn't reshuffle the cycle). From none focused,
/// the first (or last). `None` when there's nothing else to go to.
pub fn cycle_target(active: u64, windows: &[DockWindow], desktop_id: &str, forward: bool) -> Option<u64> {
    let mut ids: Vec<(u64, bool)> = windows
        .iter()
        .filter(|w| w.desktop_id.as_deref() == Some(desktop_id) && w.workspace == active && !w.minimized)
        .map(|w| (w.id, w.focused))
        .collect();
    ids.sort();
    let n = ids.len();
    let next = match ids.iter().position(|(_, focused)| *focused) {
        Some(_) if n < 2 => return None,
        Some(i) if forward => (i + 1) % n,
        Some(i) => (i + n - 1) % n,
        None if n == 0 => return None,
        None if forward => 0,
        None => n - 1,
    };
    Some(ids[next].0)
}

/// The pinned list after dragging icons around: `order` (what the page shows), keeping only
/// ids that were pinned, each once; pins the page didn't show stay, at the end.
pub fn with_order(pinned: &[String], order: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for id in order {
        if pinned.contains(id) && !out.contains(id) {
            out.push(id.clone());
        }
    }
    for id in pinned {
        if !out.contains(id) {
            out.push(id.clone());
        }
    }
    out
}

/// When the dock gets out of the way.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Hide {
    /// While the focused window is maximized or full screen (the default, as asked for).
    Smart,
    /// Always shown, always reserving its space.
    Never,
    /// Always hidden until the pointer reaches the bottom edge (macOS "Automatically hide").
    Always,
}

/// 10d-1c: which screen edge the dock is on (macOS "Position on screen").
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Side {
    Left,
    #[default]
    Bottom,
    Right,
}

impl Side {
    pub fn name(self) -> &'static str {
        match self {
            Side::Left => "left",
            Side::Bottom => "bottom",
            Side::Right => "right",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct DockPrefs {
    pub hide: Hide,
    /// Icons grow under the pointer.
    pub magnify: bool,
    /// 10d-1: the dock follows the pointer held at another monitor's bottom edge (kompact
    /// does that, 5j-3; told with `dock-follow on|off`).
    pub follow: bool,
    /// 10d-1b: icon size at rest in px (`SIZE_RANGE`).
    pub size: u32,
    /// 10d-1b: how big the icon under the pointer gets, in percent of `size` (`ZOOM_RANGE`).
    pub zoom: u32,
    /// 10d-1b: dots under running apps.
    pub dots: bool,
    /// 10d-1b: an app's icon bounces while it starts.
    pub bounce: bool,
    /// 10d-1c: the edge it's on.
    pub position: Side,
}

impl Default for DockPrefs {
    fn default() -> Self {
        DockPrefs { hide: Hide::Smart, magnify: true, follow: true, size: 50, zoom: 150, dots: true, bounce: true, position: Side::Bottom }
    }
}

impl DockPrefs {
    /// Sizes brought into range (a hand-edited file, an old page).
    pub fn clamped(self) -> Self {
        DockPrefs {
            size: self.size.clamp(SIZE_RANGE.0, SIZE_RANGE.1),
            zoom: self.zoom.clamp(ZOOM_RANGE.0, ZOOM_RANGE.1),
            ..self
        }
    }

    /// The space reserved at the bottom: the bar (icon + 14 px) and 6 px under and over it,
    /// as `dock.css` lays it out (70 px at the default 50 px icons).
    pub fn zone(&self) -> i32 {
        self.clamped().size as i32 + 20
    }
}

/// `~/.config/kompact/dock-prefs`: `key=value` lines (`hide=smart|never|always`,
/// `magnify=on|off`, `follow=on|off`, `size=32…64`, `zoom=110…200`, `dots=on|off`,
/// `bounce=on|off`); anything else keeps its default.
pub fn parse_prefs(text: &str) -> DockPrefs {
    let mut p = DockPrefs::default();
    for line in text.lines().map(str::trim).filter(|l| !l.starts_with('#')) {
        let Some((k, v)) = line.split_once('=') else { continue };
        match (k.trim(), v.trim()) {
            ("hide", "smart") => p.hide = Hide::Smart,
            ("hide", "never") => p.hide = Hide::Never,
            ("hide", "always") => p.hide = Hide::Always,
            ("magnify", "on") => p.magnify = true,
            ("magnify", "off") => p.magnify = false,
            ("follow", "on") => p.follow = true,
            ("follow", "off") => p.follow = false,
            ("dots", "on") => p.dots = true,
            ("dots", "off") => p.dots = false,
            ("bounce", "on") => p.bounce = true,
            ("bounce", "off") => p.bounce = false,
            ("position", "left") => p.position = Side::Left,
            ("position", "bottom") => p.position = Side::Bottom,
            ("position", "right") => p.position = Side::Right,
            ("size", n) => p.size = n.parse().unwrap_or(p.size),
            ("zoom", n) => p.zoom = n.parse().unwrap_or(p.zoom),
            _ => {}
        }
    }
    p.clamped()
}

pub fn prefs_text(p: DockPrefs) -> String {
    let hide = match p.hide {
        Hide::Smart => "smart",
        Hide::Never => "never",
        Hide::Always => "always",
    };
    format!(
        "# kompact dock settings (written by System Settings → Dock and the dock's right-click menu)\n\
         hide={hide}\nmagnify={}\nfollow={}\nsize={}\nzoom={}\ndots={}\nbounce={}\nposition={}\n",
        on_off(p.magnify),
        on_off(p.follow),
        p.size,
        p.zoom,
        on_off(p.dots),
        on_off(p.bounce),
        p.position.name()
    )
}

fn on_off(b: bool) -> &'static str {
    if b { "on" } else { "off" }
}

/// 10d-1: the line telling kompact whether the dock follows the pointer.
pub fn follow_line(p: DockPrefs) -> String {
    format!("dock-follow {}\n", on_off(p.follow))
}

/// 10d-1c: the line telling kompact which edge the dock is on (it moves it to another monitor
/// when the pointer is held at that edge there).
pub fn side_line(p: DockPrefs) -> String {
    format!("dock-side {}\n", p.position.name())
}

#[derive(Serialize)]
pub struct DockItems {
    pinned: Vec<String>,
    apps: Vec<AppEntry>,
}

fn config_file(name: &str) -> Option<std::path::PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(std::path::PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join(".config")))?;
    Some(base.join("kompact").join(name))
}

fn dock_file() -> Option<std::path::PathBuf> {
    config_file("dock")
}

fn write_config(path: Option<std::path::PathBuf>, text: &str) -> Result<std::path::PathBuf, String> {
    let path = path.ok_or("no home directory")?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    std::fs::write(&path, text).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(path)
}

fn current_pinned(apps: &[AppEntry]) -> (Vec<String>, bool) {
    let file = dock_file().and_then(|p| std::fs::read_to_string(p).ok());
    let installed: Vec<&str> = apps.iter().map(|a| a.id.as_str()).collect();
    (pinned(file.as_deref(), &installed), file.is_some())
}

/// The installed apps (with icons) and which are pinned, for the dock page.
#[tauri::command]
pub fn dock_items(overlay: State<'_, Overlay>) -> DockItems {
    let apps = overlay.refresh_apps();
    let (pinned, from_file) = current_pinned(&apps);
    info!("dock: {} pinned ({})", pinned.len(), if from_file { "~/.config/kompact/dock" } else { "defaults" });
    DockItems { pinned, apps }
}

/// Right-click on a dock icon: open its menu (in the overlay) above it, at screen x `x`.
#[tauri::command]
pub fn show_dock_menu(app: AppHandle, overlay: State<'_, Overlay>, desktop_id: String, x: f64, y: Option<f64>) {
    info!("dock: menu for {desktop_id}");
    overlay.open_dock_menu(&app, desktop_id, x, y.unwrap_or(0.0));
}

#[derive(Serialize)]
pub struct MenuWindow {
    id: u64,
    title: String,
    minimized: bool,
    focused: bool,
}

/// What the dock menu shows.
#[derive(Serialize)]
pub struct DockMenu {
    desktop_id: String,
    name: String,
    /// Screen x of the icon's centre; the menu is centred above it.
    x: f64,
    /// 10d-1c: screen y of the icon's centre (a dock on a side: the menu is beside it), the
    /// edge the dock is on, and how much it reserves there.
    y: f64,
    side: Side,
    zone: i32,
    pinned: bool,
    windows: Vec<MenuWindow>,
    /// For the dock's own menu (`desktop_id` "@dock": right-click between the icons).
    prefs: DockPrefs,
}

#[tauri::command]
pub fn dock_menu_info(overlay: State<'_, Overlay>, compositor: State<'_, Compositor>) -> Option<DockMenu> {
    let (desktop_id, x, y) = overlay.dock_menu_target()?;
    let apps = overlay.refresh_apps();
    let (pinned, _) = current_pinned(&apps);
    let msg = compositor.last_state();
    let windows = msg
        .as_ref()
        .and_then(|m| m.get("windows"))
        .and_then(Value::as_array)
        .map(|list| {
            list.iter()
                .filter(|w| w.get("desktop_id").and_then(Value::as_str) == Some(desktop_id.as_str()))
                .filter_map(|w| {
                    Some(MenuWindow {
                        id: w.get("id")?.as_u64()?,
                        title: w.get("title").and_then(Value::as_str).unwrap_or("").to_string(),
                        minimized: w.get("minimized").and_then(Value::as_bool).unwrap_or(false),
                        focused: w.get("focused").and_then(Value::as_bool).unwrap_or(false),
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    let name = match desktop_id.as_str() {
        "@dock" => "Dock".to_string(),
        id => overlay.app_entry(id).map(|a| a.name).unwrap_or_else(|| desktop_id.clone()),
    };
    let prefs = dock_prefs();
    Some(DockMenu { pinned: pinned.contains(&desktop_id), desktop_id, name, x, y, side: prefs.position, zone: prefs.zone(), windows, prefs })
}

/// "Keep in Dock" on or off: rewrite `~/.config/kompact/dock` and tell the dock.
#[tauri::command]
pub fn set_pinned(app: AppHandle, overlay: State<'_, Overlay>, desktop_id: String, pinned: bool) -> Result<(), String> {
    let apps = overlay.refresh_apps();
    let (list, _) = current_pinned(&apps);
    let list = with_pin(&list, &desktop_id, pinned);
    let path = write_config(dock_file(), &dock_file_text(&list))?;
    info!("dock: {} {desktop_id}; {} pinned, saved to {}", if pinned { "pinned" } else { "unpinned" }, list.len(), path.display());
    let _ = app.emit("kompact-dock-changed", ());
    Ok(())
}

/// Icons dragged into a new order (6m-d2).
#[tauri::command]
pub fn set_pin_order(app: AppHandle, overlay: State<'_, Overlay>, order: Vec<String>) -> Result<(), String> {
    let apps = overlay.refresh_apps();
    let (list, _) = current_pinned(&apps);
    let list = with_order(&list, &order);
    let path = write_config(dock_file(), &dock_file_text(&list))?;
    info!("dock: new order of {} pinned, saved to {}", list.len(), path.display());
    let _ = app.emit("kompact-dock-changed", ());
    Ok(())
}

#[tauri::command]
pub fn dock_prefs() -> DockPrefs {
    config_file("dock-prefs").and_then(|p| std::fs::read_to_string(p).ok()).map(|t| parse_prefs(&t)).unwrap_or_default()
}

#[tauri::command]
pub fn set_dock_prefs(app: AppHandle, c: State<'_, Compositor>, prefs: DockPrefs) -> Result<(), String> {
    let prefs = prefs.clamped();
    let before = dock_prefs();
    let path = write_config(config_file("dock-prefs"), &prefs_text(prefs))?;
    if prefs.position != before.position {
        // 10d-1c: onto its new edge now (commands run on the GTK main thread).
        if let Err(e) = crate::place_dock(&app, prefs.position, prefs.zone()) {
            warn!("dock: can't move it to the {} edge: {e}", prefs.position.name());
        }
    }
    info!("dock: settings {prefs:?}, saved to {}", path.display());
    if let Err(e) = c.send_dock_follow(prefs) {
        info!("dock: kompact not told about following the pointer ({e}); it is when it connects");
    }
    if prefs.position != before.position {
        let _ = c.send_dock_side(prefs);
    }
    let _ = app.emit("kompact-dock-changed", ());
    Ok(())
}

/// The page's layout changed: reserve the bottom `zone` px (shown; the page works it out from
/// the size, `DOCK_HEIGHT` if it doesn't say) or nothing (hidden), and take the pointer only
/// inside `rects` (`[x, y, w, h]` in surface pixels).
#[tauri::command]
pub fn dock_surface(app: AppHandle, reserve: bool, rects: Vec<[f64; 4]>, zone: Option<i32>) -> Result<(), String> {
    use gtk::prelude::*;
    use gtk_layer_shell::LayerShell;
    use tauri::Manager;
    let window = app.get_webview_window("dock").ok_or("no dock window")?;
    let gtk_window = window.gtk_window().map_err(|e| e.to_string())?;
    let shown = zone.unwrap_or(DOCK_HEIGHT).clamp(SIZE_RANGE.0 as i32 + 20, SIZE_RANGE.1 as i32 + 20);
    let zone = if reserve { shown } else { 0 };
    if gtk_window.exclusive_zone() != zone {
        info!("dock: {} (exclusive zone {zone})", if reserve { "shown" } else { "hidden" });
        gtk_window.set_exclusive_zone(zone);
    }
    let region = gtk::cairo::Region::create();
    for [x, y, w, h] in &rects {
        let r = gtk::cairo::RectangleInt::new(x.floor() as i32, y.floor() as i32, w.ceil() as i32, h.ceil() as i32);
        region.union_rectangle(&r).map_err(|e| e.to_string())?;
    }
    gtk_window.input_shape_combine_region(Some(&region));
    tracing::debug!("dock: input region {rects:?}");
    Ok(())
}

/// Whether the Trash has anything in it (`$XDG_DATA_HOME/Trash/files`).
#[tauri::command]
pub fn dock_trash_full() -> bool {
    let base = std::env::var_os("XDG_DATA_HOME")
        .map(std::path::PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join(".local/share")));
    base.and_then(|b| std::fs::read_dir(b.join("Trash/files")).ok()).is_some_and(|mut d| d.next().is_some())
}

/// Open the Trash in the file manager.
#[tauri::command]
pub fn dock_open_trash() -> Result<(), String> {
    let mut child = std::process::Command::new("gio")
        .args(["open", "trash:///"])
        .spawn()
        .map_err(|e| format!("gio open trash:///: {e}"))?;
    std::thread::spawn(move || child.wait());
    info!("dock: opened the Trash");
    Ok(())
}

/// "Empty Trash" (the Trash's right-click menu).
#[tauri::command]
pub fn dock_empty_trash() -> Result<(), String> {
    let mut child = std::process::Command::new("gio")
        .args(["trash", "--empty"])
        .spawn()
        .map_err(|e| format!("gio trash --empty: {e}"))?;
    std::thread::spawn(move || child.wait());
    info!("dock: emptied the Trash");
    Ok(())
}

/// Scroll over a running app's icon: bring its next window forward.
#[tauri::command]
pub fn dock_cycle(compositor: State<'_, Compositor>, desktop_id: String, forward: bool) -> Result<(), String> {
    let (active, windows) = compositor.last_state().map(|m| windows_of(&m)).unwrap_or((1, Vec::new()));
    match cycle_target(active, &windows, &desktop_id, forward) {
        Some(id) => {
            info!("dock: scroll on {desktop_id} -> window {id}");
            compositor.send("activate", Some(id))
        }
        None => Ok(()),
    }
}

/// "Open" / "New Window": start the app (another instance if it's running).
#[tauri::command]
pub fn dock_open(app: AppHandle, overlay: State<'_, Overlay>, desktop_id: String) {
    if !overlay.has_apps() {
        overlay.refresh_apps();
    }
    overlay.launch_id(&app, &desktop_id);
}

/// A click on an app in the dock.
#[tauri::command]
pub fn dock_click(
    app: AppHandle,
    overlay: State<'_, Overlay>,
    compositor: State<'_, Compositor>,
    desktop_id: String,
) -> Result<(), String> {
    let (active, windows) = compositor.last_state().map(|m| windows_of(&m)).unwrap_or((1, Vec::new()));
    let click = click_target(active, &windows, &desktop_id);
    info!("dock: {desktop_id} -> {click:?}");
    match click {
        Click::Launch => {
            if !overlay.has_apps() {
                overlay.refresh_apps();
            }
            overlay.launch_id(&app, &desktop_id);
            Ok(())
        }
        Click::Activate(id) => compositor.send("activate", Some(id)),
        Click::Minimize(id) => compositor.send("minimize", Some(id)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn w(id: u64, app: &str, workspace: u64, minimized: bool, focused: bool) -> DockWindow {
        DockWindow { id, desktop_id: Some(app.into()), workspace, minimized, focused }
    }

    #[test]
    fn default_pins() {
        let installed = ["foot.desktop", "firefox_firefox.desktop", "org.gnome.gedit.desktop", "org.gnome.Ptyxis.desktop", "x.desktop"];
        assert_eq!(pinned(None, &installed), ["firefox_firefox.desktop", "org.gnome.Ptyxis.desktop", "org.gnome.gedit.desktop"]);
    }

    #[test]
    fn pins_from_the_file() {
        let installed = ["a.desktop", "b.desktop"];
        let file = "# my dock\nb.desktop\n\n  a.desktop  \nmissing.desktop\nb.desktop\n";
        assert_eq!(pinned(Some(file), &installed), ["b.desktop", "a.desktop"]);
        assert!(pinned(Some(""), &installed).is_empty(), "an empty file means nothing pinned");
    }

    #[test]
    fn pinning() {
        let list = vec!["a".to_string(), "b".to_string()];
        assert_eq!(with_pin(&list, "c", true), ["a", "b", "c"]);
        assert_eq!(with_pin(&list, "a", true), ["b", "a"], "re-pinning moves it to the end");
        assert_eq!(with_pin(&list, "a", false), ["b"]);
        assert_eq!(with_pin(&list, "z", false), ["a", "b"]);
        let text = dock_file_text(&["x.desktop".into(), "y.desktop".into()]);
        assert_eq!(pinned(Some(&text), &["x.desktop", "y.desktop"]), ["x.desktop", "y.desktop"], "round trip");
    }

    #[test]
    fn click_not_running_launches() {
        assert_eq!(click_target(1, &[w(1, "other", 1, false, true)], "ff"), Click::Launch);
    }

    #[test]
    fn click_brings_the_top_window_here() {
        let ws = [w(1, "ff", 1, false, false), w(2, "ff", 1, false, false), w(3, "term", 1, false, true)];
        assert_eq!(click_target(1, &ws, "ff"), Click::Activate(2));
    }

    #[test]
    fn click_on_the_front_app_minimizes_its_window() {
        assert_eq!(click_target(1, &[w(2, "ff", 1, false, true)], "ff"), Click::Minimize(2));
        // Several: the focused one goes; the next click takes the one that's then in front.
        let ws = [w(1, "ff", 1, false, false), w(2, "ff", 1, false, true)];
        assert_eq!(click_target(1, &ws, "ff"), Click::Minimize(2));
        // Minimized now, nothing else of it in front: the click brings it back.
        assert_eq!(click_target(1, &[w(2, "ff", 1, true, false), w(3, "term", 1, false, true)], "ff"), Click::Activate(2));
        // Another app in front: this one comes forward, nothing is minimized.
        let ws = [w(1, "ff", 1, false, false), w(3, "term", 1, false, true)];
        assert_eq!(click_target(1, &ws, "ff"), Click::Activate(1));
    }

    #[test]
    fn click_finds_windows_elsewhere() {
        // Only on workspace 3: go there.
        assert_eq!(click_target(1, &[w(5, "ff", 3, false, false)], "ff"), Click::Activate(5));
        // Only minimized: the most recent one comes back.
        let ws = [w(6, "ff", 1, true, false), w(7, "ff", 1, true, false)];
        assert_eq!(click_target(1, &ws, "ff"), Click::Activate(7));
        // A normal window elsewhere wins over a minimized one here.
        let ws = [w(6, "ff", 1, true, false), w(8, "ff", 2, false, false)];
        assert_eq!(click_target(1, &ws, "ff"), Click::Activate(8));
    }

    #[test]
    fn hides_for_a_focused_maximized_window_here() {
        let msg = |ws: u64, focused: bool, minimized: bool, maximized: bool, fullscreen: bool| {
            serde_json::json!({"workspace":1,"windows":[
                {"id":1,"workspace":1,"focused":false,"maximized":true},
                {"id":2,"workspace":ws,"focused":focused,"minimized":minimized,"maximized":maximized,"fullscreen":fullscreen}]})
        };
        assert!(focused_fills(&msg(1, true, false, true, false)));
        assert!(focused_fills(&msg(1, true, false, false, true)), "full screen too");
        assert!(!focused_fills(&msg(1, true, false, false, false)), "a maximized window behind doesn't count");
        assert!(!focused_fills(&msg(1, false, false, true, false)));
        assert!(!focused_fills(&msg(2, true, false, true, false)), "another workspace");
        assert!(!focused_fills(&msg(1, true, true, true, false)), "minimized");
        assert!(!focused_fills(&serde_json::json!({"type":"windows"})));
        // 5j-2: only a window on the dock's monitor hides it.
        let on = |output: &str| {
            serde_json::json!({"type":"windows","workspace":1,"dock_output":"VGA-1","windows":[
                {"id":2,"workspace":1,"focused":true,"maximized":true,"output":output}]})
        };
        assert!(focused_fills(&on("VGA-1")));
        assert!(!focused_fills(&on("HDMIA-1")), "maximized on the other monitor");
        let no_output = serde_json::json!({"type":"windows","workspace":1,"dock_output":"VGA-1","windows":[
            {"id":2,"workspace":1,"focused":true,"maximized":true}]});
        assert!(focused_fills(&no_output), "on no monitor: as before");
    }

    #[test]
    fn scrolling_cycles_windows_here() {
        let ws = [w(5, "ff", 1, false, false), w(2, "ff", 1, false, true), w(9, "ff", 1, false, false),
            w(3, "ff", 2, false, false), w(4, "ff", 1, true, false), w(1, "term", 1, false, false)];
        assert_eq!(cycle_target(1, &ws, "ff", true), Some(5));
        assert_eq!(cycle_target(1, &ws, "ff", false), Some(9), "wraps backwards");
        let unfocused = [w(5, "ff", 1, false, false), w(2, "ff", 1, false, false)];
        assert_eq!(cycle_target(1, &unfocused, "ff", true), Some(2));
        assert_eq!(cycle_target(1, &unfocused, "ff", false), Some(5));
        assert_eq!(cycle_target(1, &[w(2, "ff", 1, false, true)], "ff", true), None, "alone and in front");
        assert_eq!(cycle_target(1, &ws, "gedit", true), None);
    }

    #[test]
    fn reordering_keeps_only_pins() {
        let pinned: Vec<String> = ["a", "b", "c"].map(String::from).to_vec();
        let order: Vec<String> = ["c", "x", "a", "c"].map(String::from).to_vec();
        assert_eq!(with_order(&pinned, &order), ["c", "a", "b"]);
    }

    #[test]
    fn prefs_round_trip() {
        assert_eq!(parse_prefs(""), DockPrefs::default());
        assert_eq!(
            parse_prefs("# x\nhide = always\nmagnify=off\nbogus=1\nhide=sometimes"),
            DockPrefs { hide: Hide::Always, magnify: false, ..DockPrefs::default() }
        );
        assert_eq!(parse_prefs("follow=off").follow, false);
        for hide in [Hide::Smart, Hide::Never, Hide::Always] {
            for magnify in [true, false] {
                for follow in [true, false] {
                    let position = if follow { Side::Left } else { Side::Right };
                    let p = DockPrefs { hide, magnify, follow, size: 40, zoom: 180, dots: !follow, bounce: follow, position };
                    assert_eq!(parse_prefs(&prefs_text(p)), p);
                }
            }
        }
        assert_eq!(
            serde_json::to_value(DockPrefs::default()).unwrap(),
            serde_json::json!({"hide":"smart","magnify":true,"follow":true,"size":50,"zoom":150,"dots":true,"bounce":true,"position":"bottom"})
        );
        // An older page sending only two fields keeps the default for the third.
        let p: DockPrefs = serde_json::from_value(serde_json::json!({"hide":"never","magnify":false})).unwrap();
        assert_eq!(p, DockPrefs { hide: Hide::Never, magnify: false, ..DockPrefs::default() });
        assert_eq!(follow_line(p), "dock-follow on\n");
        assert_eq!(follow_line(DockPrefs { follow: false, ..p }), "dock-follow off\n");
        assert_eq!(side_line(p), "dock-side bottom\n");
        assert_eq!(side_line(DockPrefs { position: Side::Left, ..p }), "dock-side left\n");
        assert_eq!(parse_prefs("position=right").position, Side::Right);
        assert_eq!(parse_prefs("position=top").position, Side::Bottom);
    }

    #[test]
    fn sizes_stay_in_range() {
        let p = parse_prefs("size=200\nzoom=50");
        assert_eq!((p.size, p.zoom), (SIZE_RANGE.1, ZOOM_RANGE.0));
        let p = parse_prefs("size=big\nzoom=");
        assert_eq!((p.size, p.zoom), (50, 150));
        assert_eq!(DockPrefs::default().zone(), DOCK_HEIGHT);
        assert_eq!(DockPrefs { size: 64, ..DockPrefs::default() }.zone(), 84);
        assert_eq!(DockPrefs { size: 999, ..DockPrefs::default() }.zone(), 84);
    }

    #[test]
    fn parses_messages() {
        let msg = serde_json::json!({"type":"windows","workspace":2,"windows":[
            {"id":4,"app_id":"gedit","desktop_id":"org.gnome.gedit.desktop","workspace":2,"minimized":true,"focused":false},
            {"id":5,"app_id":"x","desktop_id":null,"workspace":1,"minimized":false,"focused":true},
            {"app_id":"no id"}]});
        let (active, ws) = windows_of(&msg);
        assert_eq!(active, 2);
        assert_eq!(ws, vec![
            DockWindow { id: 4, desktop_id: Some("org.gnome.gedit.desktop".into()), workspace: 2, minimized: true, focused: false },
            DockWindow { id: 5, desktop_id: None, workspace: 1, minimized: false, focused: true },
        ]);
    }
}
