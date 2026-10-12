//! 14c: kompact's login screen, a greetd greeter. One layer-shell surface per monitor covers the
//! screen (the login form on the largest, the time and background on the others), each a WebKit
//! view of `page.html`. It lists the people who can log in (`people.rs`), shows each one's
//! picture (AccountsService's copy) and published background and message, remembers the last
//! user and each user's session, logs in through greetd (`greetd.rs`, off the GTK thread) and
//! exits once greetd will start the session. Restart and Shut Down through systemd.
//!
//! `--test`: a banner says so and Ctrl+Q quits (for trying it in a normal session against
//! `kompact/tools/fake-greetd.py`). Logs go to stderr, never the password.

#[path = "../../apps.rs"]
#[allow(dead_code)]
mod apps;
mod greetd;
#[path = "../../login_prefs.rs"]
mod login_prefs;
#[path = "../kompact-lock/look.rs"]
#[allow(dead_code)]
mod look;
mod people;

use std::cell::RefCell;
use std::path::{Path, PathBuf};

use gtk::gdk;
use gtk::glib;
use gtk::prelude::*;
use gtk_layer_shell::{Edge, KeyboardMode, Layer, LayerShell};
use javascriptcore::ValueExt;
use serde_json::{json, Value};
use webkit2gtk::{SettingsExt, UserContentManagerExt, WebViewExt};

macro_rules! log {
    ($($t:tt)*) => { eprintln!("kompact-greeter: {}", format!($($t)*)) };
}

const PAGE: &str = include_str!("page.html");

fn env_path(var: &str) -> Option<PathBuf> {
    std::env::var_os(var).filter(|v| !v.is_empty()).map(PathBuf::from)
}

/// Where the greeter keeps what it remembers.
fn memory_path() -> Option<PathBuf> {
    env_path("KOMPACT_GREETER_STATE")
        .or_else(|| env_path("XDG_STATE_HOME").map(|d| d.join("kompact-greeter/state")))
        .or_else(|| env_path("HOME").map(|h| h.join(".local/state/kompact-greeter/state")))
}

fn load_memory() -> people::Memory {
    memory_path().and_then(|p| std::fs::read_to_string(p).ok()).map(|t| people::Memory::parse(&t)).unwrap_or_default()
}

fn save_memory(m: &people::Memory) {
    let Some(path) = memory_path() else { return };
    let written = path.parent().map(std::fs::create_dir_all).transpose().and_then(|_| std::fs::write(&path, m.to_text()));
    if let Err(e) = written {
        log!("can't remember the last user in {}: {e}", path.display());
    }
}

fn session_dirs() -> Vec<PathBuf> {
    match env_path("KOMPACT_GREETER_SESSIONS") {
        Some(d) => vec![d],
        None => vec!["/usr/local/share/wayland-sessions".into(), "/usr/share/wayland-sessions".into()],
    }
}

fn load_sessions() -> Vec<people::Session> {
    let mut files = Vec::new();
    for dir in session_dirs() {
        let Ok(rd) = std::fs::read_dir(&dir) else { continue };
        for e in rd.flatten() {
            let p = e.path();
            let Some(id) = p.file_name().and_then(|n| n.to_str()).and_then(|n| n.strip_suffix(".desktop")) else { continue };
            if let Ok(t) = std::fs::read_to_string(&p) {
                files.push((id.to_string(), t));
            }
        }
    }
    people::sessions(&files)
}

fn load_people() -> Vec<people::Person> {
    let passwd = std::fs::read_to_string("/etc/passwd").unwrap_or_default();
    let shells_text = std::fs::read_to_string("/etc/shells").unwrap_or_default();
    let shells: Vec<&str> = shells_text.lines().map(str::trim).filter(|l| l.starts_with('/')).collect();
    people::people(&passwd, &shells)
}

/// 14d: a published file counts only if the user it's named after owns it (anyone may add files
/// to the shared folder, so no one can dress up someone else's login).
fn owned_by(p: &Path, uid: u32) -> bool {
    use std::os::unix::fs::MetadataExt;
    std::fs::symlink_metadata(p).is_ok_and(|m| m.is_file() && m.uid() == uid)
}

fn file_url(p: &Path) -> String {
    format!("file://{}", p.display())
}

/// Everything the page shows, as `window.GREETER`.
fn page_config(people: &[people::Person], sessions: &[people::Session], memory: &people::Memory, main: bool, test: bool) -> Value {
    let shared = env_path("KOMPACT_GREETER_SHARED").unwrap_or_else(|| PathBuf::from(people::SHARED_DIR));
    let users: Vec<Value> = people
        .iter()
        .map(|p| {
            let picture = PathBuf::from(format!("/var/lib/AccountsService/icons/{}", p.login));
            let look = people::shared_conf(&shared, &p.login)
                .filter(|c| owned_by(c, p.uid))
                .and_then(|c| std::fs::read_to_string(c).ok())
                .map(|t| login_prefs::LoginPrefs::parse(&t));
            let (backdrop, blur) = look.as_ref().map(|l| people::published_look(l, &shared)).unwrap_or((None, true));
            let backdrop = backdrop.filter(|b| owned_by(b, p.uid));
            json!({
                "login": p.login,
                "name": p.name,
                "initials": look::initials(&p.name),
                "picture": picture.is_file().then(|| file_url(&picture)),
                "wallpaper": backdrop.filter(|b| b.is_file()).map(|b| file_url(&b)),
                "blur": blur,
                "message": look.map(|l| l.message).unwrap_or_default(),
                "session": people::session_for(sessions, memory, &p.login).map(|s| s.id.clone()),
            })
        })
        .collect();
    json!({
        "users": users,
        "first": people::first_person(people, memory),
        "sessions": sessions.iter().map(|s| json!({"id": s.id, "name": s.name})).collect::<Vec<_>>(),
        "main": main,
        "test": test,
        "greetd": std::env::var("GREETD_SOCK").is_ok(),
    })
}

fn page_html(config: &Value) -> String {
    let json = config.to_string().replace('<', "\\u003c");
    PAGE.replacen("/*GREETER*/", &format!("window.GREETER = {json};"), 1)
}

struct State {
    views: Vec<webkit2gtk::WebView>,
    monitors: Vec<gdk::Monitor>,
    people: Vec<people::Person>,
    sessions: Vec<people::Session>,
    memory: people::Memory,
    test: bool,
    busy: bool,
    failures: u32,
    main_done: bool,
}

thread_local! {
    static STATE: RefCell<Option<State>> = const { RefCell::new(None) };
}

fn with_state<T>(f: impl FnOnce(&mut State) -> T) -> Option<T> {
    STATE.with(|s| s.borrow_mut().as_mut().map(f))
}

fn tell_pages(state: &State, call: &str, arg: &Value) {
    let script = format!("window.kompactGreeter && window.kompactGreeter.{call}({arg})");
    for v in &state.views {
        v.evaluate_javascript(&script, None, None, None::<&gtk::gio::Cancellable>, |_| {});
    }
}

/// The monitor for the login form: the largest (the one kompact makes the main display, unless
/// you chose another; the form's monitor is where the keyboard goes).
fn main_monitor(display: &gdk::Display) -> Option<gdk::Monitor> {
    (0..display.n_monitors())
        .filter_map(|i| display.monitor(i))
        .max_by_key(|m| {
            let g = m.geometry();
            g.width() as i64 * g.height() as i64
        })
}

fn add_window(state: &mut State, monitor: &gdk::Monitor, main: bool) {
    if state.monitors.contains(monitor) {
        return;
    }
    let window = gtk::Window::new(gtk::WindowType::Toplevel);
    window.init_layer_shell();
    window.set_namespace("kompact-greeter");
    window.set_layer(Layer::Overlay);
    window.set_monitor(monitor);
    for edge in [Edge::Top, Edge::Bottom, Edge::Left, Edge::Right] {
        window.set_anchor(edge, true);
    }
    window.set_exclusive_zone(-1);
    window.set_keyboard_mode(if main { KeyboardMode::Exclusive } else { KeyboardMode::None });
    window.connect_delete_event(|_, _| glib::Propagation::Stop);
    let ucm = webkit2gtk::UserContentManager::new();
    ucm.register_script_message_handler("kompact");
    ucm.connect_script_message_received(Some("kompact"), |_, result| {
        if let Some(text) = result.js_value().map(|v| v.to_str().to_string()) {
            on_message(&text);
        }
    });
    let view = webkit2gtk::WebView::with_user_content_manager(&ucm);
    if let Some(s) = WebViewExt::settings(&view) {
        s.set_allow_file_access_from_file_urls(true);
        s.set_enable_developer_extras(false);
    }
    view.set_background_color(&gdk::RGBA::new(0.0, 0.0, 0.0, 1.0));
    let config = page_config(&state.people, &state.sessions, &state.memory, main, state.test);
    view.load_html(&page_html(&config), Some("file:///"));
    window.add(&view);
    let v = view.clone();
    window.connect_focus_in_event(move |_, _| {
        v.grab_focus();
        glib::Propagation::Proceed
    });
    window.show_all();
    if main {
        view.grab_focus();
    }
    state.views.push(view);
    state.monitors.push(monitor.clone());
    log!("{} on {}", if main { "login form" } else { "background" }, monitor.model().map(|m| m.to_string()).unwrap_or_else(|| "a monitor".into()));
}

fn on_message(text: &str) {
    let Ok(msg) = serde_json::from_str::<Value>(text) else { return };
    match msg["type"].as_str() {
        Some("login") => login(&msg),
        Some("power") => power(msg["action"].as_str().unwrap_or("")),
        // The form's user changed: the other monitors show their background too.
        Some("select") => {
            let user = msg["user"].as_str().unwrap_or("").to_string();
            with_state(|s| {
                if s.people.iter().any(|p| p.login == user) {
                    tell_pages(s, "select", &json!(user));
                }
            });
        }
        Some("quit") if with_state(|s| s.test).unwrap_or(false) => {
            log!("test: quit (Ctrl+Q)");
            gtk::main_quit();
        }
        _ => {}
    }
}

fn power(action: &str) {
    let verb = match action {
        "restart" => "reboot",
        "shutdown" => "poweroff",
        _ => return,
    };
    if with_state(|s| s.test).unwrap_or(false) {
        log!("test: would run systemctl {verb}");
        return;
    }
    log!("systemctl {verb}");
    if let Err(e) = std::process::Command::new("systemctl").arg(verb).spawn() {
        log!("systemctl {verb}: {e}");
    }
}

fn login(msg: &Value) {
    let user = msg["user"].as_str().unwrap_or("").to_string();
    let password = msg["password"].as_str().unwrap_or("").to_string();
    let session_id = msg["session"].as_str().unwrap_or("").to_string();
    let Some(sock) = std::env::var("GREETD_SOCK").ok() else {
        with_state(|s| tell_pages(s, "error", &json!("greetd isn't running (no GREETD_SOCK).")));
        return;
    };
    let prepared = with_state(|s| {
        if s.busy || s.main_done || !s.people.iter().any(|p| p.login == user) {
            return None;
        }
        let session = s.sessions.iter().find(|x| x.id == session_id).or_else(|| s.sessions.first())?.clone();
        s.busy = true;
        tell_pages(s, "checking", &json!(true));
        Some(session)
    })
    .flatten();
    let Some(session) = prepared else {
        with_state(|s| {
            if s.sessions.is_empty() {
                tell_pages(s, "error", &json!("No sessions are installed (/usr/share/wayland-sessions)."));
            }
        });
        return;
    };
    log!("logging in {user} to {} ({})", session.name, session.cmd.join(" "));
    std::thread::spawn(move || {
        let env = people::session_env(&session);
        let note = |t: &str| {
            let t = t.to_string();
            glib::idle_add_once(move || {
                with_state(|s| tell_pages(s, "note", &json!(t)));
            });
        };
        let outcome = greetd::login(&sock, &user, &password, &session.cmd, &env, &note);
        drop(password);
        glib::idle_add_once(move || finished(&user, &session.id, outcome));
    });
}

fn finished(user: &str, session: &str, outcome: greetd::Outcome) {
    let quit = with_state(|s| {
        s.busy = false;
        match outcome {
            greetd::Outcome::Started => {
                log!("{user} logged in; greetd starts the session when the greeter exits");
                s.memory.logged_in(user, session);
                save_memory(&s.memory);
                s.main_done = true;
                tell_pages(s, "started", &json!(null));
                true
            }
            greetd::Outcome::Wrong(why) => {
                s.failures += 1;
                let wait = look::wait_after(s.failures);
                log!("wrong password for {user} ({} in a row: {why})", s.failures);
                tell_pages(s, "failed", &json!({"failures": s.failures, "wait_ms": wait.as_millis() as u64}));
                false
            }
            greetd::Outcome::Failed(why) => {
                log!("login for {user} failed: {why}");
                tell_pages(s, "error", &json!(why));
                false
            }
        }
    })
    .unwrap_or(false);
    if quit {
        // Let the page show "Logging in…" for a moment, then leave the screen to the session.
        glib::timeout_add_local_once(std::time::Duration::from_millis(300), gtk::main_quit);
    }
}

fn main() {
    let test = std::env::args().any(|a| a == "--test");
    if let Err(e) = gtk::init() {
        log!("no display: {e}");
        std::process::exit(1);
    }
    if !gtk_layer_shell::is_supported() {
        log!("the compositor has no wlr-layer-shell");
        std::process::exit(1);
    }
    let people = load_people();
    let sessions = load_sessions();
    let memory = load_memory();
    log!(
        "{} {} ({}), sessions: {}{}",
        people.len(),
        if people.len() == 1 { "person" } else { "people" },
        people.iter().map(|p| p.login.as_str()).collect::<Vec<_>>().join(", "),
        sessions.iter().map(|s| s.id.as_str()).collect::<Vec<_>>().join(", "),
        if test { "; test mode, Ctrl+Q quits" } else { "" }
    );
    if std::env::var("GREETD_SOCK").is_err() {
        log!("no GREETD_SOCK: logging in won't work (run it from greetd, or tools/fake-greetd.py to try)");
    }
    STATE.with(|s| {
        *s.borrow_mut() = Some(State {
            views: Vec::new(),
            monitors: Vec::new(),
            people,
            sessions,
            memory,
            test,
            busy: false,
            failures: 0,
            main_done: false,
        })
    });
    let display = gdk::Display::default().expect("a display after gtk::init");
    let main = main_monitor(&display);
    for i in 0..display.n_monitors() {
        if let Some(m) = display.monitor(i) {
            let is_main = main.as_ref() == Some(&m);
            with_state(|s| add_window(s, &m, is_main));
        }
    }
    display.connect_monitor_added(|_, m| {
        let m = m.clone();
        with_state(|s| add_window(s, &m, false));
    });
    gtk::main();
    log!("done");
}
