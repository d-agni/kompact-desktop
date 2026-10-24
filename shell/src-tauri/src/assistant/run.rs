//! 19c: Siva does what was asked: each step from `rules` goes through `policy::decide`, then runs
//! through the shell's own commands (the same ones the menus and Settings use), and what it
//! changed is kept so one click undoes it. A step that asks first waits for the page's
//! Confirm/Cancel (`assistant_answer`); the rest of the request waits with it.
//!
//! Matching a name to a window or an app (`best_window`, `best_app`) is pure and tested; the
//! rest talks to the shell.

use std::sync::Mutex;

use serde::Serialize;
use serde_json::Value;
use tauri::{AppHandle, Emitter, Manager};
use tracing::info;

use super::activity::{self, Kind};
use super::policy::{self, Decision, Proposal};
use super::rules::{self, Parsed, Step};
use crate::compositor::Compositor;

// ---- matching names (pure) ----

/// A window as the matching sees it.
#[derive(Debug, Clone, PartialEq)]
pub struct WinCand {
    pub id: u64,
    /// The app's name ("Firefox", "Text Editor").
    pub name: String,
    pub app_id: String,
    pub title: String,
    /// The app's keywords and comment ("terminal;shell;…"), to find Ptyxis by "terminal".
    pub about: String,
    pub focused: bool,
    pub minimized: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AppCand {
    pub id: String,
    pub name: String,
    pub about: String,
}

/// How well `query` names something with this name, id, keywords and title: 0 is not at all.
fn score(query: &str, name: &str, id: &str, about: &str, title: &str) -> u32 {
    let q = query.trim().to_lowercase();
    let (name, id, about, title) = (name.to_lowercase(), id.to_lowercase(), about.to_lowercase(), title.to_lowercase());
    if q.is_empty() {
        return 0;
    }
    if name == q || id == q || id.trim_end_matches(".desktop").rsplit('.').next() == Some(q.as_str()) {
        100
    } else if name.starts_with(&q) || id.contains(&q) {
        80
    } else if name.contains(&q) {
        70
    } else if about.split(|c: char| !c.is_alphanumeric()).any(|w| w == q) {
        60
    } else if title.contains(&q) {
        40
    } else if q.split_whitespace().all(|w| name.contains(w) || title.contains(w) || about.contains(w)) {
        30
    } else {
        0
    }
}

/// The window `query` names: the best match; among equals the focused one, then the first (the
/// list is front to back).
pub fn best_window<'a>(query: &str, windows: &'a [WinCand]) -> Option<&'a WinCand> {
    let mut best: Option<(&WinCand, u32)> = None;
    for w in windows {
        let s = score(query, &w.name, &w.app_id, &w.about, &w.title) * 2 + u32::from(w.focused);
        if s >= 2 && best.is_none_or(|(_, b)| s > b) {
            best = Some((w, s));
        }
    }
    best.map(|(w, _)| w)
}

pub fn best_app<'a>(query: &str, apps: &'a [AppCand]) -> Option<&'a AppCand> {
    let mut best: Option<(&AppCand, u32)> = None;
    for a in apps {
        let s = score(query, &a.name, &a.id, &a.about, "");
        if s > 0 && best.is_none_or(|(_, b)| s > b) {
            best = Some((a, s));
        }
    }
    best.map(|(a, _)| a)
}

/// What a confirmation card says, and its button.
pub fn confirm_text(tool: &str, args: &Value, window: Option<&str>) -> (String, String) {
    let on = args.get("on").and_then(Value::as_bool).unwrap_or(false);
    match tool {
        "close_window" => (format!("Close “{}”? It may have unsaved changes.", window.unwrap_or("this window")), "Close".into()),
        "wifi" if on => ("Turn Wi-Fi on?".into(), "Turn On".into()),
        "wifi" => ("Turn Wi-Fi off? You'll lose the wireless connection.".into(), "Turn Off".into()),
        "bluetooth" if on => ("Turn Bluetooth on?".into(), "Turn On".into()),
        "bluetooth" => ("Turn Bluetooth off? Its devices disconnect, your speaker too.".into(), "Turn Off".into()),
        "log_out" => ("Log out? Every app closes.".into(), "Log Out".into()),
        "power" => match args.get("action").and_then(Value::as_str) {
            Some("restart") => ("Restart the computer? Every app closes.".into(), "Restart".into()),
            Some("sleep") => ("Put the computer to sleep?".into(), "Sleep".into()),
            _ => ("Power off the computer? Every app closes.".into(), "Power Off".into()),
        },
        _ => ("Go ahead with this?".into(), "Go Ahead".into()),
    }
}

// ---- the replies ----

#[derive(Debug, Clone, Serialize)]
pub struct Reply {
    pub text: String,
    /// How it went, for the HUD's colour: `speaking` (an answer), `happy` (done), `concerned`
    /// (a card), `sorry` (failed or not understood), `idle` (nothing to show).
    pub mood: &'static str,
    /// An Undo button for what was done.
    pub undo: Option<u64>,
    pub confirm: Option<Card>,
    /// 19e: a request of several steps, each with how it went.
    pub plan: Option<Vec<PlanItem>>,
}

/// 19e: one line of a plan: `todo`, `now`, `done`, `failed`, `asks` (waits on a card),
/// `stopped`.
#[derive(Debug, Clone, Serialize)]
pub struct PlanItem {
    pub text: String,
    pub state: &'static str,
}

/// 19e: the plan of a request of two steps or more, shown live in the HUD
/// (`kompact-assistant-plan`). One step: no plan.
struct Plan<'a> {
    app: &'a AppHandle,
    items: Vec<PlanItem>,
}

impl<'a> Plan<'a> {
    /// Steps before `at` were done before (a request picked up again after its card).
    fn new(app: &'a AppHandle, steps: &[Step], at: usize) -> Plan<'a> {
        let items = if steps.len() < 2 {
            Vec::new()
        } else {
            steps.iter().enumerate().map(|(i, s)| PlanItem { text: rules::describe(s), state: if i < at { "done" } else { "todo" } }).collect()
        };
        Plan { app, items }
    }

    fn set(&mut self, i: usize, state: &'static str) {
        if let Some(item) = self.items.get_mut(i) {
            item.state = state;
            let _ = self.app.emit("kompact-assistant-plan", &self.items);
        }
    }

    /// The steps from `from` on that won't run now.
    fn rest(&mut self, from: usize, state: &'static str) {
        for item in self.items.iter_mut().skip(from) {
            if item.state == "todo" {
                item.state = state;
            }
        }
        if !self.items.is_empty() {
            let _ = self.app.emit("kompact-assistant-plan", &self.items);
        }
    }

    fn into_reply(self, r: Reply) -> Reply {
        Reply { plan: (!self.items.is_empty()).then_some(self.items), ..r }
    }
}

/// 19e: the HUD's Stop: the plan running now stops before its next step.
static STOP_PLAN: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

#[tauri::command]
pub fn assistant_stop() {
    info!("assistant: stop (the HUD)");
    STOP_PLAN.store(true, std::sync::atomic::Ordering::Relaxed);
}

#[derive(Debug, Clone, Serialize)]
pub struct Card {
    pub id: u64,
    pub text: String,
    pub button: String,
}

fn reply(text: impl Into<String>, mood: &'static str) -> Reply {
    Reply { text: text.into(), mood, undo: None, confirm: None, plan: None }
}

const HELP: &str = "Try: “volume 30”, “dark mode”, “open Firefox”, “put Firefox and Terminal side by side”, \
“next song”, “do not disturb”, “go to desktop 2”, “take a screenshot”, “lock the screen”, “why is my computer slow?”, “why is the sound bad?”, or a routine: “set up for coding”.";

// ---- undo ----

#[derive(Debug, Clone)]
enum Undo {
    Volume { volume: f64, muted: bool },
    Muted { output: bool, muted: bool },
    Output(u32),
    Activate(u64),
    Minimize(u64),
    Place(u64),
    Desktop(u64),
    Dnd(bool),
    Appearance(String),
    KeepAwake(bool),
    Wifi(bool),
    Bluetooth(bool),
}

/// A request waiting on a confirmation card: its steps, how far it got, what it said so far.
struct Waiting {
    id: u64,
    steps: Vec<Step>,
    at: usize,
    said: Vec<String>,
    undo: Vec<Undo>,
    /// 19d: shown again when a spoken "yes" can't answer it.
    card: Card,
    /// 19f: the card asks about a whole plan the model made ("Did you mean …?"), not one step:
    /// a click runs the plan, and its Tier 2 steps still ask on their own.
    whole_plan: bool,
}

#[derive(Default)]
pub struct Assistant {
    inner: Mutex<Inner>,
}

#[derive(Default)]
struct Inner {
    next: u64,
    waiting: Option<Waiting>,
    /// The last few requests' undo lists, newest last.
    undos: Vec<(u64, Vec<Undo>)>,
}

const KEEP_UNDOS: usize = 10;

impl Assistant {
    fn with<T>(&self, f: impl FnOnce(&mut Inner) -> T) -> T {
        f(&mut self.inner.lock().unwrap_or_else(|e| e.into_inner()))
    }

    fn id(&self) -> u64 {
        self.with(|i| {
            i.next += 1;
            i.next
        })
    }
}

// ---- reading the shell's state ----

fn windows(app: &AppHandle) -> Vec<WinCand> {
    let overlay = app.state::<crate::overlay::Overlay>();
    let state = app.state::<Compositor>().last_state();
    let list = state.as_ref().and_then(|s| s.get("windows")).and_then(Value::as_array).cloned().unwrap_or_default();
    list.iter()
        .filter_map(|w| {
            let desktop = w.get("desktop_id").and_then(Value::as_str);
            let about = desktop.and_then(|d| overlay.app_entry(d)).map(|a| format!("{} {}", a.keywords, a.comment)).unwrap_or_default();
            Some(WinCand {
                id: w.get("id")?.as_u64()?,
                name: w.get("name").and_then(Value::as_str).unwrap_or("").to_string(),
                app_id: w.get("app_id").and_then(Value::as_str).unwrap_or("").to_string(),
                title: w.get("title").and_then(Value::as_str).unwrap_or("").to_string(),
                about,
                focused: w.get("focused").and_then(Value::as_bool).unwrap_or(false),
                minimized: w.get("minimized").and_then(Value::as_bool).unwrap_or(false),
            })
        })
        .collect()
}

fn window_for(app: &AppHandle, query: &str) -> Result<WinCand, String> {
    let list = windows(app);
    best_window(query, &list).cloned().ok_or_else(|| format!("I can't find a window for “{query}”."))
}

/// What one request has done so far that later steps need.
#[derive(Default)]
struct Ctx {
    /// The windows there were before this request opened an app: "open files then put it on the
    /// left" means the *new* Files window, which takes a moment to appear.
    before_open: Option<Vec<u64>>,
}

/// How long a step waits for the window of an app this request opened.
const NEW_WINDOW_WAIT: std::time::Duration = std::time::Duration::from_secs(6);

/// The window `query` names; after an `open_app`, its new window, waited for.
async fn find_window(app: &AppHandle, query: &str, ctx: &Ctx) -> Result<WinCand, String> {
    if let Some(before) = &ctx.before_open {
        let start = std::time::Instant::now();
        while start.elapsed() < NEW_WINDOW_WAIT {
            let new: Vec<WinCand> = windows(app).into_iter().filter(|w| !before.contains(&w.id)).collect();
            if let Some(w) = best_window(query, &new) {
                info!("assistant: {query:?}'s new window after {} ms", start.elapsed().as_millis());
                return Ok(w.clone());
            }
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
    }
    window_for(app, query)
}

fn label(w: &WinCand) -> String {
    if w.title.is_empty() || w.title == w.name { w.name.clone() } else { format!("{} — {}", w.title, w.name) }
}

fn send(app: &AppHandle, action: &str, id: Option<u64>) -> Result<(), String> {
    app.state::<Compositor>().send(action, id)
}

fn place_line(app: &AppHandle, id: u64, to: &str) -> Result<(), String> {
    app.state::<Compositor>().send_line_pub("place", &format!("place {id} {to}\n"))
}

fn current_desktop(app: &AppHandle) -> Option<u64> {
    app.state::<Compositor>().last_state()?.get("workspace")?.as_u64()
}

async fn sound() -> Value {
    crate::sound::sound_state().await.unwrap_or(Value::Null)
}

fn notify_prefs(app: &AppHandle) -> crate::notify::NotifyPrefs {
    crate::notify::notify_prefs(app.state())
}

// ---- running one step ----

struct Done {
    text: String,
    undo: Option<Undo>,
}

fn done(text: impl Into<String>, undo: Option<Undo>) -> Result<Done, String> {
    Ok(Done { text: text.into(), undo })
}

async fn run_step(app: &AppHandle, s: &Step, ctx: &mut Ctx) -> Result<Done, String> {
    let a = &s.args;
    let str_arg = |k: &str| a.get(k).and_then(Value::as_str).unwrap_or("").to_string();
    let bool_arg = |k: &str| a.get(k).and_then(Value::as_bool).unwrap_or(false);
    let int_arg = |k: &str| a.get(k).and_then(Value::as_i64).unwrap_or(0);
    match s.tool {
        "list_windows" => {
            let list = windows(app);
            if list.is_empty() {
                return done("No windows are open.", None);
            }
            let names: Vec<String> = list.iter().map(|w| if w.minimized { format!("{} (minimized)", w.name) } else { w.name.clone() }).collect();
            done(format!("Open: {}.", names.join(", ")), None)
        }
        "sound_status" => {
            let st = sound().await;
            let out = &st["output"];
            let vol = (out["volume"].as_f64().unwrap_or(0.0) * 100.0).round();
            let device = out["devices"].as_array().and_then(|d| d.iter().find(|d| d["id"] == out["current"])).and_then(|d| d["nick"].as_str().or(d["label"].as_str())).unwrap_or("no output");
            let muted = if out["muted"].as_bool() == Some(true) { ", muted" } else { "" };
            done(format!("The volume is {vol}% on {device}{muted}."), None)
        }
        "network_status" => {
            let n = crate::network::network_summary().await.unwrap_or(Value::Null);
            match n["name"].as_str() {
                Some(name) => done(format!("You're connected ({name})."), None),
                None => done("You're not connected to a network.", None),
            }
        }
        "bluetooth_status" => {
            let b = crate::bluetooth::bluetooth_state().await.unwrap_or(Value::Null);
            if b["adapter"].is_null() {
                return done("There's no Bluetooth on this computer.", None);
            }
            if b["powered"].as_bool() != Some(true) {
                return done("Bluetooth is off.", None);
            }
            let on: Vec<&str> = b["devices"].as_array().map(|d| d.iter().filter(|d| d["connected"].as_bool() == Some(true)).filter_map(|d| d["name"].as_str()).collect()).unwrap_or_default();
            done(if on.is_empty() { "Bluetooth is on; nothing is connected.".to_string() } else { format!("Bluetooth is on: {}.", on.join(", ")) }, None)
        }
        // 19g: why things are as they are.
        "system_status" => {
            let load = tauri::async_runtime::spawn_blocking(super::diagnose::sample).await.ok().flatten().ok_or("I can't read the computer's load.")?;
            done(super::diagnose::load_text(&load), None)
        }
        "disk_space" => done(super::diagnose::disks_text(&super::diagnose::disks()), None),
        "sound_check" => {
            let (dump, load) = tauri::async_runtime::spawn_blocking(|| (crate::sound::dump_now(), super::diagnose::sample()))
                .await
                .map_err(|e| e.to_string())?;
            let dump = dump.map_err(|e| format!("I can't read the sound system: {e}"))?;
            done(super::diagnose::sound_text(&dump, load.map(|l| l.cpu)), None)
        }
        "now_playing" => {
            let p = crate::media::media_state().await.unwrap_or_default();
            match p.first() {
                Some(p) if !p.title.is_empty() => {
                    let by = if p.artist.is_empty() { String::new() } else { format!(" by {}", p.artist) };
                    let state = if p.status == "Playing" { "Playing" } else { "Paused" };
                    done(format!("{state}: {}{by} ({}).", p.title, p.name), None)
                }
                _ => done("Nothing is playing.", None),
            }
        }
        "set_volume" | "change_volume" => {
            let st = sound().await;
            let (before, muted) = (st["output"]["volume"].as_f64().unwrap_or(0.0), st["output"]["muted"].as_bool().unwrap_or(false));
            let to = if s.tool == "set_volume" { int_arg("percent") as f64 / 100.0 } else { (before + int_arg("by") as f64 / 100.0).clamp(0.0, 1.0) };
            crate::sound::sound_volume(true, to as f32).await?;
            done(format!("Volume {}%.", (to * 100.0).round()), Some(Undo::Volume { volume: before, muted }))
        }
        "mute" => {
            let output = str_arg("device") != "microphone";
            let st = sound().await;
            let side = if output { &st["output"] } else { &st["input"] };
            let was = side["muted"].as_bool().unwrap_or(false);
            let on = bool_arg("on");
            crate::sound::sound_mute(output, on).await?;
            let what = if output { "Sound" } else { "Microphone" };
            done(format!("{what} {}.", if on { "muted" } else { "on" }), Some(Undo::Muted { output, muted: was }))
        }
        "sound_output" => {
            let st = sound().await;
            let out = &st["output"];
            let want = str_arg("device").to_lowercase();
            let devices = out["devices"].as_array().cloned().unwrap_or_default();
            let d = devices
                .iter()
                .find(|d| [d["label"].as_str(), d["nick"].as_str()].iter().flatten().any(|n| n.to_lowercase().contains(&want)))
                .ok_or_else(|| format!("I can't find a sound output called “{}”.", str_arg("device")))?;
            let id = d["id"].as_u64().ok_or("that output has no id")? as u32;
            let before = out["current"].as_u64().map(|c| c as u32);
            crate::sound::sound_default(id).await?;
            done(format!("Sound now plays on {}.", d["nick"].as_str().or(d["label"].as_str()).unwrap_or("it")), before.map(Undo::Output))
        }
        "media" => {
            let players = crate::media::media_state().await.unwrap_or_default();
            let p = players.first().ok_or("Nothing is playing.")?;
            let action = str_arg("action");
            crate::media::media_control(p.bus.clone(), action.clone()).await?;
            let text = match action.as_str() {
                "next" => "Next one.".to_string(),
                "previous" => "Previous one.".to_string(),
                _ if p.status == "Playing" => format!("Paused {}.", p.name),
                _ => format!("Playing {}.", p.name),
            };
            done(text, None)
        }
        "open_app" => {
            let overlay = app.state::<crate::overlay::Overlay>();
            let apps: Vec<AppCand> = overlay.apps().into_iter().map(|a| AppCand { about: format!("{} {}", a.keywords, a.comment), id: a.id, name: a.name }).collect();
            let want = str_arg("app");
            let found = best_app(&want, &apps).ok_or_else(|| format!("I can't find an app called “{want}”."))?;
            // The first app opened keeps the list: windows from every app this request opens are new.
            if ctx.before_open.is_none() {
                ctx.before_open = Some(windows(app).iter().map(|w| w.id).collect());
            }
            if !overlay.launch_id(app, &found.id) {
                return Err(format!("{} didn't start.", found.name));
            }
            done(format!("Opening {}.", found.name), None)
        }
        "focus_window" => {
            let w = find_window(app, &str_arg("window"), ctx).await?;
            let before = windows(app).into_iter().find(|w| w.focused).map(|w| w.id);
            send(app, "activate", Some(w.id))?;
            // Undo puts a minimized one back in the dock, or brings back the one in front before.
            let undo = if w.minimized { Some(Undo::Minimize(w.id)) } else { before.filter(|b| *b != w.id).map(Undo::Activate) };
            done(format!("Here's {}.", w.name), undo)
        }
        "minimize_window" => {
            let w = find_window(app, &str_arg("window"), ctx).await?;
            send(app, "minimize", Some(w.id))?;
            done(format!("Minimized {}.", w.name), Some(Undo::Activate(w.id)))
        }
        "place_window" => {
            let w = find_window(app, &str_arg("window"), ctx).await?;
            let to = str_arg("where");
            place_line(app, w.id, &to)?;
            let text = match to.as_str() {
                "left" => format!("{} is on the left.", w.name),
                "right" => format!("{} is on the right.", w.name),
                "fill" => format!("{} fills the screen.", w.name),
                _ => format!("{} is back to its size.", w.name),
            };
            done(text, (to != "restore").then_some(Undo::Place(w.id)))
        }
        "switch_desktop" => {
            let before = current_desktop(app);
            let n = int_arg("number") as u64;
            send(app, "workspace", Some(n))?;
            done(format!("Desktop {n}."), before.filter(|b| *b != n).map(Undo::Desktop))
        }
        "do_not_disturb" => {
            let p = notify_prefs(app);
            let on = bool_arg("on");
            crate::notify::set_notify_prefs(app.clone(), app.state(), on, p.banner_ms, p.muted.clone())?;
            done(if on { "Do Not Disturb is on." } else { "Do Not Disturb is off." }, Some(Undo::Dnd(p.dnd)))
        }
        "appearance" => {
            let before = crate::compositor::kompact_settings_state(app.state()).and_then(|k| k["appearance"].as_str().map(str::to_string));
            let mode = str_arg("mode");
            crate::compositor::kompact_set(app.state(), "appearance".into(), mode.clone())?;
            let text = match mode.as_str() {
                "dark" => "Dark mode.",
                "light" => "Light mode.",
                _ => "Appearance follows the time of day.",
            };
            done(text, before.map(Undo::Appearance))
        }
        "keep_awake" => {
            let state = app.state::<crate::screensaver::ScreenSaver>();
            let was = crate::screensaver::keep_awake_state(app.state())["on"].as_bool().unwrap_or(false);
            let on = bool_arg("on");
            crate::screensaver::set_keep_awake(app.clone(), state, on);
            done(if on { "I'll keep the screen awake." } else { "The screen can sleep again." }, Some(Undo::KeepAwake(was)))
        }
        "screenshot" => {
            if str_arg("of") == "area" {
                let handle = app.clone();
                app.run_on_main_thread(move || crate::overlay::show_overlay(handle.clone(), handle.state(), "select".into())).map_err(|e| e.to_string())?;
                return done("Drag out the area.", None);
            }
            let d = crate::compositor::displays_state(app.state()).ok_or("I don't know the screens yet.")?;
            let main = d["primary"].as_str().unwrap_or("");
            let outputs = d["outputs"].as_array().cloned().unwrap_or_default();
            let o = outputs.iter().find(|o| o["name"] == main).or(outputs.first()).ok_or("There's no screen.")?;
            let n = |k: &str| o[k].as_i64().unwrap_or(0) as i32;
            app.state::<Compositor>().send_shot(n("x"), n("y"), n("w"), n("h"))?;
            done("Screenshot taken.", None)
        }
        "lock_screen" => {
            send(app, "lock", None)?;
            done("Locking.", None)
        }
        "close_window" => {
            let w = find_window(app, &str_arg("window"), ctx).await?;
            send(app, "close", Some(w.id))?;
            done(format!("Closing {}.", w.name), None)
        }
        "wifi" => {
            let was = crate::network::network_state().await.ok().and_then(|n| n["wifi_enabled"].as_bool());
            let on = bool_arg("on");
            crate::network::network_wifi(on).await?;
            done(if on { "Wi-Fi is on." } else { "Wi-Fi is off." }, was.map(Undo::Wifi))
        }
        "bluetooth" => {
            let was = crate::bluetooth::bluetooth_state().await.ok().and_then(|b| b["powered"].as_bool());
            let on = bool_arg("on");
            crate::bluetooth::bluetooth_power(on).await?;
            done(if on { "Bluetooth is on." } else { "Bluetooth is off." }, was.map(Undo::Bluetooth))
        }
        "log_out" => {
            send(app, "logout", None)?;
            done("Logging out.", None)
        }
        "power" => {
            let action = match str_arg("action").as_str() {
                "power-off" => "shutdown",
                other => other,
            }
            .to_string();
            let handle = app.clone();
            app.run_on_main_thread(move || {
                if let Err(e) = crate::overlay::power(handle.clone(), handle.state(), action) {
                    tracing::warn!("assistant: power: {e}");
                }
            })
            .map_err(|e| e.to_string())?;
            done("Okay.", None)
        }
        other => Err(format!("I don't know how to {other} yet.")),
    }
}

async fn undo_one(app: &AppHandle, u: &Undo) -> Result<(), String> {
    match u {
        Undo::Volume { volume, muted } => {
            crate::sound::sound_volume(true, *volume as f32).await?;
            if *muted {
                crate::sound::sound_mute(true, true).await?;
            }
            Ok(())
        }
        Undo::Muted { output, muted } => crate::sound::sound_mute(*output, *muted).await,
        Undo::Output(id) => crate::sound::sound_default(*id).await,
        Undo::Activate(id) => send(app, "activate", Some(*id)),
        Undo::Minimize(id) => send(app, "minimize", Some(*id)),
        Undo::Place(id) => place_line(app, *id, "restore"),
        Undo::Desktop(n) => send(app, "workspace", Some(*n)),
        Undo::Dnd(on) => {
            let p = notify_prefs(app);
            crate::notify::set_notify_prefs(app.clone(), app.state(), *on, p.banner_ms, p.muted.clone()).map(|_| ())
        }
        Undo::Appearance(mode) => crate::compositor::kompact_set(app.state(), "appearance".into(), mode.clone()),
        Undo::KeepAwake(on) => {
            crate::screensaver::set_keep_awake(app.clone(), app.state(), *on);
            Ok(())
        }
        Undo::Wifi(on) => crate::network::network_wifi(*on).await,
        Undo::Bluetooth(on) => crate::bluetooth::bluetooth_power(*on).await,
    }
}

// ---- a request, step by step ----

fn finish(app: &AppHandle, said: Vec<String>, undo: Vec<Undo>, read_only: bool) -> Reply {
    let mut r = reply(said.join(" "), if read_only { "speaking" } else { "happy" });
    if !undo.is_empty() {
        let state = app.state::<Assistant>();
        let id = state.id();
        state.with(|i| {
            i.undos.push((id, undo));
            let extra = i.undos.len().saturating_sub(KEEP_UNDOS);
            i.undos.drain(..extra);
        });
        r.undo = Some(id);
    }
    r
}

/// Run `steps` from `at`, until the end, a refusal, an error, a step that asks first, or Stop.
async fn proceed(app: &AppHandle, steps: Vec<Step>, at: usize, said: Vec<String>, undo: Vec<Undo>, confirmed: bool) -> Reply {
    let mut plan = Plan::new(app, &steps, at);
    let r = run_steps(app, &steps, at, said, undo, confirmed, &mut plan).await;
    plan.into_reply(r)
}

async fn run_steps(app: &AppHandle, steps: &[Step], at: usize, mut said: Vec<String>, mut undo: Vec<Undo>, mut confirmed: bool, plan: &mut Plan<'_>) -> Reply {
    use std::sync::atomic::Ordering;
    let mut ctx = Ctx::default();
    let read_only = steps.iter().all(|s| super::tools::find(s.tool).is_some_and(|t| t.tier == super::tools::Tier::Read));
    for i in at..steps.len() {
        if i > at && STOP_PLAN.swap(false, Ordering::Relaxed) {
            info!("assistant: stopped after {i} of {} steps", steps.len());
            plan.rest(i, "stopped");
            said.push(format!("Stopped: {i} of {} done.", steps.len()));
            return finish(app, said, undo, true);
        }
        let s = &steps[i];
        plan.set(i, "now");
        let proposal = Proposal { tool: s.tool.to_string(), args: s.args.clone(), from_untrusted: false };
        match policy::decide(&proposal, i) {
            Decision::Refuse(r) => {
                info!("assistant: refused {}: {r}", s.tool);
                activity::record(app, Kind::Refused, s.tool, &r.to_string());
                plan.set(i, "failed");
                plan.rest(i + 1, "stopped");
                said.push(r.to_string());
                return Reply { mood: "sorry", ..finish(app, said, undo, true) };
            }
            Decision::Confirm { .. } if !confirmed => {
                let window = s.args.get("window").and_then(Value::as_str).and_then(|q| window_for(app, q).ok()).map(|w| label(&w));
                if s.tool == "close_window" && window.is_none() {
                    plan.set(i, "failed");
                    plan.rest(i + 1, "stopped");
                    said.push(format!("I can't find a window for “{}”.", s.args["window"].as_str().unwrap_or("")));
                    return Reply { mood: "sorry", ..finish(app, said, undo, true) };
                }
                let (text, button) = confirm_text(s.tool, &s.args, window.as_deref());
                let state = app.state::<Assistant>();
                let id = state.id();
                info!("assistant: asks first: {} ({id})", s.tool);
                activity::record(app, Kind::AsksFirst, s.tool, &text);
                plan.set(i, "asks");
                let prefix = said.join(" ");
                let text = if prefix.is_empty() { text } else { format!("{prefix} {text}") };
                let card = Card { id, text: text.clone(), button };
                state.with(|inner| inner.waiting = Some(Waiting { id, steps: steps.to_vec(), at: i, said, undo, card: card.clone(), whole_plan: false }));
                return Reply { text, mood: "concerned", undo: None, confirm: Some(card), plan: None };
            }
            Decision::Confirm { .. } | Decision::Run { .. } => {
                confirmed = false;
                match run_step(app, s, &mut ctx).await {
                    Ok(d) => {
                        info!("assistant: ran {} {}", s.tool, s.args);
                        activity::record(app, Kind::Ran, s.tool, &d.text);
                        plan.set(i, "done");
                        said.push(d.text);
                        undo.extend(d.undo);
                    }
                    Err(e) => {
                        info!("assistant: {} failed: {e}", s.tool);
                        activity::record(app, Kind::Failed, s.tool, &e);
                        plan.set(i, "failed");
                        plan.rest(i + 1, "stopped");
                        said.push(e);
                        return Reply { mood: "sorry", ..finish(app, said, undo, true) };
                    }
                }
            }
        }
    }
    finish(app, said, undo, read_only)
}

/// A typed request (the HUD's Type).
#[tauri::command]
pub async fn assistant_ask(app: AppHandle, text: String) -> Reply {
    let text: String = text.chars().take(300).collect();
    info!("assistant: asked {text:?}");
    if !text.trim().is_empty() {
        activity::record(&app, Kind::Asked, "", text.trim());
    }
    let r = request(&app, &text).await;
    aloud(&app, r, false)
}

/// 19h-2: `r` said aloud too, if Settings → Assistant → Speak replies says so for a request you
/// said (`spoken`) or typed / clicked.
fn aloud(app: &AppHandle, r: Reply, spoken: bool) -> Reply {
    if !r.text.is_empty() && super::speech::speak_pref().says(spoken) {
        super::speech::say(app, &r.text);
    }
    r
}

async fn request(app: &AppHandle, text: &str) -> Reply {
    // A new request drops one that was waiting on a card, and a Stop meant for an old one.
    app.state::<Assistant>().with(|i| i.waiting = None);
    STOP_PLAN.store(false, std::sync::atomic::Ordering::Relaxed);
    // 19e: a routine ("set up for coding") before the rules ("start coding" would open an app).
    let routines = super::routines::load();
    if let Some(r) = super::routines::invoked(text, &routines) {
        return match super::routines::expand(r) {
            Ok(steps) => {
                info!("assistant: routine {:?}: {} steps", r.name, steps.len());
                let done = proceed(app, steps, 0, Vec::new(), Vec::new(), false).await;
                // All done: one line for the whole routine (the plan shows each step).
                if done.mood == "happy" { Reply { text: format!("Ready for {}.", r.name), ..done } } else { done }
            }
            Err(p) => reply(format!("The {} routine has a request I don't understand: “{p}”. Fix it in Settings → Assistant.", r.name), "sorry"),
        };
    }
    match rules::parse(text) {
        Parsed::Hello => reply("Vanakkam! What can I do for you?", "happy"),
        Parsed::Help => reply(HELP, "speaking"),
        Parsed::Unknown(part) if part.is_empty() => reply("Type what you'd like me to do.", "idle"),
        // 19f: what the rules don't know, the local model may (its steps go through `policy`
        // like any others).
        Parsed::Unknown(part) if super::model::state().ready => {
            let words = text.to_string();
            match tauri::async_runtime::spawn_blocking(move || super::model::ask(&words)).await.unwrap_or_else(|e| Err(e.to_string())) {
                Ok(steps) if !steps.is_empty() => {
                    let lines: Vec<String> = steps.iter().map(rules::describe).collect();
                    activity::record(app, Kind::Understood, "", &lines.join("; "));
                    // One step about what you said runs (with Undo); several steps, or one that
                    // isn't about your words (misheard words make those), only on your click.
                    if steps.len() == 1 && super::model::grounded(&steps[0], text) {
                        return proceed(app, steps, 0, Vec::new(), Vec::new(), false).await;
                    }
                    let state = app.state::<Assistant>();
                    let id = state.id();
                    info!("assistant: the model's plan asks first ({id}): {}", lines.join("; "));
                    let card = Card { id, text: format!("Did you mean: {}?", lines.join(", ")), button: "Do It".into() };
                    let plan = lines.into_iter().map(|text| PlanItem { text, state: "todo" }).collect();
                    state.with(|inner| inner.waiting = Some(Waiting { id, steps, at: 0, said: Vec::new(), undo: Vec::new(), card: card.clone(), whole_plan: true }));
                    Reply { text: card.text.clone(), mood: "concerned", undo: None, confirm: Some(card), plan: Some(plan) }
                }
                Ok(_) => {
                    activity::record(app, Kind::Refused, "", "Not something I can do here");
                    reply("That's not something I can do on this desktop.", "sorry")
                }
                Err(e) => {
                    info!("assistant: the model couldn't help: {e}");
                    activity::record(app, Kind::Refused, "", &format!("Didn't understand “{part}”"));
                    reply(format!("Sorry, I don't understand “{part}”. Ask me “help” for ideas."), "sorry")
                }
            }
        }
        Parsed::Unknown(part) => {
            activity::record(app, Kind::Refused, "", &format!("Didn't understand “{part}”"));
            reply(format!("Sorry, I don't understand “{part}” yet. Ask me “help” for ideas."), "sorry")
        }
        Parsed::Steps(steps) => proceed(app, steps, 0, Vec::new(), Vec::new(), false).await,
    }
}

/// 19h: what the menu bar and the HUD show: `listening` (with the microphone's name), `ready`
/// (it gives sound), `thinking` (with what was heard), `idle`.
pub fn tell(app: &AppHandle, state: &str, detail: &str) {
    let _ = app.emit("kompact-assistant", serde_json::json!({"state": state, "detail": detail}));
}

/// 19h: listen until you've finished, then do what was said. The menu bar and the HUD follow
/// along through `kompact-assistant` and the wave through `kompact-assistant-level`.
#[tauri::command]
pub async fn assistant_listen(app: AppHandle) -> Reply {
    let ready = super::voice::state();
    if let Some(missing) = ready.missing {
        return reply(format!("My ears aren't set up yet. {missing}"), "sorry");
    }
    if super::voice::listening() {
        return reply("", "idle");
    }
    let mic = match tauri::async_runtime::spawn_blocking(crate::sound::microphone_now).await {
        Ok(Ok(Some(mic))) => mic,
        Ok(Ok(None)) => return reply("There's no microphone. Connect your headset, or choose one in Settings → Sound.", "sorry"),
        Ok(Err(e)) => return reply(format!("I can't find the microphone: {e}"), "sorry"),
        Err(e) => return reply(format!("I can't find the microphone: {e}"), "sorry"),
    };
    // Never talk over you (nor let the microphone hear Siva).
    super::speech::hush();
    tell(&app, "listening", &mic.label);
    let a = app.clone();
    let heard = tauri::async_runtime::spawn_blocking(move || {
        let level = |l: f64| {
            let _ = a.emit("kompact-assistant-level", l);
        };
        // The microphone is on: say so with the chime (a Bluetooth one takes ~1 s to wake).
        let ready = || {
            super::speech::play_chime();
            tell(&a, "ready", &mic.label);
        };
        super::voice::listen(&mic, &level, &ready)
    })
    .await
    .unwrap_or_else(|e| Err(e.to_string()));
    let r = match heard {
        Ok(super::voice::Heard::Text(text)) => {
            tell(&app, "thinking", &text);
            assistant_heard(app.clone(), text).await
        }
        Ok(super::voice::Heard::Nothing(why)) => reply(why, "sorry"),
        Ok(super::voice::Heard::Cancelled) => reply("", "idle"),
        Err(e) => reply(e, "sorry"),
    };
    tell(&app, "idle", "");
    aloud(&app, r, true)
}

/// 19d: what Siva heard. While a card waits, a spoken "no"
/// or "cancel" answers it; a spoken "yes" can't (`policy::confirmed`), so the card stays for a
/// click. Anything else is a new request, as if typed.
#[tauri::command]
pub async fn assistant_heard(app: AppHandle, text: String) -> Reply {
    let text: String = text.chars().take(300).collect();
    info!("assistant: heard {text:?}");
    activity::record(&app, Kind::Heard, "", text.trim());
    let waiting = app.state::<Assistant>().with(|i| i.waiting.as_ref().map(|w| (w.id, w.card.clone())));
    let spoken = rules::spoken_answer(&text);
    match (spoken, waiting) {
        (Some(s), Some((id, card))) => match policy::confirmed(policy::Answer::Spoken(s)) {
            Some(confirm) => answer(&app, id, confirm).await,
            None => {
                info!("assistant: a spoken yes can't confirm; the card stays");
                let text = format!("I heard “{}”, but only a click on {} says yes to this.", text.trim(), card.button);
                Reply { text: text.clone(), mood: "concerned", undo: None, confirm: Some(Card { text, ..card }), plan: None }
            }
        },
        (Some(policy::Spoken::No), None) if matches!(rules::parse(&text), Parsed::Unknown(_)) => reply("Okay. There's nothing to cancel.", "idle"),
        (Some(policy::Spoken::Yes), None) => reply("There's nothing waiting for a yes.", "idle"),
        _ => request(&app, &rules::for_voice(&text)).await,
    }
}

/// The card's Confirm (a click or Enter on it) or Cancel. A spoken answer goes through
/// `policy::confirmed` first (19d) and can only cancel.
#[tauri::command]
pub async fn assistant_answer(app: AppHandle, id: u64, confirm: bool) -> Reply {
    let r = answer(&app, id, confirm).await;
    aloud(&app, r, false)
}

async fn answer(app: &AppHandle, id: u64, confirm: bool) -> Reply {
    let app = app.clone();
    let waiting = app.state::<Assistant>().with(|i| i.waiting.take_if(|w| w.id == id));
    let Some(w) = waiting else { return reply("That question has gone; ask me again.", "sorry") };
    if !confirm {
        info!("assistant: cancelled {}", w.steps[w.at].tool);
        activity::record(&app, Kind::Cancelled, w.steps[w.at].tool, "Not done");
        let mut said = w.said;
        said.push("Okay, I won't.".into());
        return finish(&app, said, w.undo, true);
    }
    info!("assistant: confirmed {} (clicked)", w.steps[w.at].tool);
    STOP_PLAN.store(false, std::sync::atomic::Ordering::Relaxed);
    activity::record(&app, Kind::Confirmed, w.steps[w.at].tool, "Clicked");
    proceed(&app, w.steps, w.at, w.said, w.undo, !w.whole_plan).await
}

#[tauri::command]
pub async fn assistant_undo(app: AppHandle, id: u64) -> Reply {
    let r = undo(&app, id).await;
    aloud(&app, r, false)
}

async fn undo(app: &AppHandle, id: u64) -> Reply {
    let app = app.clone();
    let list = app.state::<Assistant>().with(|i| i.undos.iter().position(|(u, _)| *u == id).map(|p| i.undos.remove(p).1));
    let Some(list) = list else { return reply("There's nothing to undo there any more.", "sorry") };
    for u in list.iter().rev() {
        if let Err(e) = undo_one(&app, u).await {
            info!("assistant: undo failed: {e}");
            activity::record(&app, Kind::Failed, "undo", &e);
            return reply(format!("I couldn't undo all of it: {e}"), "sorry");
        }
    }
    info!("assistant: undone ({} changes)", list.len());
    activity::record(&app, Kind::Undone, "", &format!("{} change{}", list.len(), if list.len() == 1 { "" } else { "s" }));
    reply("Undone.", "happy")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn win(id: u64, name: &str, app_id: &str, title: &str, about: &str, focused: bool) -> WinCand {
        WinCand { id, name: name.into(), app_id: app_id.into(), title: title.into(), about: about.into(), focused, minimized: false }
    }

    #[test]
    fn finding_windows() {
        let list = vec![
            win(1, "Firefox", "firefox_firefox", "YouTube — Mozilla Firefox", "web browser internet", false),
            win(2, "Ptyxis", "org.gnome.Ptyxis", "dasanb@bUbuntu: ~", "terminal shell prompt command", true),
            win(3, "Text Editor", "org.gnome.TextEditor", "Untitled", "text editor gedit", false),
            win(4, "Firefox", "firefox_firefox", "Docs — Mozilla Firefox", "web browser internet", true),
        ];
        assert_eq!(best_window("firefox", &list).map(|w| w.id), Some(4), "of two Firefox windows, the focused one");
        assert_eq!(best_window("terminal", &list).map(|w| w.id), Some(2), "by the app's keywords");
        assert_eq!(best_window("gedit", &list).map(|w| w.id), Some(3));
        assert_eq!(best_window("youtube", &list).map(|w| w.id), Some(1), "by the title");
        assert_eq!(best_window("text editor", &list).map(|w| w.id), Some(3));
        assert_eq!(best_window("spotify", &list), None);
        assert_eq!(best_window("", &list), None);
    }

    #[test]
    fn finding_apps() {
        let apps = vec![
            AppCand { id: "firefox_firefox.desktop".into(), name: "Firefox".into(), about: "web browser".into() },
            AppCand { id: "org.gnome.Ptyxis.desktop".into(), name: "Ptyxis".into(), about: "terminal;shell;prompt".into() },
            AppCand { id: "org.gnome.Nautilus.desktop".into(), name: "Files".into(), about: "folder;manager;explore".into() },
            AppCand { id: "org.gnome.Settings.desktop".into(), name: "Settings".into(), about: "preferences".into() },
        ];
        assert_eq!(best_app("firefox", &apps).map(|a| a.name.as_str()), Some("Firefox"));
        assert_eq!(best_app("terminal", &apps).map(|a| a.name.as_str()), Some("Ptyxis"));
        assert_eq!(best_app("files", &apps).map(|a| a.name.as_str()), Some("Files"));
        assert_eq!(best_app("nautilus", &apps).map(|a| a.name.as_str()), Some("Files"), "by its id");
        assert_eq!(best_app("sudo rm -rf", &apps), None, "nothing to start");
    }

    #[test]
    fn cards() {
        assert_eq!(confirm_text("close_window", &json!({"window": "gedit"}), Some("Untitled — Text Editor")).0, "Close “Untitled — Text Editor”? It may have unsaved changes.");
        assert_eq!(confirm_text("bluetooth", &json!({"on": false}), None).1, "Turn Off");
        assert_eq!(confirm_text("power", &json!({"action": "restart"}), None).1, "Restart");
        assert_eq!(confirm_text("power", &json!({"action": "power-off"}), None).1, "Power Off");
    }
}
