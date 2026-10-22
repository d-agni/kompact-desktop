//! 19b: Siva on the desktop: a small transparent layer surface (`siva.html`) on the top layer
//! (below full-screen windows), anchored top-left with margins so it can sit anywhere; never the
//! keyboard, and the pointer only on what's drawn (`siva_input`). Dragged with the pointer (Smithay
//! keeps a pressed button's surface under the pointer, so the page sees the whole drag) and placed
//! on release; or put in a corner from his menu. Off until shown from the Control Center.
//!
//! `~/.config/kompact/siva`: `shown=on|off`, `motion=full|less`, `x=…`, `y=…` (where he was
//! put; none: the bottom-right corner, above the dock).

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};
use tracing::{info, warn};

pub const LABEL: &str = "siva";
/// The surface: room for Siva and his bubble or menu above him.
pub const WIDTH: i32 = 300;
pub const HEIGHT: i32 = 320;
/// Kept from the screen's edges, and above the dock in the bottom corners.
const EDGE: i32 = 16;
const ABOVE_DOCK: i32 = 100;
/// The menu bar's strip, which top-anchored surfaces are placed below.
const MENU_BAR: i32 = 28;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Pos {
    pub x: i32,
    pub y: i32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Prefs {
    pub shown: bool,
    /// Less movement: no continuous animation, only the mood's face.
    pub less_motion: bool,
    pub pos: Option<Pos>,
}

impl Default for Prefs {
    fn default() -> Self {
        Prefs { shown: false, less_motion: false, pos: None }
    }
}

pub fn parse_prefs(text: &str) -> Prefs {
    let mut p = Prefs::default();
    let (mut x, mut y) = (None, None);
    for line in text.lines().map(str::trim).filter(|l| !l.starts_with('#')) {
        let Some((k, v)) = line.split_once('=') else { continue };
        match (k.trim(), v.trim()) {
            ("shown", "on") => p.shown = true,
            ("shown", "off") => p.shown = false,
            ("motion", "less") => p.less_motion = true,
            ("motion", "full") => p.less_motion = false,
            ("x", n) => x = n.parse::<i32>().ok().filter(|n| (0..=16384).contains(n)),
            ("y", n) => y = n.parse::<i32>().ok().filter(|n| (0..=16384).contains(n)),
            _ => {}
        }
    }
    if let (Some(x), Some(y)) = (x, y) {
        p.pos = Some(Pos { x, y });
    }
    p
}

pub fn prefs_text(p: Prefs) -> String {
    let mut s = format!(
        "# Siva, the assistant (written by Siva's menu and the Control Center)\nshown={}\nmotion={}\n",
        if p.shown { "on" } else { "off" },
        if p.less_motion { "less" } else { "full" }
    );
    if let Some(Pos { x, y }) = p.pos {
        s.push_str(&format!("x={x}\ny={y}\n"));
    }
    s
}

/// The area margins are measured in: the monitor below the menu bar.
fn area(monitor: (i32, i32)) -> (i32, i32) {
    (monitor.0.max(WIDTH), (monitor.1 - MENU_BAR).max(HEIGHT))
}

/// Kept wholly on the monitor.
pub fn clamp(p: Pos, monitor: (i32, i32)) -> Pos {
    let (w, h) = area(monitor);
    Pos { x: p.x.clamp(0, w - WIDTH), y: p.y.clamp(0, h - HEIGHT) }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Corner {
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
}

impl Corner {
    pub fn parse(s: &str) -> Option<Corner> {
        match s {
            "top-left" => Some(Corner::TopLeft),
            "top-right" => Some(Corner::TopRight),
            "bottom-left" => Some(Corner::BottomLeft),
            "bottom-right" => Some(Corner::BottomRight),
            _ => None,
        }
    }

    pub fn pos(self, monitor: (i32, i32)) -> Pos {
        let (w, h) = area(monitor);
        let (left, right) = (EDGE, w - WIDTH - EDGE);
        let (top, bottom) = (EDGE / 2, h - HEIGHT - ABOVE_DOCK);
        let p = match self {
            Corner::TopLeft => Pos { x: left, y: top },
            Corner::TopRight => Pos { x: right, y: top },
            Corner::BottomLeft => Pos { x: left, y: bottom },
            Corner::BottomRight => Pos { x: right, y: bottom },
        };
        clamp(p, monitor)
    }
}

/// Where a drag ends: the surface moves by how far the pointer went from where it was pressed
/// (`grab` and `release` in surface pixels, as the page saw them).
pub fn dropped(at: Pos, grab: (f64, f64), release: (f64, f64), monitor: (i32, i32)) -> Pos {
    let dx = (release.0 - grab.0).round() as i32;
    let dy = (release.1 - grab.1).round() as i32;
    clamp(Pos { x: at.x + dx, y: at.y + dy }, monitor)
}

/// Which half he's on: his bubble opens toward the middle of the screen.
pub fn on_left(p: Pos, monitor: (i32, i32)) -> bool {
    p.x + WIDTH / 2 < monitor.0 / 2
}

// ---- The window ----

fn prefs_file() -> Option<std::path::PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(std::path::PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join(".config")))?;
    Some(base.join("kompact").join("siva"))
}

pub fn load() -> Prefs {
    prefs_file().and_then(|f| std::fs::read_to_string(f).ok()).map(|t| parse_prefs(&t)).unwrap_or_default()
}

fn save(p: Prefs) {
    let Some(file) = prefs_file() else { return };
    if let Some(dir) = file.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Err(e) = std::fs::write(&file, prefs_text(p)) {
        warn!("siva: can't save {}: {e}", file.display());
    }
}

static PREFS: std::sync::Mutex<Option<Prefs>> = std::sync::Mutex::new(None);

fn prefs() -> Prefs {
    *PREFS.lock().unwrap_or_else(|e| e.into_inner()).get_or_insert_with(load)
}

fn set_prefs(p: Prefs) {
    *PREFS.lock().unwrap_or_else(|e| e.into_inner()) = Some(p);
    save(p);
}

/// The size of the monitor Siva's surface is on (logical px), or a guess before it's mapped.
fn monitor_size(gtk_window: &gtk::ApplicationWindow) -> (i32, i32) {
    use gtk::prelude::*;
    let geometry = gtk_window
        .window()
        .and_then(|w| gtk::prelude::WidgetExt::display(gtk_window).monitor_at_window(&w))
        .map(|m| m.geometry());
    geometry.map(|g| (g.width(), g.height())).unwrap_or((1366, 768))
}

fn place(gtk_window: &gtk::ApplicationWindow, p: Pos) {
    use gtk_layer_shell::{Edge, LayerShell};
    gtk_window.set_layer_shell_margin(Edge::Left, p.x);
    gtk_window.set_layer_shell_margin(Edge::Top, p.y);
}

/// Put the surface where the settings say, kept on its monitor (also once the page is up, when
/// the monitor is known). Where he is now.
fn apply(app: &AppHandle) -> Option<(Pos, (i32, i32))> {
    let window = app.get_webview_window(LABEL)?;
    let gtk_window = window.gtk_window().ok()?;
    let monitor = monitor_size(&gtk_window);
    let p = prefs().pos.map(|p| clamp(p, monitor)).unwrap_or_else(|| Corner::BottomRight.pos(monitor));
    place(&gtk_window, p);
    Some((p, monitor))
}

#[derive(Clone, Serialize)]
pub struct SivaState {
    shown: bool,
    less_motion: bool,
    /// His bubble opens to the right (he's on the left half).
    left: bool,
}

fn state(app: &AppHandle) -> SivaState {
    let p = prefs();
    let left = apply(app).map(|(pos, m)| on_left(pos, m)).unwrap_or(false);
    SivaState { shown: p.shown, less_motion: p.less_motion, left }
}

/// Builder settings for `main.rs`: the window is made hidden; shown here if he was shown.
pub fn after_build(app: &AppHandle) {
    let p = prefs();
    let _ = apply(app);
    if p.shown {
        if let Some(w) = app.get_webview_window(LABEL) {
            let _ = w.show();
        }
    }
    info!("siva: built ({})", if p.shown { "shown" } else { "hidden until the Control Center shows him" });
}

#[tauri::command]
pub fn siva_state(app: AppHandle) -> SivaState {
    state(&app)
}

/// The page's drawn parts (`[x, y, w, h]` in surface px): only they take the pointer.
#[tauri::command]
pub fn siva_input(app: AppHandle, rects: Vec<[f64; 4]>) -> Result<(), String> {
    use gtk::prelude::*;
    let window = app.get_webview_window(LABEL).ok_or("no siva window")?;
    let gtk_window = window.gtk_window().map_err(|e| e.to_string())?;
    let region = gtk::cairo::Region::create();
    for [x, y, w, h] in rects.iter().take(16) {
        let r = gtk::cairo::RectangleInt::new(x.floor() as i32, y.floor() as i32, w.ceil() as i32, h.ceil() as i32);
        region.union_rectangle(&r).map_err(|e| e.to_string())?;
    }
    gtk_window.input_shape_combine_region(Some(&region));
    Ok(())
}

/// A drag ended: move by how far the pointer went, remember it. Whether he's now on the left.
#[tauri::command]
pub fn siva_drop(app: AppHandle, gx: f64, gy: f64, rx: f64, ry: f64) -> Result<bool, String> {
    let (at, monitor) = apply(&app).ok_or("no siva window")?;
    let to = dropped(at, (gx, gy), (rx, ry), monitor);
    set_prefs(Prefs { pos: Some(to), ..prefs() });
    let _ = apply(&app);
    info!("siva: moved to {},{}", to.x, to.y);
    Ok(on_left(to, monitor))
}

#[tauri::command]
pub fn siva_corner(app: AppHandle, corner: String) -> Result<bool, String> {
    let c = Corner::parse(&corner).ok_or_else(|| format!("no corner {corner:?}"))?;
    let (_, monitor) = apply(&app).ok_or("no siva window")?;
    let to = c.pos(monitor);
    set_prefs(Prefs { pos: Some(to), ..prefs() });
    let _ = apply(&app);
    info!("siva: moved to the {corner} corner");
    Ok(on_left(to, monitor))
}

/// Shown or hidden (the Control Center's Siva tile, his menu's Hide).
#[tauri::command]
pub fn siva_show(app: AppHandle, on: bool) -> Result<(), String> {
    let window = app.get_webview_window(LABEL).ok_or("no siva window")?;
    set_prefs(Prefs { shown: on, ..prefs() });
    if on {
        let _ = apply(&app);
        window.show().map_err(|e| e.to_string())?;
    } else {
        window.hide().map_err(|e| e.to_string())?;
    }
    info!("siva: {}", if on { "shown" } else { "hidden" });
    let _ = app.emit("kompact-siva", state(&app));
    Ok(())
}

/// 19c: the keyboard while his bubble takes typing (Exclusive, as the overlay does), and back
/// to none after, so the window that had it gets it again.
#[tauri::command]
pub fn siva_keyboard(app: AppHandle, on: bool) -> Result<(), String> {
    use gtk_layer_shell::{KeyboardMode, LayerShell};
    let window = app.get_webview_window(LABEL).ok_or("no siva window")?;
    let gtk_window = window.gtk_window().map_err(|e| e.to_string())?;
    gtk_window.set_keyboard_mode(if on { KeyboardMode::Exclusive } else { KeyboardMode::None });
    if on {
        let _ = window.set_focus();
    }
    Ok(())
}

#[tauri::command]
pub fn siva_motion(app: AppHandle, less: bool) {
    set_prefs(Prefs { less_motion: less, ..prefs() });
    info!("siva: {} movement", if less { "less" } else { "full" });
    let _ = app.emit("kompact-siva", state(&app));
}

#[cfg(test)]
mod tests {
    use super::*;

    const MON: (i32, i32) = (1366, 768);

    #[test]
    fn settings() {
        assert_eq!(parse_prefs(""), Prefs::default());
        let p = parse_prefs("shown=on\nmotion=less\nx=40\ny=300\n");
        assert_eq!(p, Prefs { shown: true, less_motion: true, pos: Some(Pos { x: 40, y: 300 }) });
        assert_eq!(parse_prefs(&prefs_text(p)), p);
        // Half a position, or nonsense, is no position.
        assert_eq!(parse_prefs("x=40").pos, None);
        assert_eq!(parse_prefs("x=-5\ny=3").pos, None);
        assert_eq!(parse_prefs("x=abc\ny=3").pos, None);
    }

    #[test]
    fn stays_on_screen() {
        // The area below the menu bar is 1366 × 740.
        assert_eq!(clamp(Pos { x: -50, y: -9 }, MON), Pos { x: 0, y: 0 });
        assert_eq!(clamp(Pos { x: 5000, y: 5000 }, MON), Pos { x: 1366 - WIDTH, y: 740 - HEIGHT });
        // A monitor smaller than him: pinned at 0.
        assert_eq!(clamp(Pos { x: 9, y: 9 }, (200, 200)), Pos { x: 0, y: 0 });
    }

    #[test]
    fn corners() {
        assert_eq!(Corner::TopLeft.pos(MON), Pos { x: 16, y: 8 });
        assert_eq!(Corner::BottomRight.pos(MON), Pos { x: 1366 - WIDTH - 16, y: 740 - HEIGHT - 100 });
        assert_eq!(Corner::parse("bottom-left"), Some(Corner::BottomLeft));
        assert_eq!(Corner::parse("middle"), None);
        assert!(on_left(Corner::TopLeft.pos(MON), MON));
        assert!(!on_left(Corner::BottomRight.pos(MON), MON));
    }

    #[test]
    fn dragging() {
        let at = Pos { x: 1000, y: 300 };
        // Pressed at (150, 250) in the surface, let go at (-350, 190): 500 left, 60 up.
        assert_eq!(dropped(at, (150.0, 250.0), (-350.0, 190.0), MON), Pos { x: 500, y: 240 });
        // Thrown off the screen: kept on it.
        assert_eq!(dropped(at, (150.0, 250.0), (2000.0, 900.0), MON), Pos { x: 1366 - WIDTH, y: 740 - HEIGHT });
    }
}
