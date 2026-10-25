//! 6m-c: the shell's end of kompact's private channel (kompact `src/ipc.rs` has the protocol).
//!
//! A thread connects to `$XDG_RUNTIME_DIR/kompact-<WAYLAND_DISPLAY>.sock` (retrying every 2 s,
//! so the shell works without it and picks kompact up when it appears), reads its JSON lines,
//! adds each window's app name and desktop id from the installed apps, and passes them to the
//! pages as the `kompact-windows` event. Shortcuts (`Super+Space`) go to the overlay. Commands
//! from the pages (`window_action`) are written back as text lines.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde_json::Value;
use tauri::{AppHandle, Emitter, Manager, State};
use tracing::{info, warn};

use crate::apps::{self, App};

#[derive(Default)]
pub struct Compositor {
    writer: Mutex<Option<UnixStream>>,
    /// The last `kompact-windows` payload, for pages that load later (`compositor_state`).
    state: Mutex<Option<Value>>,
    /// 8a: launches waiting for an activation token, oldest first.
    token_waiters: Mutex<std::collections::VecDeque<std::sync::mpsc::Sender<String>>>,
    /// 9e-c: the last `sharing` message (screen sharing on/off), for the panel (`sharing_state`).
    sharing: Mutex<Option<Value>>,
    /// 10b: the last `keymap` message (every shortcut), for the settings window (`keymap_state`).
    keymap: Mutex<Option<Value>>,
    /// 11c: the last `settings` message (kompact's own settings), for the settings window.
    settings: Mutex<Option<Value>>,
    /// 12f: the last `keyboard` message (the layouts and the active one).
    keyboard: Mutex<Option<Value>>,
    /// 12h-1: the last `displays` message (where the monitors are), for Settings → Displays.
    displays: Mutex<Option<Value>>,
    /// 12i-1: the last `idle` message (the apps keeping the screen awake).
    idle: Mutex<Option<Value>>,
    /// 19k: the last `layout` message (where the windows are) and when it came, for the
    /// assistant's "the Firefox on the left".
    layout: Mutex<Option<(std::time::Instant, Value)>>,
}

/// 9e-c: the monitors a `sharing` message says are shared (empty when it's off or malformed).
pub fn shared_outputs(msg: &Value) -> Vec<String> {
    if msg.get("on").and_then(Value::as_bool) != Some(true) {
        return Vec::new();
    }
    msg.get("outputs")
        .and_then(Value::as_array)
        .map(|l| l.iter().filter_map(Value::as_str).map(str::to_string).collect())
        .unwrap_or_default()
}

fn socket_path() -> Option<PathBuf> {
    let dir = std::env::var_os("XDG_RUNTIME_DIR")?;
    let display = std::env::var("WAYLAND_DISPLAY").ok()?;
    Some(PathBuf::from(dir).join(format!("kompact-{}.sock", display.replace('/', "_"))))
}

/// The text line for a page's `window_action`, or `None` if it isn't one we send.
pub fn request_line(action: &str, id: Option<u64>) -> Option<String> {
    match (action, id) {
        ("activate" | "close" | "minimize" | "thumb", Some(id)) => Some(format!("{action} {id}\n")),
        // 6e: the panel's workspace indicator; `id` is the workspace (1-based).
        ("workspace", Some(n)) => Some(format!("workspace {n}\n")),
        ("logout" | "layout" | "lock" | "token" | "show-desktop", None) => Some(format!("{action}\n")),
        // 16b/16c: the window switcher (the dock's button, the overlay's cards).
        ("switcher-open", None) => Some("switcher open\n".into()),
        ("switcher-cancel", None) => Some("switcher cancel\n".into()),
        ("switcher-select", Some(id)) => Some(format!("switcher select {id}\n")),
        ("switcher-pick", Some(id)) => Some(format!("switcher pick {id}\n")),
        _ => None,
    }
}

/// 11a: `dock-icons <size> <id>:<x>:<y> …`, the dock's icons with windows (centres in the dock
/// surface's coordinates); at most 256, as kompact takes.
pub fn dock_icons_line(size: u32, icons: &[[i64; 3]]) -> Option<String> {
    if !(8..=256).contains(&size) || icons.len() > 256 || icons.iter().any(|i| i[0] < 0) {
        return None;
    }
    let mut line = format!("dock-icons {size}");
    for [id, x, y] in icons {
        line.push_str(&format!(" {id}:{x}:{y}"));
    }
    line.push('\n');
    Some(line)
}

#[tauri::command]
pub fn dock_icons(c: State<'_, Compositor>, size: u32, icons: Vec<[i64; 3]>) -> Result<(), String> {
    let line = dock_icons_line(size, &icons).ok_or("bad dock icons")?;
    c.send_dock_icons(&line)
}

/// 9c: the line asking kompact for a screenshot of a region, or `None` if it's empty.
pub fn shot_line(x: i32, y: i32, w: i32, h: i32) -> Option<String> {
    (w > 0 && h > 0).then(|| format!("shot {x} {y} {w} {h}\n"))
}

/// Add `name` (the app's name, else its app id, else the title) and `desktop_id` (or null) to
/// each window of a `windows` message.
pub fn enrich(mut msg: Value, apps: &[App]) -> Value {
    if let Some(list) = msg.get_mut("windows").and_then(Value::as_array_mut) {
        for w in list {
            let app_id = w.get("app_id").and_then(Value::as_str).unwrap_or("").to_string();
            let app = apps::for_app_id(apps, &app_id);
            let title = w.get("title").and_then(Value::as_str).unwrap_or("").to_string();
            let name = match app {
                Some(a) => a.name.clone(),
                None if !app_id.is_empty() => app_id.clone(),
                None => title,
            };
            if let Some(obj) = w.as_object_mut() {
                obj.insert("name".into(), Value::String(name));
                obj.insert("desktop_id".into(), app.map_or(Value::Null, |a| Value::String(a.id.clone())));
            }
        }
    }
    let fills = crate::dock::focused_fills(&msg);
    if let Some(obj) = msg.as_object_mut() {
        obj.insert("focused_fills".into(), Value::Bool(fills));
    }
    msg
}

/// Whether a `windows` message has an app id we couldn't match (the app list may be stale).
fn has_unknown(msg: &Value) -> bool {
    msg.get("windows").and_then(Value::as_array).is_some_and(|l| {
        l.iter().any(|w| w.get("desktop_id").is_some_and(Value::is_null) && w.get("app_id").and_then(Value::as_str).is_some_and(|a| !a.is_empty()))
    })
}

fn scan_apps() -> Vec<App> {
    let path = std::env::var("PATH").ok();
    let dirs = apps::data_dirs(|k| std::env::var(k).ok());
    apps::scan(&dirs, |p| apps::program_exists(p, path.as_deref()))
}

pub fn start(app: AppHandle) {
    let Some(path) = socket_path() else {
        warn!("compositor: no XDG_RUNTIME_DIR or WAYLAND_DISPLAY, not connecting to kompact");
        return;
    };
    std::thread::spawn(move || {
        let mut said_waiting = false;
        loop {
            match UnixStream::connect(&path) {
                Ok(stream) => {
                    info!("compositor: connected to {}", path.display());
                    said_waiting = false;
                    serve(&app, stream);
                    info!("compositor: kompact went away, reconnecting");
                    if let Some(c) = app.try_state::<Compositor>() {
                        *c.writer.lock().unwrap_or_else(|e| e.into_inner()) = None;
                    }
                }
                Err(e) if !said_waiting => {
                    info!("compositor: no kompact channel at {} yet ({e}), retrying every 2 s", path.display());
                    said_waiting = true;
                }
                Err(_) => {}
            }
            std::thread::sleep(Duration::from_secs(2));
        }
    });
}

fn serve(app: &AppHandle, stream: UnixStream) {
    let Some(c) = app.try_state::<Compositor>() else { return };
    match stream.try_clone() {
        Ok(w) => *c.writer.lock().unwrap_or_else(|e| e.into_inner()) = Some(w),
        Err(e) => warn!("compositor: can't share the socket for writing: {e}"),
    }
    // 10d-1: the Dock settings kompact applies (it keeps nothing of its own).
    let prefs = crate::dock::dock_prefs();
    if let Err(e) = c.send_dock_follow(prefs).and_then(|_| c.send_dock_side(prefs)) {
        warn!("compositor: can't send the dock settings: {e}");
    }
    // 16d: where the dock's bar is, if the dock has said.
    if let Some(line) = crate::dock::last_bar_line() {
        if let Err(e) = c.send_line_pub("dock-bar", &line) {
            warn!("compositor: can't send the dock bar: {e}");
        }
    }
    // 12i-1b: a kompact that restarted hears who keeps the screen awake.
    if let Some(s) = app.try_state::<crate::screensaver::ScreenSaver>() {
        s.send(app);
    }
    let mut apps = scan_apps();
    let mut scanned = Instant::now();
    for line in BufReader::new(stream).lines() {
        let Ok(line) = line else { break };
        let msg: Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(e) => {
                warn!("compositor: bad message ({e}): {line:.200}");
                continue;
            }
        };
        match msg.get("type").and_then(Value::as_str) {
            Some("windows") => {
                let mut out = enrich(msg.clone(), &apps);
                // A new app was installed since we looked: look again, at most every 10 s.
                if has_unknown(&out) && scanned.elapsed() > Duration::from_secs(10) {
                    apps = scan_apps();
                    scanned = Instant::now();
                    out = enrich(msg, &apps);
                }
                if let Some(t) = app.try_state::<crate::thumbs::Thumbs>() {
                    let ids: Vec<u64> = out
                        .get("windows")
                        .and_then(Value::as_array)
                        .map(|l| l.iter().filter_map(|w| w.get("id").and_then(Value::as_u64)).collect())
                        .unwrap_or_default();
                    t.retain(&ids);
                }
                *c.state.lock().unwrap_or_else(|e| e.into_inner()) = Some(out.clone());
                if let Err(e) = app.emit("kompact-windows", out) {
                    warn!("compositor: can't pass the window list on: {e}");
                }
            }
            Some("key") => {
                let action = msg.get("action").and_then(Value::as_str).unwrap_or("").to_string();
                info!("compositor: key {action}");
                let wanted = match action.as_str() {
                    "spotlight" => Some((crate::overlay::Mode::Spotlight, "Super+Space")),
                    // 9c: drag out a screenshot region.
                    "select" => Some((crate::overlay::Mode::Select, "Shift+Print")),
                    // 10a: the workspace overview from the keyboard.
                    "overview" => Some((crate::overlay::Mode::Spaces, "overview key")),
                    _ => None,
                };
                // 12c: the volume keys.
                if matches!(action.as_str(), "volume-up" | "volume-down" | "volume-mute" | "mic-mute") {
                    crate::sound::key(app, &action);
                }
                // 19d-2: Ctrl+Super+V, as a click on the assistant's menu-bar item.
                if action == "assistant" {
                    if let Err(e) = crate::assistant::hud::assistant_toggle(app.clone()) {
                        warn!("compositor: assistant key: {e}");
                    }
                }
                if let Some((mode, key)) = wanted {
                    let handle = app.clone();
                    let _ = app.run_on_main_thread(move || {
                        let overlay = handle.state::<crate::overlay::Overlay>();
                        overlay.toggle(&handle, mode, key);
                    });
                }
            }
            // 5j: the output under the pointer; the overlay opens there.
            Some("pointer_output") => {
                let num = |k: &str| msg.get(k).and_then(Value::as_i64).map(|v| v as i32);
                let (Some(x), Some(y), Some(w), Some(h)) = (num("x"), num("y"), num("w"), num("h")) else {
                    warn!("compositor: bad pointer_output message: {line:.200}");
                    continue;
                };
                let name = msg.get("name").and_then(Value::as_str).unwrap_or("?");
                info!("compositor: pointer on {name} ({x},{y} {w}x{h})");
                if let Some(o) = app.try_state::<crate::overlay::Overlay>() {
                    o.set_pointer_output((x, y, w, h));
                }
            }
            // 5j-3: the pointer was held at the bottom of another monitor.
            Some("dock_to") => {
                let to = msg.get("output").and_then(Value::as_str).unwrap_or("?").to_string();
                info!("compositor: dock to {to}");
                let handle = app.clone();
                let _ = app.run_on_main_thread(move || crate::remap_dock(&handle, &format!("pointer held at the bottom of {to}")));
            }
            // 8a: an activation token for the oldest launch waiting for one.
            Some("token") => {
                let token = msg.get("token").and_then(Value::as_str).unwrap_or("").to_string();
                let waiter = c.token_waiters.lock().unwrap_or_else(|e| e.into_inner()).pop_front();
                if let Some(w) = waiter.filter(|_| !token.is_empty()) {
                    let _ = w.send(token);
                }
            }
            Some("thumb") => {
                let (Some(m), Some(t)) = (crate::thumbs::parse(&msg), app.try_state::<crate::thumbs::Thumbs>()) else {
                    warn!("compositor: bad thumb message: {line:.200}");
                    continue;
                };
                if let Some(event) = t.take(&m) {
                    let _ = app.emit("kompact-thumb", event);
                }
            }
            // 9c: kompact saved a screenshot: a banner for it.
            Some("screenshot") => {
                let path = msg.get("path").and_then(Value::as_str).unwrap_or("");
                if path.is_empty() {
                    warn!("compositor: bad screenshot message: {line:.200}");
                    continue;
                }
                info!("compositor: screenshot saved: {path}");
                crate::notify::screenshot_saved(app, PathBuf::from(path));
            }
            // 9e-c: screen sharing started or stopped: the panel's icon.
            Some("sharing") => {
                let outputs = shared_outputs(&msg);
                if outputs.is_empty() {
                    info!("compositor: screen sharing off");
                } else {
                    info!("compositor: screen sharing on ({})", outputs.join(", "));
                }
                *c.sharing.lock().unwrap_or_else(|e| e.into_inner()) = Some(msg.clone());
                // 10d-5: a frame-rate change waiting for the share to end.
                crate::sharing::on_sharing_changed(!outputs.is_empty());
                let _ = app.emit("kompact-sharing", msg);
            }
            // 10b: the shortcuts, and answers to the settings window's `keys …` requests.
            Some("keymap") => {
                let preset = msg.get("preset").and_then(Value::as_str).unwrap_or("?");
                let n = msg.get("actions").and_then(Value::as_array).map_or(0, Vec::len);
                info!("compositor: keymap, preset {preset}, {n} actions");
                *c.keymap.lock().unwrap_or_else(|e| e.into_inner()) = Some(msg.clone());
                let _ = app.emit("kompact-keymap", msg);
            }
            // 11c: kompact's own settings (animations, title-bar double-click, focus follows mouse).
            Some("settings") => {
                info!("compositor: settings {line:.200}");
                *c.settings.lock().unwrap_or_else(|e| e.into_inner()) = Some(msg.clone());
                let _ = app.emit("kompact-settings", msg);
            }
            // 12f: the keyboard layouts and the active one (the menu bar's input switcher).
            Some("keyboard") => {
                info!("compositor: keyboard {line:.200}");
                *c.keyboard.lock().unwrap_or_else(|e| e.into_inner()) = Some(msg.clone());
                let _ = app.emit("kompact-keyboard", msg);
            }
            // 12h-1: where the monitors are (Settings → Displays).
            Some("displays") => {
                info!("compositor: displays {line:.300}");
                *c.displays.lock().unwrap_or_else(|e| e.into_inner()) = Some(msg.clone());
                // 12h-2: as many menu bars as asked for, on the right displays.
                let (handle, m) = (app.clone(), msg.clone());
                let _ = app.run_on_main_thread(move || crate::sync_panels(&handle, &m));
                let _ = app.emit("kompact-displays", msg);
            }
            // 12i-1: the apps keeping the screen awake (Settings → Lock Screen).
            Some("idle") => {
                info!("compositor: idle {line:.200}");
                *c.idle.lock().unwrap_or_else(|e| e.into_inner()) = Some(msg.clone());
                let _ = app.emit("kompact-idle", msg);
            }
            // 12i-2: the screen is locked (or not), for locking before sleep.
            Some("lock") => {
                let locked = msg.get("locked").and_then(Value::as_bool).unwrap_or(false);
                info!("compositor: screen {}", if locked { "locked" } else { "unlocked" });
                app.state::<crate::power::LockWatch>().set(locked);
            }
            Some("keys_result") => {
                let ok = msg.get("ok").and_then(Value::as_bool).unwrap_or(false);
                let text = msg.get("message").and_then(Value::as_str).unwrap_or("");
                info!("compositor: keys result: {} {text}", if ok { "ok" } else { "refused" });
                let _ = app.emit("kompact-keys-result", msg);
            }
            Some("recorded") => {
                info!("compositor: recorded {line:.120}");
                let _ = app.emit("kompact-recorded", msg);
            }
            // 16b: the window switcher appeared, changed or ended (kompact drives it).
            Some("switcher") => {
                let open = msg.get("open").and_then(Value::as_bool).unwrap_or(false);
                let handle = app.clone();
                let _ = app.emit("kompact-switcher", msg.clone());
                let _ = app.run_on_main_thread(move || {
                    let overlay = handle.state::<crate::overlay::Overlay>();
                    if open {
                        overlay.show_switcher(&handle, msg);
                    } else {
                        overlay.hide_switcher(&handle);
                    }
                });
            }
            // 6e: where the windows are, for the workspace overview.
            Some("layout") => {
                *c.layout.lock().unwrap_or_else(|e| e.into_inner()) = Some((std::time::Instant::now(), msg.clone()));
                let _ = app.emit("kompact-layout", msg);
            }
            other => warn!("compositor: unknown message type {other:?}"),
        }
    }
}

impl Compositor {
    /// 20b: the last `settings` message (kompact's own settings).
    pub fn last_settings(&self) -> Option<Value> {
        self.settings.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// 19k: the last `layout` message, if it came after `since`.
    pub fn layout_since(&self, since: std::time::Instant) -> Option<Value> {
        self.layout.lock().unwrap_or_else(|e| e.into_inner()).as_ref().filter(|(t, _)| *t >= since).map(|(_, v)| v.clone())
    }

    /// 9e-c: the last `sharing` message.
    pub fn sharing_message(&self) -> Option<Value> {
        self.sharing.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// The last window list (enriched), if kompact sent one.
    pub fn last_state(&self) -> Option<Value> {
        self.state.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// Send kompact a request (`request_line`).
    /// 8a: ask kompact for an activation token and wait for it (up to `wait`; not on the main
    /// thread). `None` if kompact isn't there or doesn't answer in time.
    pub fn activation_token(&self, wait: Duration) -> Option<String> {
        let (tx, rx) = std::sync::mpsc::channel();
        self.token_waiters.lock().unwrap_or_else(|e| e.into_inner()).push_back(tx);
        if let Err(e) = self.send("token", None) {
            info!("compositor: no activation token ({e})");
            self.token_waiters.lock().unwrap_or_else(|e| e.into_inner()).pop_back();
            return None;
        }
        match rx.recv_timeout(wait) {
            Ok(token) => Some(token),
            Err(_) => {
                warn!("compositor: no activation token within {} ms", wait.as_millis());
                None
            }
        }
    }

    pub fn send(&self, action: &str, id: Option<u64>) -> Result<(), String> {
        let line = request_line(action, id).ok_or_else(|| format!("unknown window action {action:?} {id:?}"))?;
        self.send_line(action, &line)
    }

    /// 9c: ask kompact for a screenshot of this region (global logical coordinates).
    pub fn send_shot(&self, x: i32, y: i32, w: i32, h: i32) -> Result<(), String> {
        let line = shot_line(x, y, w, h).ok_or_else(|| format!("empty screenshot region {w}x{h}"))?;
        self.send_line("shot", &line)
    }

    /// 10c: a `keys …` line from the settings window (`settings::keys_line`).
    pub fn send_keys_line(&self, line: &str) -> Result<(), String> {
        self.send_line("keys", line)
    }

    /// 11a: where each window's dock icon is, for the minimize animation (`dock_icons_line`).
    pub fn send_dock_icons(&self, line: &str) -> Result<(), String> {
        self.send_line("dock-icons", line)
    }

    /// 10d-1c: which edge the dock is on.
    pub fn send_dock_side(&self, prefs: crate::dock::DockPrefs) -> Result<(), String> {
        self.send_line("dock-side", &crate::dock::side_line(prefs))
    }

    /// 10d-1: whether the dock follows the pointer to another monitor (kompact does that).
    pub fn send_dock_follow(&self, prefs: crate::dock::DockPrefs) -> Result<(), String> {
        self.send_line("dock-follow", &crate::dock::follow_line(prefs))
    }

    /// 12f: a line from another module (`keyboard.rs`).
    pub fn send_line_pub(&self, action: &str, line: &str) -> Result<(), String> {
        self.send_line(action, line)
    }

    fn send_line(&self, action: &str, line: &str) -> Result<(), String> {
        let mut writer = self.writer.lock().unwrap_or_else(|e| e.into_inner());
        let Some(stream) = writer.as_mut() else { return Err("not connected to kompact".into()) };
        if action == "thumb" || action == "dock-icons" || action == "switcher-select" {
            // Every 2 s per window while a dock preview is open.
            tracing::debug!("compositor: sending {}", line.trim_end());
        } else {
            info!("compositor: sending {}", line.trim_end());
        }
        stream.write_all(line.as_bytes()).map_err(|e| {
            *writer = None;
            format!("can't reach kompact: {e}")
        })
    }
}

/// The last window list, for a page that loaded after it came (null before kompact sent one).
#[tauri::command]
pub fn compositor_state(c: State<'_, Compositor>) -> Option<Value> {
    c.last_state()
}

/// 11c: `set <key> <value>`: the key a short word of letters and dashes; the value one word
/// (12f: also digits and `,()_.:+-`, for `us,de(nodeadkeys)` and `-0.3`).
pub fn setting_line(key: &str, value: &str) -> Option<String> {
    let key_ok = !key.is_empty() && key.len() <= 40 && key.bytes().all(|b| b.is_ascii_lowercase() || b == b'-');
    let value_ok = !value.is_empty() && value.len() <= 160 && value.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b",()_.:+-".contains(&b));
    (key_ok && value_ok).then(|| format!("set {key} {value}\n"))
}

/// 12f: the last `keyboard` message (null before kompact sent one).
#[tauri::command]
pub fn keyboard_state(c: State<'_, Compositor>) -> Option<Value> {
    c.keyboard.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

/// 12i-1: the last `idle` message (null before kompact sent one).
#[tauri::command]
pub fn idle_state(c: State<'_, Compositor>) -> Option<Value> {
    c.idle.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

/// 12h-1: the last `displays` message (null before kompact sent one).
#[tauri::command]
pub fn displays_state(c: State<'_, Compositor>) -> Option<Value> {
    c.displays.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

/// 12h-1: `display-move <name> <x> <y>`, for a monitor name kompact gives (`HDMI-A-1`).
pub fn display_move_line(name: &str, x: i32, y: i32) -> Option<String> {
    let ok = !name.is_empty() && name.len() <= 32 && name.bytes().all(|b| b.is_ascii_alphanumeric() || b"-_".contains(&b));
    (ok && x.abs() <= 100_000 && y.abs() <= 100_000).then(|| format!("display-move {name} {x} {y}\n"))
}

/// 12h-1: Settings → Displays dropped a monitor at `x`,`y` (global coordinates); kompact snaps
/// it next to the others and answers with a `kompact-displays` event.
#[tauri::command]
pub fn display_move(c: State<'_, Compositor>, name: String, x: i32, y: i32) -> Result<(), String> {
    let line = display_move_line(&name, x, y).ok_or_else(|| format!("{name:?} isn't a monitor"))?;
    c.send_line("display-move", &line)
}

/// 12h-2: make `name` the main display (menu bar, dock).
#[tauri::command]
pub fn display_main(c: State<'_, Compositor>, name: String) -> Result<(), String> {
    // The same name check as a move.
    display_move_line(&name, 0, 0).ok_or_else(|| format!("{name:?} isn't a monitor"))?;
    c.send_line("display-main", &format!("display-main {name}\n"))
}

/// 12h-2: a menu bar on every display, or only on the main one.
#[tauri::command]
pub fn menu_bars(c: State<'_, Compositor>, every: bool) -> Result<(), String> {
    c.send_line("menu-bars", if every { "menu-bars every\n" } else { "menu-bars primary\n" })
}

/// 11c: change one of kompact's settings; kompact answers with a `kompact-settings` event.
#[tauri::command]
pub fn kompact_set(c: State<'_, Compositor>, key: String, value: String) -> Result<(), String> {
    let line = setting_line(&key, &value).ok_or("not a setting")?;
    c.send_line("set", &line)
}

/// 10d-4: `set-path <key> <path>`: a short key, and an absolute path on one line (or empty for
/// the default).
pub fn setting_path_line(key: &str, path: &str) -> Option<String> {
    let key_ok = !key.is_empty() && key.len() <= 40 && key.bytes().all(|b| b.is_ascii_lowercase() || b == b'-');
    let path_ok = path.is_empty() || (path.starts_with('/') && path.len() <= 1000 && !path.contains(['\n', '\r']));
    (key_ok && path_ok).then(|| format!("set-path {key} {path}\n"))
}

/// 10d-4: change a folder setting (`shots-dir`); kompact answers with `kompact-settings`.
#[tauri::command]
pub fn kompact_set_path(c: State<'_, Compositor>, key: String, path: String) -> Result<(), String> {
    let line = setting_path_line(&key, &path).ok_or("not a folder setting")?;
    c.send_line("set", &line)
}

/// 10d-4: "Choose…" for the screenshots folder: GTK's folder chooser; the folder picked is sent
/// to kompact.
#[tauri::command]
pub fn choose_shots_folder(app: AppHandle, current: Option<String>) {
    let handle = app.clone();
    let _ = app.run_on_main_thread(move || {
        use gtk::prelude::*;
        let dialog = gtk::FileChooserDialog::with_buttons(
            Some("Save Screenshots To"),
            None::<&gtk::Window>,
            gtk::FileChooserAction::SelectFolder,
            &[("Cancel", gtk::ResponseType::Cancel), ("Choose", gtk::ResponseType::Accept)],
        );
        dialog.set_create_folders(true);
        if let Some(dir) = current.filter(|d| std::path::Path::new(d).is_dir()) {
            let _ = dialog.set_current_folder(dir);
        }
        dialog.connect_response(move |d, response| {
            if response == gtk::ResponseType::Accept {
                if let Some(path) = d.filename() {
                    let path = path.display().to_string();
                    info!("screenshots: folder chosen: {path}");
                    let sent = setting_path_line("shots-dir", &path)
                        .ok_or_else(|| "that folder's name can't be used".to_string())
                        .and_then(|line| handle.state::<Compositor>().send_line("set", &line));
                    if let Err(e) = sent {
                        warn!("screenshots: {e}");
                    }
                }
            }
            d.close();
        });
        dialog.show_all();
    });
}

/// 10d-4: "Show in Files": open a folder (made first, if missing) in the file manager.
#[tauri::command]
pub fn open_folder(path: String) -> Result<(), String> {
    let dir = std::path::Path::new(&path);
    if !dir.is_absolute() {
        return Err("not a full path".into());
    }
    std::fs::create_dir_all(dir).map_err(|e| format!("{path}: {e}"))?;
    std::process::Command::new("xdg-open").arg(dir).spawn().map_err(|e| format!("xdg-open: {e}"))?;
    info!("opened {path}");
    Ok(())
}

/// 11c: the last `settings` message (null before kompact sent one).
#[tauri::command]
pub fn kompact_settings_state(c: State<'_, Compositor>) -> Option<Value> {
    c.settings.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

/// 10b: the last `keymap` message, for a page that loaded after it came (null before).
#[tauri::command]
pub fn keymap_state(c: State<'_, Compositor>) -> Option<Value> {
    c.keymap.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

/// 9e-c: the last `sharing` message, for a page that loaded after it came (null before).
#[tauri::command]
pub fn sharing_state(c: State<'_, Compositor>) -> Option<Value> {
    c.sharing.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

/// 6m-d3: ask kompact for thumbnails of these windows (the dock, while a preview is open).
/// With `KOMPACT_THUMBNAILS=off` kompact ignores it and the cards keep their icons.
#[tauri::command]
pub fn request_thumbs(c: State<'_, Compositor>, ids: Vec<u64>) -> Result<(), String> {
    for id in ids {
        c.send("thumb", Some(id))?;
    }
    Ok(())
}

/// From the pages: `activate`/`close`/`minimize` a window by id, `workspace` N, `layout`, or
/// `logout`.
#[tauri::command]
pub fn window_action(c: State<'_, Compositor>, action: String, id: Option<u64>) -> Result<(), String> {
    c.send(&action, id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sharing_messages() {
        let on: Value = serde_json::from_str(r#"{"type":"sharing","on":true,"outputs":["VGA-1"]}"#).unwrap();
        assert_eq!(shared_outputs(&on), ["VGA-1"]);
        let off: Value = serde_json::from_str(r#"{"type":"sharing","on":false,"outputs":[]}"#).unwrap();
        assert!(shared_outputs(&off).is_empty());
        let odd: Value = serde_json::from_str(r#"{"type":"sharing","on":true}"#).unwrap();
        assert!(shared_outputs(&odd).is_empty());
    }

    #[test]
    fn setting_path_lines() {
        assert_eq!(setting_path_line("shots-dir", "/home/u/My Shots").as_deref(), Some("set-path shots-dir /home/u/My Shots\n"));
        assert_eq!(setting_path_line("shots-dir", "").as_deref(), Some("set-path shots-dir \n"));
        assert_eq!(setting_path_line("shots-dir", "relative"), None);
        assert_eq!(setting_path_line("shots-dir", "/a\nlogout"), None);
        assert_eq!(setting_path_line("Shots", "/a"), None);
    }

    #[test]
    fn setting_lines() {
        assert_eq!(setting_line("animations", "off").as_deref(), Some("set animations off\n"));
        assert_eq!(setting_line("title-double-click", "minimize").as_deref(), Some("set title-double-click minimize\n"));
        assert_eq!(setting_line("animations", "off\nlogout"), None);
        assert_eq!(setting_line("", "on"), None);
        assert_eq!(setting_line("a b", "on"), None);
        assert_eq!(setting_line("layouts", "us,de(nodeadkeys)").as_deref(), Some("set layouts us,de(nodeadkeys)\n"));
        assert_eq!(setting_line("pointer-speed", "-0.3").as_deref(), Some("set pointer-speed -0.3\n"));
        assert_eq!(setting_line("layouts", "us de"), None);
    }

    #[test]
    fn display_move_lines() {
        assert_eq!(display_move_line("HDMI-A-1", -1366, 132).as_deref(), Some("display-move HDMI-A-1 -1366 132\n"));
        assert_eq!(display_move_line("VGA-1\nlogout", 0, 0), None);
        assert_eq!(display_move_line("", 0, 0), None);
        assert_eq!(display_move_line("VGA-1", 0, 200_000), None);
    }

    #[test]
    fn dock_icons_lines() {
        assert_eq!(dock_icons_line(50, &[[3, 120, 200], [7, 178, 200]]).as_deref(), Some("dock-icons 50 3:120:200 7:178:200\n"));
        assert_eq!(dock_icons_line(50, &[]).as_deref(), Some("dock-icons 50\n"));
        assert_eq!(dock_icons_line(0, &[]), None);
        assert_eq!(dock_icons_line(50, &[[-1, 0, 0]]), None);
        assert_eq!(dock_icons_line(50, &vec![[1, 0, 0]; 257]), None);
    }

    #[test]
    fn request_lines() {
        assert_eq!(request_line("activate", Some(3)).as_deref(), Some("activate 3\n"));
        assert_eq!(request_line("minimize", Some(1)).as_deref(), Some("minimize 1\n"));
        assert_eq!(request_line("thumb", Some(4)).as_deref(), Some("thumb 4\n"));
        assert_eq!(request_line("workspace", Some(2)).as_deref(), Some("workspace 2\n"));
        assert_eq!(request_line("workspace", None), None);
        assert_eq!(request_line("layout", None).as_deref(), Some("layout\n"));
        assert_eq!(request_line("switcher-open", None).as_deref(), Some("switcher open\n"));
        assert_eq!(request_line("switcher-cancel", None).as_deref(), Some("switcher cancel\n"));
        assert_eq!(request_line("switcher-select", Some(3)).as_deref(), Some("switcher select 3\n"));
        assert_eq!(request_line("switcher-pick", Some(9)).as_deref(), Some("switcher pick 9\n"));
        assert_eq!(request_line("switcher-pick", None), None);
        assert_eq!(request_line("switcher-open", Some(1)), None);
        assert_eq!(request_line("logout", None).as_deref(), Some("logout\n"));
        assert_eq!(request_line("logout", Some(1)), None);
        assert_eq!(request_line("lock", None).as_deref(), Some("lock\n"));
        assert_eq!(request_line("token", None).as_deref(), Some("token\n"));
        assert_eq!(request_line("activate", None), None);
        assert_eq!(request_line("rm", Some(1)), None);
        assert_eq!(shot_line(1466, 50, 300, 200).as_deref(), Some("shot 1466 50 300 200\n"));
        assert_eq!(shot_line(0, 0, 0, 10), None);
    }

    #[test]
    fn enriching() {
        let app = App {
            id: "org.gnome.gedit.desktop".into(),
            name: "Text Editor".into(),
            comment: String::new(),
            keywords: String::new(),
            exec: "gedit".into(),
            try_exec: None,
            icon: String::new(),
            wm_class: String::new(),
            mime_types: String::new(),
            categories: String::new(),
        };
        let msg = serde_json::json!({"type":"windows","workspace":1,"windows":[
            {"id":1,"app_id":"gedit","title":"notes.txt"},
            {"id":2,"app_id":"mystery","title":"?"},
            {"id":3,"app_id":"","title":"Untitled"}]});
        let out = enrich(msg, &[app]);
        let w = out["windows"].as_array().unwrap();
        assert_eq!(w[0]["name"], "Text Editor");
        assert_eq!(w[0]["desktop_id"], "org.gnome.gedit.desktop");
        assert_eq!(w[1]["name"], "mystery");
        assert!(w[1]["desktop_id"].is_null());
        assert_eq!(w[2]["name"], "Untitled");
        assert!(has_unknown(&out));
    }
}
