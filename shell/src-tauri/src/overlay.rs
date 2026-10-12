//! The overlay: one full-screen layer surface on the overlay layer that shows, one at a time,
//! the macOS-style pop-ups: Launchpad (6t-c), Spotlight (6m-a) and the logo menu with its About
//! and confirm dialogs (6m-b). One page for all of them, so
//! they share one WebKit web process (about 60 MB each otherwise) and open instantly.
//!
//! Built hidden at start-up; opening shows the layer surface with an exclusive keyboard
//! (kompact focuses it, 6c-1), closing hides it and kompact gives the keyboard back to the
//! window underneath.
//!
//! 5j: it opens on the monitor under the pointer. kompact says which output that is
//! (`pointer_output` on its channel), and the hidden layer surface is moved to the GDK monitor
//! with the same geometry (`pick_monitor`) just before it's shown.
//!
//! The pages talk to the shell with Tauri commands (`window.__TAURI__.core.invoke`). Plain
//! (non-async) commands run on the GTK main thread, so they may touch GTK directly.

use std::collections::HashMap;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::Mutex;

use gtk::prelude::*;
use gtk_layer_shell::{KeyboardMode, LayerShell};
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State};
use tracing::{error, info, warn};

use crate::apps::{self, App};
use crate::config;

pub const LABEL: &str = "overlay";

/// What the overlay shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    Launchpad,
    Spotlight,
    Menu,
    /// A dock icon's right-click menu (6m-d).
    #[serde(rename = "dockmenu")]
    DockMenu,
    /// 6e: all workspaces as miniatures, from the panel's workspace icon.
    Spaces,
    /// 9c: drag out a screenshot region (Shift+Print).
    Select,
    /// 12a: a menu-bar status item's menu (network; later Bluetooth, sound, Control Center),
    /// under the item (`open_status`).
    Status,
    /// 12e: polkit's password dialog (`polkit.rs`).
    Auth,
}

/// 12a: the menu-bar items that open a status menu.
pub const STATUS_ITEMS: [&str; 6] = ["network", "bluetooth", "sound", "center", "control", "input"];

impl Mode {
    pub fn parse(s: &str) -> Option<Mode> {
        match s {
            "launchpad" => Some(Mode::Launchpad),
            "spotlight" => Some(Mode::Spotlight),
            "menu" => Some(Mode::Menu),
            "dockmenu" => Some(Mode::DockMenu),
            "spaces" => Some(Mode::Spaces),
            "select" => Some(Mode::Select),
            "status" => Some(Mode::Status),
            "auth" => Some(Mode::Auth),
            _ => None,
        }
    }

    fn name(self) -> &'static str {
        match self {
            Mode::Launchpad => "launchpad",
            Mode::Spotlight => "spotlight",
            Mode::Menu => "menu",
            Mode::DockMenu => "dockmenu",
            Mode::Spaces => "spaces",
            Mode::Select => "select",
            Mode::Status => "status",
            Mode::Auth => "auth",
        }
    }
}

/// What `toggle_overlay(mode)` does, given what is showing now.
#[derive(Debug, PartialEq, Eq)]
enum Toggle {
    Open,
    Switch,
    Close,
}

fn toggle(showing: Option<Mode>, wanted: Mode) -> Toggle {
    match showing {
        None => Toggle::Open,
        Some(m) if m == wanted => Toggle::Close,
        Some(_) => Toggle::Switch,
    }
}

/// 5j: which of these monitors (x, y, w, h) is the output the pointer is on: the one with the
/// same geometry, else the one containing that output's centre (`None`: neither, stay put).
pub fn pick_monitor(monitors: &[(i32, i32, i32, i32)], pointer: (i32, i32, i32, i32)) -> Option<usize> {
    if let Some(i) = monitors.iter().position(|&m| m == pointer) {
        return Some(i);
    }
    let (cx, cy) = (pointer.0 + pointer.2 / 2, pointer.1 + pointer.3 / 2);
    monitors.iter().position(|&(x, y, w, h)| cx >= x && cy >= y && cx < x + w && cy < y + h)
}

/// 5j: put the (hidden) overlay on the monitor `pointer` names. Returns the monitor's geometry.
fn place_on(gtk_window: &gtk::ApplicationWindow, pointer: (i32, i32, i32, i32)) -> Option<(i32, i32, i32, i32)> {
    let display = gtk_window.display();
    let monitors: Vec<gtk::gdk::Monitor> = (0..display.n_monitors()).filter_map(|i| display.monitor(i)).collect();
    let geometry: Vec<(i32, i32, i32, i32)> = monitors
        .iter()
        .map(|m| {
            let g = m.geometry();
            (g.x(), g.y(), g.width(), g.height())
        })
        .collect();
    let i = pick_monitor(&geometry, pointer)?;
    gtk_window.set_monitor(&monitors[i]);
    Some(geometry[i])
}

/// 9c: a region the page chose (its own px) in global logical coordinates, given the monitor
/// the overlay is on (x, y, w, h; `None`: not known, taken as at 0,0).
pub fn global_region(on: Option<(i32, i32, i32, i32)>, x: i32, y: i32, w: i32, h: i32) -> (i32, i32, i32, i32) {
    let (ox, oy) = on.map_or((0, 0), |m| (m.0, m.1));
    (ox + x, oy + y, w, h)
}

/// Shared by the commands and the icon scheme handler.
pub struct Overlay {
    inner: Mutex<Inner>,
    /// Environment variables the shell set for itself; launched apps don't inherit them.
    our_env: Vec<&'static str>,
}

#[derive(Default)]
struct Inner {
    /// What is showing; `None` = hidden.
    open: Option<Mode>,
    /// The page has loaded and asked whether it should show itself (`overlay_ready`).
    ready: bool,
    /// What the page was last given, to look up the app `launch` names.
    apps: Vec<App>,
    /// 5j: the output under the pointer (x, y, w, h), from kompact.
    pointer_output: Option<(i32, i32, i32, i32)>,
    /// 5j: the monitor geometry the overlay was last put on.
    on: Option<(i32, i32, i32, i32)>,
    /// Desktop id → icon file, for `kompact-icon://`.
    icons: HashMap<String, PathBuf>,
    /// The dock icon a `dockmenu` is for, and where (screen x of the icon's centre).
    dock_menu: Option<(String, f64, f64)>,
    /// 12a: which status menu, and the screen x of its item's right edge.
    status: Option<(String, f64)>,
    /// 13b: the window is mapped. After a close it stays mapped (views cleared) until the page
    /// has drawn that empty frame, so the next open doesn't show the old view's last frame.
    mapped: bool,
    /// 13b: counts closes; a late "cleared" from an earlier close hides nothing.
    hide_gen: u64,
}

/// One app as the overlay and dock pages see it.
#[derive(Serialize, Clone)]
pub struct AppEntry {
    pub id: String,
    name: String,
    comment: String,
    keywords: String,
    /// Whether `kompact-icon://localhost/<id>` has an icon; otherwise the page draws a badge.
    icon: bool,
}

impl Overlay {
    pub fn new(our_env: Vec<&'static str>) -> Overlay {
        Overlay { inner: Mutex::new(Inner::default()), our_env }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// The icon file for a desktop id, if `list_apps` found one.
    pub fn icon_path(&self, id: &str) -> Option<PathBuf> {
        self.lock().icons.get(id).cloned()
    }

    /// 8c: serve `path` as `kompact-icon://localhost/<key>` (a notification's picture).
    pub fn add_icon(&self, key: String, path: PathBuf) {
        self.lock().icons.insert(key, path);
    }

    pub fn remove_icon(&self, key: &str) {
        self.lock().icons.remove(key);
    }

    /// 9c: open `path` with its default app (`xdg-open`), like a launch from the dock.
    pub fn open_path(&self, handle: &AppHandle, path: PathBuf) {
        let (handle, our_env) = (handle.clone(), self.our_env.clone());
        std::thread::spawn(move || {
            let token = handle.try_state::<crate::compositor::Compositor>().and_then(|c| c.activation_token(TOKEN_WAIT));
            let argv = vec!["xdg-open".to_string(), path.to_string_lossy().into_owned()];
            spawn_now("xdg-open", &argv, &our_env, token);
        });
    }

    /// 5j: kompact says the pointer is now on this output.
    pub fn set_pointer_output(&self, output: (i32, i32, i32, i32)) {
        self.lock().pointer_output = Some(output);
    }

    fn open(&self, app: &AppHandle, mode: Mode, why: &str) {
        let Some((window, gtk_window)) = windows(app) else { return };
        let (was_mapped, ready) = {
            let mut inner = self.lock();
            if inner.open == Some(mode) {
                return;
            }
            inner.open = Some(mode);
            // 13b: a close still waiting for its empty frame: the window is up, keep it.
            (std::mem::replace(&mut inner.mapped, true), inner.ready)
        };
        if !was_mapped {
            // 5j: hidden, so moving it is only a matter of where it's mapped next.
            let pointer = self.lock().pointer_output;
            if let Some(pointer) = pointer {
                match place_on(&gtk_window, pointer) {
                    Some(m) => {
                        let moved = self.lock().on.replace(m) != Some(m);
                        if moved {
                            info!("overlay: on the monitor at {},{} {}x{} (under the pointer)", m.0, m.1, m.2, m.3);
                        }
                    }
                    None => warn!("overlay: no monitor matches the pointer's output {pointer:?}; it opens where it was"),
                }
            }
        }
        // The page switches to `mode` first, then the window shows: shown first, its first frame
        // was the view from last time (a launcher flashing before the logo menu).
        if ready {
            // Otherwise the page opens itself when it's ready (`overlay_ready`).
            let script = format!("window.kompactOverlay && window.kompactOverlay.open({:?})", mode.name());
            if let Err(e) = window.eval(&script) {
                warn!("overlay: script failed: {e}");
            }
        }
        // Exclusive: kompact gives us the keyboard while the overlay is up (6c-1).
        gtk_window.set_keyboard_mode(KeyboardMode::Exclusive);
        if !was_mapped {
            if let Err(e) = window.show() {
                warn!("overlay: can't show: {e}");
            }
        }
        let _ = window.set_focus();
        info!("overlay: {} opened ({why})", mode.name());
    }

    /// Open `mode`, switch to it, or close it if it's what's showing.
    pub fn toggle(&self, app: &AppHandle, mode: Mode, why: &str) {
        let showing = self.lock().open;
        match toggle(showing, mode) {
            Toggle::Close => self.close(app, why),
            Toggle::Open | Toggle::Switch => self.open(app, mode, why),
        }
    }

    pub fn close(&self, app: &AppHandle, why: &str) {
        let (mode, gen) = {
            let mut inner = self.lock();
            let Some(mode) = inner.open.take() else { return };
            inner.hide_gen += 1;
            (mode, inner.hide_gen)
        };
        let Some((window, gtk_window)) = windows(app) else { return };
        // Give up the keyboard first, so kompact hands focus back to the window underneath
        // whether hiding destroys or only unmaps the layer surface.
        gtk_window.set_keyboard_mode(KeyboardMode::None);
        // 13b: WebKit shows its last drawn frame again when the window maps next, so the page
        // hides its views and says when it has drawn that (`overlay_cleared`), and the window
        // hides then; at most `CLEAR_WAIT` later if the page doesn't answer.
        let script = format!("window.kompactOverlay ? window.kompactOverlay.clear({gen}) : window.__TAURI__.core.invoke('overlay_cleared', {{gen: {gen}}})");
        if let Err(e) = window.eval(&script) {
            warn!("overlay: script failed: {e}");
            self.finish_hide(app, gen);
        } else {
            let app = app.clone();
            std::thread::spawn(move || {
                std::thread::sleep(CLEAR_WAIT);
                app.state::<Overlay>().finish_hide(&app, gen);
            });
        }
        info!("overlay: {} closed ({why})", mode.name());
        // The dock stays up while its menu shows (6m-d2).
        let _ = app.emit("kompact-overlay-closed", mode.name());
    }
}

/// 13b: longest wait for the page's empty frame before the window hides anyway.
const CLEAR_WAIT: std::time::Duration = std::time::Duration::from_millis(200);

impl Overlay {
    /// 13b: hide the window after close number `gen`, unless it opened again meanwhile.
    fn finish_hide(&self, app: &AppHandle, gen: u64) {
        {
            let mut inner = self.lock();
            if inner.open.is_some() || inner.hide_gen != gen || !inner.mapped {
                return;
            }
            inner.mapped = false;
        }
        let Some(window) = app.get_webview_window(LABEL) else { return };
        if let Err(e) = window.hide() {
            warn!("overlay: can't hide: {e}");
        }
    }
}

/// 13b: the page has drawn its empty frame after close number `gen`.
#[tauri::command]
pub fn overlay_cleared(app: AppHandle, overlay: State<'_, Overlay>, gen: u64) {
    overlay.finish_hide(&app, gen);
}

fn windows(app: &AppHandle) -> Option<(tauri::WebviewWindow, gtk::ApplicationWindow)> {
    let window = app.get_webview_window(LABEL)?;
    match window.gtk_window() {
        Ok(gtk_window) => Some((window, gtk_window)),
        Err(e) => {
            warn!("overlay: no GTK window: {e}");
            None
        }
    }
}

/// The icon file for an `Icon=` value: an absolute path as is, a theme name through the GTK icon
/// theme. Only formats WebKit can show (`config::image_mime`; not xpm).
pub fn icon_file(theme: Option<&gtk::IconTheme>, icon: &str) -> Option<PathBuf> {
    if icon.is_empty() {
        return None;
    }
    let path = if icon.starts_with('/') {
        PathBuf::from(icon)
    } else {
        theme?.lookup_icon(icon, 96, gtk::IconLookupFlags::empty())?.filename()?
    };
    (path.is_file() && config::image_mime(&path).is_some()).then_some(path)
}

/// 9c: the page's selection (its own px): hide the overlay, then ask kompact for that region.
/// kompact waits until the overlay's surface is gone before it captures.
#[tauri::command]
pub fn select_region(app: AppHandle, overlay: State<'_, Overlay>, x: i32, y: i32, w: i32, h: i32) -> Result<(), String> {
    let on = overlay.lock().on;
    overlay.close(&app, "screenshot region chosen");
    let (gx, gy, w, h) = global_region(on, x, y, w, h);
    info!("overlay: screenshot region {gx},{gy} {w}x{h}");
    app.state::<crate::compositor::Compositor>().send_shot(gx, gy, w, h)
}

/// From the panel's buttons: open `mode`, switch to it, or close it if it's what's showing.
#[tauri::command]
pub fn toggle_overlay(app: AppHandle, overlay: State<'_, Overlay>, mode: String) {
    let Some(mode) = Mode::parse(&mode) else {
        warn!("overlay: unknown mode {mode:?}");
        return;
    };
    overlay.toggle(&app, mode, "panel button");
}

/// 12a: a menu-bar status item was clicked; `x` is its right edge on the monitor.
#[tauri::command]
pub fn open_status(app: AppHandle, overlay: State<'_, Overlay>, which: String, x: f64) {
    if !STATUS_ITEMS.contains(&which.as_str()) {
        warn!("overlay: unknown status item {which:?}");
        return;
    }
    overlay.open_status(&app, which, x);
}

/// 12a: which status menu the page should show, and where.
#[tauri::command]
pub fn status_target(overlay: State<'_, Overlay>) -> Option<(String, f64)> {
    overlay.status_target()
}

#[tauri::command]
pub fn close_overlay(app: AppHandle, overlay: State<'_, Overlay>) {
    overlay.close(&app, "closed from the page");
}

/// Called by the overlay page once it has loaded: what it should show right away, if a panel
/// button was pressed while it was still loading.
#[tauri::command]
pub fn overlay_ready(overlay: State<'_, Overlay>) -> Option<Mode> {
    let mut inner = overlay.lock();
    inner.ready = true;
    info!("overlay: page ready");
    inner.open
}

/// The installed apps, scanned fresh on every open (a few ms), with their icons.
#[tauri::command]
pub fn list_apps(overlay: State<'_, Overlay>) -> Vec<AppEntry> {
    overlay.refresh_apps()
}

impl Overlay {
    /// Scan the installed apps and their icons (GTK main thread: the icon theme), keep them for
    /// `launch` and `kompact-icon://`, and return them for a page. Launchpad, Spotlight and the
    /// dock (6m-d) all go through here.
    pub fn refresh_apps(&self) -> Vec<AppEntry> {
        let list = scan_apps();
        let theme = gtk::IconTheme::default();
        let mut icons: HashMap<String, PathBuf> =
            list.iter().filter_map(|a| Some((a.id.clone(), icon_file(theme.as_ref(), &a.icon)?))).collect();
        // The dock's Trash (6m-d2), empty and full.
        for (id, name) in [(crate::dock::TRASH_ICON, "user-trash"), (crate::dock::TRASH_FULL_ICON, "user-trash-full")] {
            if let Some(file) = icon_file(theme.as_ref(), name) {
                icons.insert(id.to_string(), file);
            }
        }
        info!("apps: {} installed, {} with icons", list.len(), icons.len());
        // 10c: System Settings is ours, not a desktop file.
        let settings_icon = ["preferences-system", "org.gnome.Settings", "preferences-desktop"]
            .iter()
            .find_map(|name| icon_file(theme.as_ref(), name));
        let has_settings_icon = settings_icon.is_some();
        if let Some(file) = settings_icon {
            icons.insert(crate::settings::APP_ID.to_string(), file);
        }
        let mut entries: Vec<AppEntry> = list
            .iter()
            .map(|a| AppEntry {
                id: a.id.clone(),
                name: a.name.clone(),
                comment: a.comment.clone(),
                keywords: a.keywords.clone(),
                icon: icons.contains_key(&a.id),
            })
            .collect();
        entries.push(AppEntry {
            id: crate::settings::APP_ID.into(),
            name: "System Settings".into(),
            comment: "Keyboard shortcuts".into(),
            keywords: "settings;preferences;keyboard;shortcuts;keys;hotkeys;".into(),
            icon: has_settings_icon,
        });
        let mut inner = self.lock();
        inner.apps = list;
        // 8c: keep the pictures of notifications on screen.
        // Notification pictures (8c) and the user's pictures (14b) aren't apps'.
        inner.icons.retain(|k, _| k.starts_with("notify-") || k.starts_with("user-"));
        inner.icons.extend(icons);
        entries
    }

    /// Open the dock menu for `desktop_id` at screen x `x`.
    pub fn open_dock_menu(&self, app: &AppHandle, desktop_id: String, x: f64, y: f64) {
        self.lock().dock_menu = Some((desktop_id, x, y));
        if self.lock().open == Some(Mode::DockMenu) {
            // Already showing one: show the new one instead.
            self.lock().open = None;
        }
        self.open(app, Mode::DockMenu, "dock right-click");
    }

    /// 12a: open (or, clicked again, close) a status item's menu.
    pub fn open_status(&self, app: &AppHandle, which: String, x: f64) {
        let same = self.lock().open == Some(Mode::Status) && self.lock().status.as_ref().map(|(w, _)| w.as_str()) == Some(which.as_str());
        if same {
            self.close(app, "status item clicked again");
            return;
        }
        self.lock().status = Some((which, x));
        if self.lock().open == Some(Mode::Status) {
            // Another item's menu is up: show this one instead.
            self.lock().open = None;
        }
        self.open(app, Mode::Status, "menu-bar status item");
    }

    /// 12e: show polkit's password dialog (over whatever else is open).
    pub fn open_auth(&self, app: &AppHandle) {
        if self.lock().open.is_some_and(|m| m != Mode::Auth) {
            self.lock().open = None;
        }
        self.open(app, Mode::Auth, "polkit asks for a password");
    }

    pub fn status_target(&self) -> Option<(String, f64)> {
        self.lock().status.clone()
    }

    pub fn dock_menu_target(&self) -> Option<(String, f64, f64)> {
        self.lock().dock_menu.clone()
    }

    /// The installed app with this desktop id.
    pub fn app_entry(&self, id: &str) -> Option<App> {
        self.lock().apps.iter().find(|a| a.id == id).cloned()
    }

    /// 12j: every installed app, as last scanned.
    pub fn apps(&self) -> Vec<App> {
        self.lock().apps.clone()
    }

    /// Whether `refresh_apps` has run yet.
    pub fn has_apps(&self) -> bool {
        !self.lock().apps.is_empty()
    }

    /// Start the app with desktop id `id`. False if we don't know it.
    pub fn launch_id(&self, handle: &AppHandle, id: &str) -> bool {
        if id == crate::settings::APP_ID {
            crate::settings::open(handle, "Spotlight / Launchpad");
            return true;
        }
        let found = self.lock().apps.iter().find(|a| a.id == id).cloned();
        match found {
            Some(a) => {
                spawn(handle, &a, &self.our_env);
                true
            }
            None => {
                warn!("launch: no app {id:?}");
                false
            }
        }
    }
}

fn scan_apps() -> Vec<App> {
    let path = std::env::var("PATH").ok();
    let dirs = apps::data_dirs(|k| std::env::var(k).ok());
    apps::scan(&dirs, |p| apps::program_exists(p, path.as_deref()))
}

#[tauri::command]
pub fn launch(app: AppHandle, overlay: State<'_, Overlay>, id: String) {
    overlay.close(&app, "app chosen");
    overlay.launch_id(&app, &id);
}

/// A view of the overlay asked for another (the logo menu's Launchpad and Spotlight items).
#[tauri::command]
pub fn show_overlay(app: AppHandle, overlay: State<'_, Overlay>, mode: String) {
    match Mode::parse(&mode) {
        Some(mode) => overlay.open(&app, mode, "from the overlay"),
        None => warn!("overlay: unknown mode {mode:?}"),
    }
}

/// "About kompact".
#[tauri::command]
pub fn system_info() -> crate::system::SystemInfo {
    crate::system::read()
}

/// The logo menu's Restart… / Shut Down…, after the page's confirm dialog. An error (logind said
/// no, e.g. another user is logged in) goes back to the page to show.
#[tauri::command]
pub fn power(app: AppHandle, overlay: State<'_, Overlay>, action: String) -> Result<(), String> {
    let action = crate::system::Power::parse(&action).ok_or_else(|| format!("unknown action {action:?}"))?;
    if action == crate::system::Power::Sleep && !crate::power::sleep_enabled() {
        return Err("Sleep is off (KOMPACT_SLEEP=on turns it on)".into());
    }
    let [prog, arg] = action.command();
    info!("power: running {prog} {arg}");
    let out = Command::new(prog).arg(arg).stdin(Stdio::null()).output().map_err(|e| format!("{prog}: {e}"))?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
        warn!("power: {prog} {arg} failed ({}): {err}", out.status);
        return Err(if err.is_empty() { format!("{prog} {arg} failed ({})", out.status) } else { err });
    }
    overlay.close(&app, "power");
    Ok(())
}

/// The logo menu's Restart Shell: start a fresh copy of this binary (after a second, once this
/// one has let go of its single-instance lock) and quit. It keeps our environment and output.
/// Started by kompact (6d), we just exit with `EXIT_RESTART` and kompact starts us again.
#[tauri::command]
pub fn restart_shell(app: AppHandle) {
    if std::env::var(crate::SUPERVISED_ENV).is_ok_and(|v| v == "1") {
        info!("restart shell: exiting for kompact to start us again");
        app.exit(crate::EXIT_RESTART);
        return;
    }
    let exe = match std::env::current_exe() {
        Ok(p) => p,
        Err(e) => {
            error!("restart shell: can't find our binary: {e}");
            return;
        }
    };
    // Rebuilt while running, Linux reports "<path> (deleted)"; the new binary is at <path>.
    let exe = PathBuf::from(exe.to_string_lossy().trim_end_matches(" (deleted)").to_string());
    let mut cmd = Command::new("sh");
    cmd.arg("-c").arg("sleep 1; exec \"$0\"").arg(&exe).stdin(Stdio::null());
    std::os::unix::process::CommandExt::process_group(&mut cmd, 0);
    match cmd.spawn() {
        Ok(_) => {
            info!("restart shell: starting {} again in 1 s, exiting", exe.display());
            app.exit(0);
        }
        Err(e) => error!("restart shell: {e}"),
    }
}

/// Spotlight's calculator (`calc.rs`): the result of `text`, or null if it isn't a calculation.
#[tauri::command]
pub fn calculate(text: String) -> Option<String> {
    crate::calc::calculate(&text)
}

/// Start an app as its own process (own process group, no stdio), so it outlives the shell.
/// 8a: with an activation token from kompact (`XDG_ACTIVATION_TOKEN`), so an app that is
/// already running brings its window forward; asked for off the main thread, at most
/// `TOKEN_WAIT`, and the app starts without one if kompact doesn't answer.
fn spawn(handle: &AppHandle, app: &App, our_env: &[&'static str]) {
    let Some(argv) = apps::exec_argv(&app.exec) else {
        warn!("launch {}: can't parse Exec={:?}", app.id, app.exec);
        return;
    };
    let (handle, app, our_env) = (handle.clone(), app.clone(), our_env.to_vec());
    std::thread::spawn(move || {
        let token = handle.try_state::<crate::compositor::Compositor>().and_then(|c| c.activation_token(TOKEN_WAIT));
        spawn_now(&app.id, &argv, &our_env, token);
    });
}

const TOKEN_WAIT: std::time::Duration = std::time::Duration::from_millis(300);

fn spawn_now(id: &str, argv: &[String], our_env: &[&str], token: Option<String>) {
    let mut cmd = Command::new(&argv[0]);
    cmd.args(&argv[1..]).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    if let Some(home) = std::env::var_os("HOME") {
        cmd.current_dir(home);
    }
    for key in our_env {
        cmd.env_remove(key);
    }
    std::os::unix::process::CommandExt::process_group(&mut cmd, 0);
    if let Some(token) = &token {
        cmd.env("XDG_ACTIVATION_TOKEN", token);
    }
    match cmd.spawn() {
        Ok(mut child) => {
            info!("launched {} ({:?}) as pid {}{}", id, argv, child.id(), if token.is_some() { ", with an activation token" } else { "" });
            // Reap it when it exits, so it doesn't linger as a zombie.
            std::thread::spawn(move || {
                let _ = child.wait();
            });
        }
        Err(e) => error!("launch {}: {e}", id),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overlay_goes_to_the_pointers_monitor() {
        let monitors = [(1366, 0, 1440, 900), (0, 0, 1366, 768)];
        assert_eq!(pick_monitor(&monitors, (0, 0, 1366, 768)), Some(1));
        assert_eq!(pick_monitor(&monitors, (1366, 0, 1440, 900)), Some(0));
        // GDK rounding differently: the one containing the output's centre.
        assert_eq!(pick_monitor(&monitors, (0, 0, 1360, 760)), Some(1));
        assert_eq!(pick_monitor(&monitors, (5000, 0, 100, 100)), None);
        assert_eq!(pick_monitor(&[], (0, 0, 1366, 768)), None);
    }

    #[test]
    fn regions_become_global() {
        assert_eq!(global_region(Some((1366, 0, 1440, 900)), 100, 50, 300, 200), (1466, 50, 300, 200));
        assert_eq!(global_region(Some((0, 0, 1366, 768)), 10, 20, 30, 40), (10, 20, 30, 40));
        assert_eq!(global_region(None, 10, 20, 30, 40), (10, 20, 30, 40));
        assert_eq!(Mode::parse("select"), Some(Mode::Select));
    }

    #[test]
    fn modes() {
        assert_eq!(Mode::parse("launchpad"), Some(Mode::Launchpad));
        assert_eq!(Mode::parse("spotlight"), Some(Mode::Spotlight));
        assert_eq!(Mode::parse("Spotlight"), None);
        assert_eq!(Mode::parse("menu"), Some(Mode::Menu));
        for m in [Mode::Launchpad, Mode::Spotlight, Mode::Menu, Mode::DockMenu, Mode::Spaces] {
            assert_eq!(Mode::parse(m.name()), Some(m));
        }
    }

    #[test]
    fn toggling() {
        assert_eq!(toggle(None, Mode::Spotlight), Toggle::Open);
        assert_eq!(toggle(Some(Mode::Spotlight), Mode::Spotlight), Toggle::Close);
        assert_eq!(toggle(Some(Mode::Launchpad), Mode::Spotlight), Toggle::Switch);
    }
}
