//! 6m-c: the private channel between kompact and its shell (kompact-shell).
//!
//! A Unix socket, `$XDG_RUNTIME_DIR/kompact-<WAYLAND_DISPLAY>.sock` (the runtime dir is the
//! user's alone, as for the Wayland socket itself). kompact sends one JSON object per line:
//!
//! - `{"type":"windows","workspace":1,"windows":[{"id":3,"app_id":"firefox","title":"…",
//!    "workspace":1,"minimized":false,"focused":true,"maximized":false,"fullscreen":false}, …]}`
//!   when a client connects and whenever
//!   that changes (windows bottom-to-top, then those parked or minimized; workspaces 1-based).
//!   5j-2: each window also has `"output":"VGA-1"` (left out when it's on none), and the message
//!   `"dock_output":"VGA-1"`, the output the shell's dock is on (left out without a dock);
//! - `{"type":"key","action":"spotlight"}` for shortcuts the shell handles (`Super+Space`);
//! - `{"type":"pointer_output","name":"VGA-1","x":1366,"y":0,"w":1440,"h":900}` (5j) when the
//!   output under the pointer changes (and to a shell that just connected), so the shell opens
//!   its overlay there;
//! - `{"type":"dock_to","output":"HDMIA-1"}` (5j-3, `dock_edge.rs`): the pointer was held at the
//!   bottom of that output; the shell remaps its dock so kompact puts it there.
//!
//! The shell sends one command per line: `activate <id>`, `close <id>`, `minimize <id>`,
//! `thumb <id>` (6m-d3, answered with a `thumb` message, see `thumbnail.rs`), `logout`, `lock` (8d),
//! `token` (8a: answered with `{"type":"token","token":"…"}`, an activation token for an app the
//! shell is about to start). Unknown lines are logged and ignored.
//!
//! 10b, keyboard shortcuts (`keymap.rs`): `keys` (answered with the `keymap` message, also sent
//! when a shell connects and after every change), `keys preset <macos|windows|classic> <keep|drop>`,
//! `keys bind <action> <keys>` (`Super+W`, `Shift+Super+3, Print`, `none`), `keys reset <action>`,
//! `keys reset-all`, `keys record` (the next combination comes back as `recorded` instead of
//! being acted on), `keys record-cancel`. Changes are answered with `keys_result`.
//!
//! Encoding and parsing are pure and tested here; the socket work is non-blocking and runs from
//! a 50 ms timer in `state.rs`, so a slow or dead shell never stalls the compositor (a client
//! that can't take a write is dropped).

use std::io::{ErrorKind, Read, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WindowInfo {
    pub id: u64,
    pub app_id: String,
    pub title: String,
    /// 1-based, as shown to the user.
    pub workspace: usize,
    pub minimized: bool,
    pub focused: bool,
    /// Fills the usable area / the whole output (6m-d2: the dock hides for these).
    pub maximized: bool,
    pub fullscreen: bool,
    /// 5j-2: the output its centre is on (empty: none), so the dock only hides for its own.
    pub output: String,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Request {
    Activate(u64),
    Close(u64),
    Minimize(u64),
    /// 6m-d3: a thumbnail of this window, please (`thumbnail.rs`).
    Thumb(u64),
    /// 6e: show workspace N (1-based, as in the window list), from the panel's indicator.
    Workspace(usize),
    /// 6e: where every window is (`encode_layout`), for the workspace overview. Asked for when
    /// it opens rather than sent with every window list, so a drag doesn't flood the shell.
    Layout,
    Logout,
    /// 8d: lock the screen (runs the lock command).
    Lock,
    /// 8a: an activation token for an app the shell starts.
    Token,
    /// 9c: a screenshot of this region (global logical coordinates), chosen in the shell.
    Shot { x: i32, y: i32, w: i32, h: i32 },
    /// 10b: the keyboard shortcuts.
    Keys(KeysRequest),
    /// 10d-1: the dock follows the pointer to another monitor (the Dock settings, 5j-3).
    DockFollow(bool),
    /// 10d-1c: the edge the dock is on.
    DockSide(crate::dock_edge::Side),
    /// 11a: where each window's dock icon is (centre, in the dock surface's own coordinates)
    /// and how big icons are, so minimizing can shrink a window into it.
    DockIcons { size: i32, icons: Vec<(u64, i32, i32)> },
    /// 11c: `set <key> <value>`, one of kompact's settings (`prefs.rs`); `set` alone asks for
    /// them all.
    Set(Option<(String, String)>),
    /// 10d-4: `set-path <key> <path…>`, a setting that's a folder (it may have spaces).
    SetPath(String, String),
    /// 12f: `keyboard-layout <n>`: use the n-th (0-based) configured layout.
    KeyboardLayout(u32),
    /// 12h-1: `displays`: where the monitors are (`encode_displays`).
    Displays,
    /// 12h-1: `display-move <name> <x> <y>`: Settings → Displays dropped a monitor there
    /// (global logical coordinates; kompact snaps it next to the others).
    DisplayMove { name: String, x: i32, y: i32 },
    /// 12h-2: `display-main <name>`: make it the main display.
    DisplayMain(String),
    /// 12h-2: `menu-bars every|primary`.
    MenuBars(bool),
    /// 12i-1b: `keep-awake [<app>,<app>…]`: the apps keeping the screen awake through the
    /// shell's org.freedesktop.ScreenSaver (D-Bus; the snap Firefox's way, via the portal);
    /// none: nothing.
    KeepAwake(Vec<String>),
}

/// 12i-1b: most apps `keep-awake` lists, and the longest name.
pub const MAX_AWAKE: usize = 16;
pub const MAX_AWAKE_NAME: usize = 64;

fn parse_keep_awake(rest: &str) -> Option<Request> {
    let names: Vec<String> = rest.split(',').map(str::trim).filter(|n| !n.is_empty()).map(str::to_string).collect();
    let ok = names.len() <= MAX_AWAKE && names.iter().all(|n| n.len() <= MAX_AWAKE_NAME && !n.contains(|c: char| c.is_control()));
    ok.then_some(Request::KeepAwake(names))
}

/// At most this many windows in one `dock-icons` line.
pub const MAX_DOCK_ICONS: usize = 256;

/// `dock-icons <size> <id>:<x>:<y> …` (no icons: the dock has none with windows).
fn parse_dock_icons(rest: &str) -> Option<Request> {
    let mut words = rest.split_whitespace();
    let size: i32 = words.next()?.parse().ok().filter(|s| (8..=256).contains(s))?;
    let mut icons = Vec::new();
    for w in words {
        let mut parts = w.split(':');
        let id = parts.next()?.parse().ok()?;
        let x = parts.next()?.parse().ok()?;
        let y = parts.next()?.parse().ok()?;
        if parts.next().is_some() || icons.len() == MAX_DOCK_ICONS {
            return None;
        }
        icons.push((id, x, y));
    }
    Some(Request::DockIcons { size, icons })
}

/// 10b: what the settings app asks about the shortcuts.
#[derive(Debug, PartialEq, Eq)]
pub enum KeysRequest {
    Get,
    Preset { preset: String, keep: bool },
    Bind { action: String, keys: String },
    Reset(String),
    ResetAll,
    Record,
    RecordCancel,
}

fn parse_keys(rest: &str) -> Option<KeysRequest> {
    let rest = rest.trim();
    let (cmd, args) = rest.split_once(char::is_whitespace).map(|(c, a)| (c, a.trim())).unwrap_or((rest, ""));
    let one_word = |a: &str| (!a.is_empty() && !a.contains(char::is_whitespace)).then(|| a.to_string());
    Some(match cmd {
        "" => KeysRequest::Get,
        "preset" => {
            let mut w = args.split_whitespace();
            let preset = w.next()?.to_string();
            let keep = match w.next()? {
                "keep" => true,
                "drop" => false,
                _ => return None,
            };
            if w.next().is_some() {
                return None;
            }
            KeysRequest::Preset { preset, keep }
        }
        "bind" => {
            let (action, keys) = args.split_once(char::is_whitespace)?;
            let keys = keys.trim();
            if keys.is_empty() {
                return None;
            }
            KeysRequest::Bind { action: action.to_string(), keys: keys.to_string() }
        }
        "reset" => KeysRequest::Reset(one_word(args)?),
        "reset-all" if args.is_empty() => KeysRequest::ResetAll,
        "record" if args.is_empty() => KeysRequest::Record,
        "record-cancel" if args.is_empty() => KeysRequest::RecordCancel,
        _ => return None,
    })
}

/// 10b: every shortcut, for the settings app: `{"type":"keymap","preset":"macos","presets":[…],
/// "groups":[…],"actions":[{"id","label","group","keys":[…],"preset_keys":[…],"changed",
/// "family","required"}],"fixed":[…]}`.
pub fn encode_keymap(map: &crate::keymap::Keymap) -> String {
    use crate::keymap::{Combo, ACTIONS, GROUPS, PRESETS};
    let list = |c: &[Combo]| c.iter().map(|c| json_string(&c.to_string())).collect::<Vec<_>>().join(",");
    let presets: Vec<String> = PRESETS
        .iter()
        .map(|p| format!("{{\"id\":{},\"label\":{}}}", json_string(p.id()), json_string(p.label())))
        .collect();
    let groups: Vec<String> = GROUPS.iter().map(|g| json_string(g.label())).collect();
    let actions: Vec<String> = ACTIONS
        .iter()
        .map(|&a| {
            format!(
                "{{\"id\":{},\"label\":{},\"group\":{},\"keys\":[{}],\"preset_keys\":[{}],\"changed\":{},\"family\":{},\"required\":{}}}",
                json_string(a.id()),
                json_string(a.label()),
                json_string(a.group().label()),
                list(map.combos(a)),
                list(&map.preset().combos(a)),
                map.is_changed(a),
                a.is_family(),
                a.required()
            )
        })
        .collect();
    format!(
        "{{\"type\":\"keymap\",\"preset\":{},\"presets\":[{}],\"groups\":[{}],\"actions\":[{}],\"fixed\":[\"Ctrl+Alt+F1-F12\"]}}\n",
        json_string(map.preset().id()),
        presets.join(","),
        groups.join(","),
        actions.join(",")
    )
}

/// 10b: how a `keys …` request went: `{"type":"keys_result","request":"bind window.close Super+W","ok":true,"message":"…"}`.
pub fn encode_keys_result(request: &str, ok: bool, message: &str) -> String {
    format!(
        "{{\"type\":\"keys_result\",\"request\":{},\"ok\":{ok},\"message\":{}}}\n",
        json_string(request),
        json_string(message)
    )
}

/// 10b: a recorded combination (`{"type":"recorded","keys":"Ctrl+Super+Q"}`) or why there's none
/// (`{"type":"recorded","cancelled":"Esc"}`).
pub fn encode_recorded(r: &crate::keymap::Recorded) -> String {
    match r {
        crate::keymap::Recorded::Combo(c) => format!("{{\"type\":\"recorded\",\"keys\":{}}}\n", json_string(&c.to_string())),
        crate::keymap::Recorded::Cancelled(why) => format!("{{\"type\":\"recorded\",\"cancelled\":{}}}\n", json_string(why)),
    }
}

/// 6e: a window's place, global logical coordinates (its parked spot if on another workspace).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Placed {
    pub id: u64,
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

/// `{"type":"layout","output":{x,y,w,h},"windows":[{id,x,y,w,h},...]}`, windows in the same
/// order as the window list (on screen bottom-to-top, then parked and minimized).
pub fn encode_layout(output: (i32, i32, i32, i32), windows: &[Placed]) -> String {
    let list: Vec<String> = windows
        .iter()
        .map(|p| format!("{{\"id\":{},\"x\":{},\"y\":{},\"w\":{},\"h\":{}}}", p.id, p.x, p.y, p.w, p.h))
        .collect();
    let (x, y, w, h) = output;
    format!(
        "{{\"type\":\"layout\",\"output\":{{\"x\":{x},\"y\":{y},\"w\":{w},\"h\":{h}}},\"windows\":[{}]}}\n",
        list.join(",")
    )
}

pub fn parse_request(line: &str) -> Option<Request> {
    // 10b: `keys …` takes free text (`Shift+Super+3, Print`).
    let trimmed = line.trim();
    if trimmed == "keys" || trimmed.starts_with("keys ") {
        return parse_keys(&trimmed[4..]).map(Request::Keys);
    }
    if let Some(rest) = trimmed.strip_prefix("dock-icons ") {
        return parse_dock_icons(rest);
    }
    if trimmed == "keep-awake" {
        return Some(Request::KeepAwake(Vec::new()));
    }
    if let Some(rest) = trimmed.strip_prefix("keep-awake ") {
        return parse_keep_awake(rest);
    }
    if let Some(rest) = trimmed.strip_prefix("set-path ") {
        let (key, path) = rest.split_once(' ').unwrap_or((rest, ""));
        let ok = !key.is_empty() && key.len() <= 40 && path.len() <= crate::prefs::MAX_PATH;
        return ok.then(|| Request::SetPath(key.to_string(), path.trim().to_string()));
    }
    let mut words = line.split_whitespace();
    let cmd = words.next()?;
    let id = || -> Option<u64> { words.clone().next()?.parse().ok() };
    let req = match cmd {
        "activate" => Request::Activate(id()?),
        "close" => Request::Close(id()?),
        "minimize" => Request::Minimize(id()?),
        "thumb" => Request::Thumb(id()?),
        "workspace" => {
            let n = usize::try_from(id()?).ok().filter(|n| (1..=crate::workspace::COUNT).contains(n))?;
            Request::Workspace(n)
        }
        "logout" => Request::Logout,
        "lock" => Request::Lock,
        "token" => Request::Token,
        "layout" => Request::Layout,
        "displays" => Request::Displays,
        "display-main" => {
            let name = words.clone().next()?;
            let ok = name.len() <= 32 && name.bytes().all(|b| b.is_ascii_alphanumeric() || b"-_".contains(&b));
            Request::DisplayMain(ok.then(|| name.to_string())?)
        }
        "menu-bars" => match words.clone().next()? {
            "every" => Request::MenuBars(true),
            "primary" => Request::MenuBars(false),
            _ => return None,
        },
        "display-move" => {
            let rest: Vec<&str> = words.by_ref().collect();
            let [name, x, y] = rest[..] else { return None };
            let (x, y) = (x.parse::<i32>().ok()?, y.parse::<i32>().ok()?);
            let ok = name.len() <= 32 && x.abs() <= 100_000 && y.abs() <= 100_000;
            return ok.then(|| Request::DisplayMove { name: name.to_string(), x, y });
        }
        "set" => {
            let rest: Vec<&str> = words.by_ref().collect();
            return match rest[..] {
                [] => Some(Request::Set(None)),
                [k, v] if k.len() <= 40 && v.len() <= 160 => Some(Request::Set(Some((k.to_string(), v.to_string())))),
                _ => None,
            };
        }
        "keyboard-layout" => Request::KeyboardLayout(id().and_then(|n| u32::try_from(n).ok())?),
        "dock-side" => Request::DockSide(crate::dock_edge::Side::from_name(words.clone().next()?)?),
        "dock-follow" => match words.clone().next()? {
            "on" => Request::DockFollow(true),
            "off" => Request::DockFollow(false),
            _ => return None,
        },
        "shot" => {
            let n: Vec<i32> = words.by_ref().map(str::parse).collect::<Result<_, _>>().ok()?;
            match n[..] {
                [x, y, w, h] if w > 0 && h > 0 => Request::Shot { x, y, w, h },
                _ => return None,
            }
        }
        _ => return None,
    };
    // Exactly the words the command takes.
    let expected = match req {
        Request::Logout | Request::Layout | Request::Lock | Request::Token | Request::Displays => 1,
        Request::Shot { .. } => 5,
        _ => 2,
    };
    (line.split_whitespace().count() == expected).then_some(req)
}

pub fn encode_windows(active_workspace: usize, windows: &[WindowInfo], dock_output: Option<&str>) -> String {
    let list: Vec<String> = windows
        .iter()
        .map(|w| {
            let output = if w.output.is_empty() { String::new() } else { format!(",\"output\":{}", json_string(&w.output)) };
            format!(
                "{{\"id\":{},\"app_id\":{},\"title\":{},\"workspace\":{},\"minimized\":{},\"focused\":{},\"maximized\":{},\"fullscreen\":{}{output}}}",
                w.id,
                json_string(&w.app_id),
                json_string(&w.title),
                w.workspace,
                w.minimized,
                w.focused,
                w.maximized,
                w.fullscreen
            )
        })
        .collect();
    let dock = dock_output.map(|d| format!(",\"dock_output\":{}", json_string(d))).unwrap_or_default();
    format!("{{\"type\":\"windows\",\"workspace\":{}{dock},\"windows\":[{}]}}\n", active_workspace, list.join(","))
}

/// 5j: the output under the pointer, global logical coordinates.
pub fn encode_pointer_output(name: &str, (x, y, w, h): (i32, i32, i32, i32)) -> String {
    format!("{{\"type\":\"pointer_output\",\"name\":{},\"x\":{x},\"y\":{y},\"w\":{w},\"h\":{h}}}\n", json_string(name))
}

/// 12i-2: `{"type":"lock","locked":true}`: the screen is locked (sent once the lock is
/// confirmed, and on unlock), so the shell can let the machine sleep.
pub fn encode_lock(locked: bool) -> String {
    format!("{{\"type\":\"lock\",\"locked\":{locked}}}\n")
}

/// 12i-1: `{"type":"idle","awake":["firefox"]}`: the apps keeping the screen awake.
pub fn encode_idle(apps: &[String]) -> String {
    let list: Vec<String> = apps.iter().map(|a| json_string(a)).collect();
    format!("{{\"type\":\"idle\",\"awake\":[{}]}}\n", list.join(","))
}

/// 12h-1: one monitor for Settings → Displays.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DisplayInfo {
    pub name: String,
    /// Global logical position and size.
    pub rect: (i32, i32, i32, i32),
    pub refresh_mhz: i32,
    /// Physical size in millimetres (0: unknown).
    pub mm: (i32, i32),
}

/// 12h-1: `{"type":"displays","outputs":[{"name":"VGA-1","x":0,"y":0,"w":1440,"h":900,"refresh":60000,"mm_w":410,"mm_h":260}],"primary":"VGA-1","menu_bars":"primary"}`
/// (12h-2: the main display, and `menu_bars` `every` or `primary`).
pub fn encode_displays(outputs: &[DisplayInfo], primary: &str, every_menu_bar: bool) -> String {
    let list: Vec<String> = outputs
        .iter()
        .map(|d| {
            let (x, y, w, h) = d.rect;
            format!(
                "{{\"name\":{},\"x\":{x},\"y\":{y},\"w\":{w},\"h\":{h},\"refresh\":{},\"mm_w\":{},\"mm_h\":{}}}",
                json_string(&d.name),
                d.refresh_mhz,
                d.mm.0,
                d.mm.1
            )
        })
        .collect();
    format!(
        "{{\"type\":\"displays\",\"outputs\":[{}],\"primary\":{},\"menu_bars\":\"{}\"}}\n",
        list.join(","),
        json_string(primary),
        if every_menu_bar { "every" } else { "primary" }
    )
}

/// 8a: an activation token for the shell.
pub fn encode_token(token: &str) -> String {
    format!("{{\"type\":\"token\",\"token\":{}}}\n", json_string(token))
}

/// 5j-3: move the dock to this output.
pub fn encode_dock_to(output: &str) -> String {
    format!("{{\"type\":\"dock_to\",\"output\":{}}}\n", json_string(output))
}

/// 9c: a screenshot was saved: `{"type":"screenshot","path":"…","width":…,"height":…}`.
/// 12f: `{"type":"keyboard","layouts":[{"code":"us","name":"English (US)"}…],"active":0}`.
pub fn encode_keyboard(codes: &[&str], names: &[String], active: u32) -> String {
    let list: Vec<String> = names
        .iter()
        .enumerate()
        .map(|(i, n)| format!("{{\"code\":{},\"name\":{}}}", json_string(codes.get(i).copied().unwrap_or("")), json_string(n)))
        .collect();
    format!("{{\"type\":\"keyboard\",\"layouts\":[{}],\"active\":{active}}}\n", list.join(","))
}

pub fn encode_screenshot(path: &str, width: i32, height: i32) -> String {
    format!("{{\"type\":\"screenshot\",\"path\":{},\"width\":{width},\"height\":{height}}}\n", json_string(path))
}

/// 9e-c: whether the screen is being shared (a screencopy stream is running), and which
/// monitors: `{"type":"sharing","on":true,"outputs":["VGA-1"]}`. The panel shows an icon.
pub fn encode_sharing(outputs: &[String]) -> String {
    let list = outputs.iter().map(|o| json_string(o)).collect::<Vec<_>>().join(",");
    format!("{{\"type\":\"sharing\",\"on\":{},\"outputs\":[{list}]}}\n", !outputs.is_empty())
}

pub fn encode_key(action: &str) -> String {
    format!("{{\"type\":\"key\",\"action\":{}}}\n", json_string(action))
}

/// A JSON string literal.
pub fn json_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

pub fn socket_path(runtime_dir: &Path, display: &str) -> PathBuf {
    runtime_dir.join(format!("kompact-{}.sock", display.replace('/', "_")))
}

/// Split complete lines off the front of `buf`, leaving a partial last line in it.
pub fn take_lines(buf: &mut Vec<u8>) -> Vec<String> {
    let Some(end) = buf.iter().rposition(|&b| b == b'\n') else { return Vec::new() };
    let rest = buf.split_off(end + 1);
    let lines = String::from_utf8_lossy(buf).lines().map(str::to_string).collect();
    *buf = rest;
    lines
}

/// The socket file, for `cleanup` on exit.
static PATH: Mutex<Option<PathBuf>> = Mutex::new(None);

/// Remove the socket file (called from `main` after the backend returns).
pub fn cleanup() {
    if let Some(path) = PATH.lock().ok().and_then(|mut p| p.take()) {
        let _ = std::fs::remove_file(path);
    }
}

struct Client {
    stream: UnixStream,
    buf: Vec<u8>,
}

/// The listening socket and connected shells.
#[derive(Default)]
pub struct Ipc {
    listener: Option<UnixListener>,
    tried: bool,
    clients: Vec<Client>,
    /// The last `windows` message sent, to send again only on change.
    last: String,
    /// 5j: the last `pointer_output` message sent, likewise.
    last_pointer_output: String,
    /// 9e-c: the last `sharing` message sent, likewise.
    last_sharing: String,
}

/// More than this unread from one client means it isn't speaking our protocol.
const MAX_LINE: usize = 4096;

impl Ipc {
    /// Start listening, once `WAYLAND_DISPLAY` is known. Logs and gives up for good on failure.
    pub fn ensure_listening(&mut self) {
        if self.tried {
            return;
        }
        let (Some(dir), Ok(display)) = (std::env::var_os("XDG_RUNTIME_DIR"), std::env::var("WAYLAND_DISPLAY")) else {
            return;
        };
        self.tried = true;
        let path = socket_path(Path::new(&dir), &display);
        // A leftover from a kompact that didn't exit cleanly; the display name is ours now.
        let _ = std::fs::remove_file(&path);
        match UnixListener::bind(&path).and_then(|l| l.set_nonblocking(true).map(|_| l)) {
            Ok(l) => {
                tracing::info!("shell channel listening on {}", path.display());
                self.listener = Some(l);
                if let Ok(mut p) = PATH.lock() {
                    *p = Some(path);
                }
            }
            Err(e) => tracing::warn!("shell channel: can't listen on {}: {e}", path.display()),
        }
    }

    /// Accept new shells. True if one connected (it needs the current state).
    pub fn accept(&mut self) -> bool {
        let Some(listener) = &self.listener else { return false };
        let mut any = false;
        loop {
            match listener.accept() {
                Ok((stream, _)) => {
                    if stream.set_nonblocking(true).is_ok() {
                        tracing::info!("shell channel: shell connected ({} now)", self.clients.len() + 1);
                        self.clients.push(Client { stream, buf: Vec::new() });
                        any = true;
                    }
                }
                Err(e) if e.kind() == ErrorKind::WouldBlock => break,
                Err(e) => {
                    tracing::warn!("shell channel: accept failed: {e}");
                    break;
                }
            }
        }
        if any {
            self.last.clear();
            self.last_pointer_output.clear();
            self.last_sharing.clear();
        }
        any
    }

    /// Read what the shells sent; drops those that hung up or misbehave.
    pub fn read_requests(&mut self) -> Vec<Request> {
        let mut out = Vec::new();
        self.clients.retain_mut(|c| {
            let mut chunk = [0u8; 1024];
            loop {
                match c.stream.read(&mut chunk) {
                    Ok(0) => {
                        tracing::info!("shell channel: shell disconnected");
                        return false;
                    }
                    Ok(n) => c.buf.extend_from_slice(&chunk[..n]),
                    Err(e) if e.kind() == ErrorKind::WouldBlock => break,
                    Err(e) => {
                        tracing::info!("shell channel: shell gone ({e})");
                        return false;
                    }
                }
            }
            for line in take_lines(&mut c.buf) {
                match parse_request(&line) {
                    Some(r) => out.push(r),
                    None => tracing::warn!("shell channel: ignoring {line:?}"),
                }
            }
            if c.buf.len() > MAX_LINE {
                tracing::warn!("shell channel: dropping a client sending {} bytes without a newline", c.buf.len());
                return false;
            }
            true
        });
        out
    }

    pub fn has_clients(&self) -> bool {
        !self.clients.is_empty()
    }

    /// Send `windows` if it differs from what was sent last.
    pub fn send_windows_if_changed(&mut self, message: String) {
        if message != self.last {
            self.broadcast(&message);
            self.last = message;
        }
    }

    /// 9e-c: send `sharing` if it differs from what was sent last (or a shell just connected).
    /// Returns whether it was sent.
    pub fn send_sharing_if_changed(&mut self, message: String) -> bool {
        if message == self.last_sharing {
            return false;
        }
        self.broadcast(&message);
        self.last_sharing = message;
        true
    }

    /// 5j: send `pointer_output` if it differs from what was sent last.
    pub fn send_pointer_output_if_changed(&mut self, message: String) {
        if message != self.last_pointer_output {
            self.broadcast(&message);
            self.last_pointer_output = message;
        }
    }

    pub fn broadcast(&mut self, message: &str) {
        self.clients.retain_mut(|c| match c.stream.write_all(message.as_bytes()) {
            Ok(()) => true,
            Err(e) => {
                tracing::info!("shell channel: dropping a shell that can't take writes ({e})");
                false
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shot_requests() {
        assert_eq!(parse_request("shot 1466 100 300 200"), Some(Request::Shot { x: 1466, y: 100, w: 300, h: 200 }));
        assert_eq!(parse_request("shot -5 0 10 10"), Some(Request::Shot { x: -5, y: 0, w: 10, h: 10 }));
        assert_eq!(parse_request("shot 1 2 0 4"), None); // empty
        assert_eq!(parse_request("shot 1 2 3"), None);
        assert_eq!(parse_request("shot 1 2 3 4 5"), None);
        assert_eq!(parse_request("shot a 2 3 4"), None);
        assert_eq!(
            encode_screenshot("/home/u/Pictures/Screenshots/Screenshot 2026-10-11 at 09.03.07.png", 300, 200),
            "{\"type\":\"screenshot\",\"path\":\"/home/u/Pictures/Screenshots/Screenshot 2026-10-11 at 09.03.07.png\",\"width\":300,\"height\":200}\n"
        );
    }

    #[test]
    fn pointer_output_message() {
        let msg = encode_pointer_output("VGA-1", (1366, 0, 1440, 900));
        assert_eq!(msg, "{\"type\":\"pointer_output\",\"name\":\"VGA-1\",\"x\":1366,\"y\":0,\"w\":1440,\"h\":900}\n");
    }

    #[test]
    fn requests() {
        assert_eq!(parse_request("activate 3"), Some(Request::Activate(3)));
        assert_eq!(parse_request(" close 12 "), Some(Request::Close(12)));
        assert_eq!(parse_request("minimize 1"), Some(Request::Minimize(1)));
        assert_eq!(parse_request("logout"), Some(Request::Logout));
        assert_eq!(parse_request("thumb 12"), Some(Request::Thumb(12)));
        assert_eq!(parse_request("thumb"), None);
        assert_eq!(parse_request("workspace 2"), Some(Request::Workspace(2)));
        assert_eq!(parse_request("workspace 9"), Some(Request::Workspace(9)));
        assert_eq!(parse_request("workspace 0"), None);
        assert_eq!(parse_request("workspace 10"), None);
        assert_eq!(parse_request("layout"), Some(Request::Layout));
        assert_eq!(parse_request("lock"), Some(Request::Lock));
        assert_eq!(parse_request("lock now"), None);
        assert_eq!(parse_request("token"), Some(Request::Token));
        assert_eq!(parse_request("layout 1"), None);
        for bad in ["", "activate", "activate x", "activate 3 4", "logout now", "rm -rf /", "close -1"] {
            assert_eq!(parse_request(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn windows_message() {
        let w = WindowInfo {
            id: 3,
            app_id: "firefox".into(),
            title: "Say \"hi\"\n".into(),
            workspace: 2,
            minimized: false,
            focused: true,
            maximized: true,
            fullscreen: false,
            output: String::new(),
        };
        assert_eq!(
            encode_windows(1, &[w.clone()], None),
            "{\"type\":\"windows\",\"workspace\":1,\"windows\":[{\"id\":3,\"app_id\":\"firefox\",\"title\":\"Say \\\"hi\\\"\\n\",\"workspace\":2,\"minimized\":false,\"focused\":true,\"maximized\":true,\"fullscreen\":false}]}\n"
        );
        assert_eq!(encode_windows(4, &[], None), "{\"type\":\"windows\",\"workspace\":4,\"windows\":[]}\n");
        // 5j-2: with outputs.
        let on_hdmi = WindowInfo { output: "HDMIA-1".into(), ..w };
        let msg = encode_windows(1, &[on_hdmi], Some("VGA-1"));
        assert!(msg.starts_with("{\"type\":\"windows\",\"workspace\":1,\"dock_output\":\"VGA-1\",\"windows\":[{\"id\":3,"), "{msg}");
        assert!(msg.ends_with("\"fullscreen\":false,\"output\":\"HDMIA-1\"}]}\n"), "{msg}");
    }

    #[test]
    fn token_message() {
        assert_eq!(encode_token("abc"), "{\"type\":\"token\",\"token\":\"abc\"}\n");
    }

    #[test]
    fn dock_to_message() {
        assert_eq!(encode_dock_to("HDMIA-1"), "{\"type\":\"dock_to\",\"output\":\"HDMIA-1\"}\n");
    }

    #[test]
    fn key_message() {
        assert_eq!(encode_key("spotlight"), "{\"type\":\"key\",\"action\":\"spotlight\"}\n");
    }

    #[test]
    fn json_escapes() {
        assert_eq!(json_string("a\\b\t\u{1}é"), "\"a\\\\b\\t\\u0001é\"");
    }

    #[test]
    fn lines() {
        let mut buf = b"activate 1\nclose 2\nmini".to_vec();
        assert_eq!(take_lines(&mut buf), vec!["activate 1", "close 2"]);
        assert_eq!(buf, b"mini");
        assert!(take_lines(&mut buf).is_empty());
        buf.extend_from_slice(b"mize 3\n");
        assert_eq!(take_lines(&mut buf), vec!["minimize 3"]);
        assert!(buf.is_empty());
    }

    #[test]
    fn path() {
        assert_eq!(socket_path(Path::new("/run/user/1000"), "wayland-1"), PathBuf::from("/run/user/1000/kompact-wayland-1.sock"));
    }

    #[test]
    fn socket_round_trip() {
        // A real socket pair: what a shell writes comes out as requests; broadcasts reach it.
        let (ours, mut theirs) = UnixStream::pair().unwrap();
        ours.set_nonblocking(true).unwrap();
        let mut ipc = Ipc { clients: vec![Client { stream: ours, buf: Vec::new() }], ..Default::default() };
        theirs.write_all(b"activate 7\nlogout\n").unwrap();
        assert_eq!(ipc.read_requests(), vec![Request::Activate(7), Request::Logout]);
        ipc.send_windows_if_changed(encode_windows(1, &[], None));
        ipc.send_windows_if_changed(encode_windows(1, &[], None)); // unchanged: not sent again
        drop(ipc);
        let mut got = String::new();
        theirs.read_to_string(&mut got).unwrap();
        assert_eq!(got, encode_windows(1, &[], None));
    }

    #[test]
    fn keyboard_messages() {
        assert_eq!(
            encode_keyboard(&["us", "de(nodeadkeys)"], &["English (US)".into(), "German (no dead keys)".into()], 1),
            "{\"type\":\"keyboard\",\"layouts\":[{\"code\":\"us\",\"name\":\"English (US)\"},{\"code\":\"de(nodeadkeys)\",\"name\":\"German (no dead keys)\"}],\"active\":1}\n"
        );
        assert_eq!(parse_request("keyboard-layout 1"), Some(Request::KeyboardLayout(1)));
        assert_eq!(parse_request("keyboard-layout x"), None);
        assert_eq!(parse_request("displays"), Some(Request::Displays));
        assert_eq!(parse_request("displays x"), None);
        assert_eq!(parse_request("display-move VGA-1 -1366 132"), Some(Request::DisplayMove { name: "VGA-1".into(), x: -1366, y: 132 }));
        assert_eq!(parse_request("display-move VGA-1 0"), None);
        assert_eq!(parse_request("display-move VGA-1 0 0 0"), None);
        assert_eq!(parse_request("display-move VGA-1 0 999999"), None);
        let d = DisplayInfo { name: "VGA-1".into(), rect: (0, 0, 1440, 900), refresh_mhz: 60000, mm: (410, 260) };
        assert_eq!(
            encode_displays(&[d], "VGA-1", true),
            "{\"type\":\"displays\",\"outputs\":[{\"name\":\"VGA-1\",\"x\":0,\"y\":0,\"w\":1440,\"h\":900,\"refresh\":60000,\"mm_w\":410,\"mm_h\":260}],\"primary\":\"VGA-1\",\"menu_bars\":\"every\"}\n"
        );
        assert_eq!(parse_request("display-main HDMIA-1"), Some(Request::DisplayMain("HDMIA-1".into())));
        assert_eq!(parse_request("display-main a;b"), None);
        assert_eq!(parse_request("menu-bars every"), Some(Request::MenuBars(true)));
        assert_eq!(parse_request("menu-bars primary"), Some(Request::MenuBars(false)));
        assert_eq!(parse_request("menu-bars some"), None);
        assert_eq!(encode_idle(&["firefox".into()]), "{\"type\":\"idle\",\"awake\":[\"firefox\"]}\n");
        assert_eq!(encode_lock(true), "{\"type\":\"lock\",\"locked\":true}\n");
        assert_eq!(encode_idle(&[]), "{\"type\":\"idle\",\"awake\":[]}\n");
    }

    #[test]
    fn set_requests() {
        assert_eq!(parse_request("set"), Some(Request::Set(None)));
        assert_eq!(parse_request("set animations off\n"), Some(Request::Set(Some(("animations".into(), "off".into())))));
        assert_eq!(parse_request("set animations"), None);
        assert_eq!(parse_request("set a b c"), None);
        assert_eq!(parse_request("set-path shots-dir /home/u/My Shots\n"), Some(Request::SetPath("shots-dir".into(), "/home/u/My Shots".into())));
        assert_eq!(parse_request("set-path shots-dir"), Some(Request::SetPath("shots-dir".into(), String::new())));
        // 12i-1b
        assert_eq!(parse_request("keep-awake Firefox (Playing video), mpv\n"), Some(Request::KeepAwake(vec!["Firefox (Playing video)".into(), "mpv".into()])));
        assert_eq!(parse_request("keep-awake\n"), Some(Request::KeepAwake(Vec::new())));
        assert_eq!(parse_request(&format!("keep-awake {}", "x".repeat(65))), None);
        assert_eq!(parse_request(&format!("keep-awake {}", vec!["a"; 17].join(","))), None);
    }

    #[test]
    fn dock_icons_requests() {
        assert_eq!(
            parse_request("dock-icons 50 3:120:200 7:178:200\n"),
            Some(Request::DockIcons { size: 50, icons: vec![(3, 120, 200), (7, 178, 200)] })
        );
        assert_eq!(parse_request("dock-icons 50"), Some(Request::DockIcons { size: 50, icons: vec![] }));
        assert_eq!(parse_request("dock-icons"), None);
        assert_eq!(parse_request("dock-icons 0 1:2:3"), None);
        assert_eq!(parse_request("dock-icons 50 1:2"), None);
        assert_eq!(parse_request("dock-icons 50 1:2:3:4"), None);
        assert_eq!(parse_request("dock-icons 50 x:2:3"), None);
        let many: String = (0..=MAX_DOCK_ICONS).map(|i| format!(" {i}:0:0")).collect();
        assert_eq!(parse_request(&format!("dock-icons 50{many}")), None);
    }

    #[test]
    fn dock_follow_requests() {
        assert_eq!(parse_request("dock-follow on"), Some(Request::DockFollow(true)));
        assert_eq!(parse_request("dock-follow off\n"), Some(Request::DockFollow(false)));
        assert_eq!(parse_request("dock-follow"), None);
        assert_eq!(parse_request("dock-follow maybe"), None);
        assert_eq!(parse_request("dock-follow on off"), None);
        assert_eq!(parse_request("dock-side left"), Some(Request::DockSide(crate::dock_edge::Side::Left)));
        assert_eq!(parse_request("dock-side top"), None);
    }

    #[test]
    fn keys_requests() {
        use KeysRequest as K;
        let k = |l: &str| parse_request(l);
        assert_eq!(k("keys"), Some(Request::Keys(K::Get)));
        assert_eq!(k("keys preset windows keep"), Some(Request::Keys(K::Preset { preset: "windows".into(), keep: true })));
        assert_eq!(k("keys preset macos drop"), Some(Request::Keys(K::Preset { preset: "macos".into(), keep: false })));
        assert_eq!(k("keys preset macos"), None);
        assert_eq!(k("keys preset macos maybe"), None);
        assert_eq!(
            k("keys bind screenshot.all Shift+Super+3, Print"),
            Some(Request::Keys(K::Bind { action: "screenshot.all".into(), keys: "Shift+Super+3, Print".into() }))
        );
        assert_eq!(k("keys bind window.close"), None);
        assert_eq!(k("keys reset window.close"), Some(Request::Keys(K::Reset("window.close".into()))));
        assert_eq!(k("keys reset"), None);
        assert_eq!(k("keys reset-all"), Some(Request::Keys(K::ResetAll)));
        assert_eq!(k("keys record"), Some(Request::Keys(K::Record)));
        assert_eq!(k("keys record-cancel"), Some(Request::Keys(K::RecordCancel)));
        assert_eq!(k("keys record now"), None);
        assert_eq!(k("keys dance"), None);
        assert_eq!(k("keysmith"), None);
    }

    #[test]
    fn keys_messages() {
        use crate::keymap::{parse_combo, Keymap, Preset, Recorded};
        let msg = encode_keymap(&Keymap::new(Preset::Windows));
        assert!(msg.starts_with("{\"type\":\"keymap\",\"preset\":\"windows\",\"presets\":[{\"id\":\"macos\",\"label\":\"macOS\"}"), "{msg}");
        assert!(msg.contains("{\"id\":\"window.close\",\"label\":\"Close the window\",\"group\":\"Windows\",\"keys\":[\"Alt+F4\"],\"preset_keys\":[\"Alt+F4\"],\"changed\":false,\"family\":false,\"required\":false}"), "{msg}");
        assert!(msg.contains("\"keys\":[\"Super+S\",\"Super\"]"), "{msg}");
        assert!(msg.ends_with("\"fixed\":[\"Ctrl+Alt+F1-F12\"]}\n"));
        assert_eq!(
            encode_keys_result("bind x y", false, "no action \"x\""),
            "{\"type\":\"keys_result\",\"request\":\"bind x y\",\"ok\":false,\"message\":\"no action \\\"x\\\"\"}\n"
        );
        assert_eq!(encode_recorded(&Recorded::Combo(parse_combo("Ctrl+Super+Q").unwrap())), "{\"type\":\"recorded\",\"keys\":\"Ctrl+Super+Q\"}\n");
        assert_eq!(encode_recorded(&Recorded::Cancelled("Esc")), "{\"type\":\"recorded\",\"cancelled\":\"Esc\"}\n");
    }

    #[test]
    fn sharing_messages() {
        assert_eq!(encode_sharing(&[]), "{\"type\":\"sharing\",\"on\":false,\"outputs\":[]}\n");
        assert_eq!(
            encode_sharing(&["HDMIA-1".into(), "VGA-1".into()]),
            "{\"type\":\"sharing\",\"on\":true,\"outputs\":[\"HDMIA-1\",\"VGA-1\"]}\n"
        );
        let (ours, mut theirs) = UnixStream::pair().unwrap();
        ours.set_nonblocking(true).unwrap();
        let mut ipc = Ipc { clients: vec![Client { stream: ours, buf: Vec::new() }], ..Default::default() };
        assert!(ipc.send_sharing_if_changed(encode_sharing(&[])));
        assert!(!ipc.send_sharing_if_changed(encode_sharing(&[]))); // unchanged: not sent again
        assert!(ipc.send_sharing_if_changed(encode_sharing(&["VGA-1".into()])));
        drop(ipc);
        let mut got = String::new();
        theirs.read_to_string(&mut got).unwrap();
        assert_eq!(got, encode_sharing(&[]) + &encode_sharing(&["VGA-1".into()]));
    }

    #[test]
    fn layout_message() {
        let p = Placed { id: 4, x: 10, y: -20, w: 300, h: 200 };
        assert_eq!(
            encode_layout((0, 0, 1440, 900), &[p]),
            "{\"type\":\"layout\",\"output\":{\"x\":0,\"y\":0,\"w\":1440,\"h\":900},\"windows\":[{\"id\":4,\"x\":10,\"y\":-20,\"w\":300,\"h\":200}]}\n"
        );
        assert_eq!(
            encode_layout((0, 0, 1, 1), &[]),
            "{\"type\":\"layout\",\"output\":{\"x\":0,\"y\":0,\"w\":1,\"h\":1},\"windows\":[]}\n"
        );
    }
}
