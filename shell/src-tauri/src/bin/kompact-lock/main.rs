//! 14a: kompact's own lock screen. kompact runs it to lock (`KOMPACT_LOCK_CMD`, in place of
//! `swaylock -f`). It locks the session with ext-session-lock-v1 through libgtk-session-lock
//! (one GTK window per monitor, each a WebKit view of `page.html`), checks the password with PAM
//! off the GTK thread, and unlocks and exits when it's right. kompact keeps the screen black if
//! this crashes, and a new lock client can take over.
//!
//! If the library is missing (`sudo apt install libgtk-session-lock0`) or the compositor refuses
//! the lock, it runs `swaylock -f` instead, so a lock never silently doesn't happen.
//!
//! Logs go to stderr (kompact's log, `ktest.log`), never the password.

mod dl;
#[path = "../../login_prefs.rs"]
mod login_prefs;
mod look;
mod pam;

use std::cell::RefCell;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use gtk::gdk;
use gtk::glib;
use gtk::glib::translate::{FromGlibPtrFull, ToGlibPtr};
use gtk::prelude::*;
use javascriptcore::ValueExt;
use serde_json::{json, Value};
use webkit2gtk::{SettingsExt, UserContentManagerExt, WebViewExt};

macro_rules! log {
    ($($t:tt)*) => { eprintln!("kompact-lock: {}", format!($($t)*)) };
}

/// The page, with its style and script inline (one file, nothing to find at run time).
const PAGE: &str = include_str!("page.html");

/// libgtk-session-lock 0.2's calls.
struct SessionLock {
    is_supported: unsafe extern "C" fn() -> glib::ffi::gboolean,
    prepare: unsafe extern "C" fn() -> *mut glib::gobject_ffi::GObject,
    lock: unsafe extern "C" fn(*mut glib::gobject_ffi::GObject),
    unlock_and_destroy: unsafe extern "C" fn(*mut glib::gobject_ffi::GObject),
    new_surface: unsafe extern "C" fn(*mut glib::gobject_ffi::GObject, *mut gtk::ffi::GtkWindow, *mut gdk::ffi::GdkMonitor),
}

impl SessionLock {
    fn load() -> Result<SessionLock, String> {
        let lib = dl::Lib::open("libgtk-session-lock.so.0")?;
        // SAFETY: the signatures in gtk-session-lock.h (0.2).
        unsafe {
            Ok(SessionLock {
                is_supported: lib.get("gtk_session_lock_is_supported")?,
                prepare: lib.get("gtk_session_lock_prepare_lock")?,
                lock: lib.get("gtk_session_lock_lock_lock")?,
                unlock_and_destroy: lib.get("gtk_session_lock_lock_unlock_and_destroy")?,
                new_surface: lib.get("gtk_session_lock_lock_new_surface")?,
            })
        }
    }
}

/// Who is locked out: login name, full name, home.
struct User {
    login: String,
    name: String,
    home: PathBuf,
}

fn current_user() -> Option<User> {
    // SAFETY: getpwuid's result is read at once, before any other passwd call.
    unsafe {
        let pw = libc::getpwuid(libc::getuid());
        if pw.is_null() {
            return None;
        }
        let s = |p: *const libc::c_char| if p.is_null() { String::new() } else { std::ffi::CStr::from_ptr(p).to_string_lossy().into_owned() };
        let login = s((*pw).pw_name);
        Some(User { name: look::display_name(&s((*pw).pw_gecos), &login), home: PathBuf::from(s((*pw).pw_dir)), login })
    }
}

fn config_dir(home: &Path) -> PathBuf {
    std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from).unwrap_or_else(|| home.join(".config")).join("kompact")
}

fn local_hour() -> u32 {
    // SAFETY: time(NULL) and localtime_r with a valid out-pointer.
    unsafe {
        let now = libc::time(std::ptr::null_mut());
        let mut tm: libc::tm = std::mem::zeroed();
        if libc::localtime_r(&now, &mut tm).is_null() {
            return 12;
        }
        tm.tm_hour.clamp(0, 23) as u32
    }
}

/// What the page needs to know, as `window.LOCK`.
fn page_config(user: &User) -> Value {
    let dir = config_dir(&user.home);
    let read = |f: &str| std::fs::read_to_string(dir.join(f)).unwrap_or_default();
    let (dark, accent) = look::appearance(&read("settings"), local_hour());
    let file_url = |p: PathBuf| format!("file://{}", p.display());
    // 14b: Settings → Users & Login.
    let login = dir.parent().map(login_prefs::path).and_then(|p| std::fs::read_to_string(p).ok()).unwrap_or_default();
    let prefs = login_prefs::LoginPrefs::parse(&login);
    let desktop = look::wallpaper_image(&read("wallpaper")).filter(|p| p.is_file());
    let (backdrop, blur) = prefs.backdrop(desktop, |p| p.is_file());
    json!({
        "name": user.name,
        "initials": look::initials(&user.name),
        "avatar": look::avatar(&user.login, &user.home, |p| p.is_file()).map(file_url),
        "wallpaper": backdrop.map(file_url),
        "blur": blur,
        "message": prefs.message,
        "hour24": login_prefs::hour24(&read("clock")),
        "dark": dark,
        "accent": accent,
    })
}

/// The page with its config in: `/*LOCK*/` becomes `window.LOCK = {…};` (JSON, with `<` escaped
/// so a name can't end the script).
fn page_html(config: &Value) -> String {
    let json = config.to_string().replace('<', "\\u003c");
    PAGE.replacen("/*LOCK*/", &format!("window.LOCK = {json};"), 1)
}

/// Fall back to swaylock, so the screen still locks.
fn fall_back(why: &str) -> ! {
    log!("{why}; locking with swaylock -f instead");
    let e = std::process::Command::new("swaylock").arg("-f").exec();
    log!("can't run swaylock either: {e}");
    std::process::exit(2);
}

struct State {
    lock: *mut glib::gobject_ffi::GObject,
    api: SessionLock,
    views: Vec<webkit2gtk::WebView>,
    /// Monitors that have a lock window (one per monitor, as the library wants).
    monitors: Vec<gdk::Monitor>,
    html: String,
    service: &'static str,
    login: String,
    checking: bool,
    failures: u32,
    unlocked: bool,
}

thread_local! {
    static STATE: RefCell<Option<State>> = const { RefCell::new(None) };
}

fn with_state<T>(f: impl FnOnce(&mut State) -> T) -> Option<T> {
    STATE.with(|s| s.borrow_mut().as_mut().map(f))
}

/// Tell every page something (`kompactLock.<call>`).
fn tell_pages(state: &State, call: &str, arg: &Value) {
    let script = format!("window.kompactLock && window.kompactLock.{call}({arg})");
    for v in &state.views {
        v.evaluate_javascript(&script, None, None, None::<&gtk::gio::Cancellable>, |_| {});
    }
}

/// One lock window on `monitor`.
fn add_window(state: &mut State, monitor: &gdk::Monitor) {
    if state.monitors.contains(monitor) {
        return;
    }
    let window = gtk::Window::new(gtk::WindowType::Toplevel);
    // A lock surface, not a window: GTK draws no title bar (it did, with a close button that
    // took the page off that monitor), it starts the monitor's size, and nothing closes it.
    window.set_decorated(false);
    let geo = monitor.geometry();
    window.set_default_size(geo.width(), geo.height());
    window.connect_delete_event(|_, _| {
        log!("a close request for a lock window, refused");
        glib::Propagation::Stop
    });
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
    view.load_html(&state.html, Some("file:///"));
    window.add(&view);
    // The keyboard goes where kompact sends it (the monitor under the pointer).
    let v = view.clone();
    window.connect_focus_in_event(move |_, _| {
        v.grab_focus();
        glib::Propagation::Proceed
    });
    let lock = state.lock;
    // SAFETY: a live lock object and window/monitor pointers GTK keeps alive.
    unsafe { (state.api.new_surface)(lock, window.to_glib_none().0, monitor.to_glib_none().0) };
    window.show_all();
    state.views.push(view);
    state.monitors.push(monitor.clone());
    log!("lock window on {}", monitor.model().map(|m| m.to_string()).unwrap_or_else(|| "a monitor".into()));
}

/// From a page: `{"type":"unlock","password":"…"}`.
fn on_message(text: &str) {
    let Ok(msg) = serde_json::from_str::<Value>(text) else { return };
    if msg["type"] != "unlock" {
        return;
    }
    let password = msg["password"].as_str().unwrap_or("").as_bytes().to_vec();
    let started = with_state(|s| {
        if s.checking || s.unlocked {
            return None;
        }
        s.checking = true;
        tell_pages(s, "checking", &json!(true));
        Some((s.service, s.login.clone()))
    })
    .flatten();
    let Some((service, login)) = started else { return };
    std::thread::spawn(move || {
        let result = pam::check(service, &login, password);
        glib::idle_add_once(move || checked(result));
    });
}

fn checked(result: Result<(), String>) {
    let done = with_state(|s| {
        s.checking = false;
        match result {
            Ok(()) => {
                s.unlocked = true;
                log!("password right, unlocking");
                // SAFETY: the live lock object; never used again.
                unsafe { (s.api.unlock_and_destroy)(s.lock) };
                true
            }
            Err(why) => {
                s.failures += 1;
                let wait = look::wait_after(s.failures);
                log!("password wrong ({} in a row: {why}){}", s.failures, if wait.is_zero() { String::new() } else { format!(", next try in {} s", wait.as_secs()) });
                tell_pages(s, "failed", &json!({"failures": s.failures, "wait_ms": wait.as_millis() as u64}));
                false
            }
        }
    })
    .unwrap_or(false);
    if done {
        if let Some(d) = gdk::Display::default() {
            d.sync();
        }
        gtk::main_quit();
    }
}

fn main() {
    if let Err(e) = gtk::init() {
        fall_back(&format!("no GTK display: {e}"));
    }
    let api = SessionLock::load().unwrap_or_else(|e| fall_back(&format!("{e} (sudo apt install libgtk-session-lock0)")));
    // SAFETY: plain query.
    if unsafe { (api.is_supported)() } == 0 {
        fall_back("the compositor doesn't offer ext-session-lock");
    }
    let Some(user) = current_user() else { fall_back("can't tell who is logged in") };
    let service = look::pam_service(|p| p.exists());
    let html = page_html(&page_config(&user));
    // SAFETY: a new lock object we own (transfer full).
    let lock = unsafe { (api.prepare)() };
    if lock.is_null() {
        fall_back("gtk-session-lock made no lock");
    }
    // SAFETY: a valid GObject; this extra reference keeps it alive for the signal handlers.
    let obj = unsafe { glib::Object::from_glib_full(glib::gobject_ffi::g_object_ref(lock)) };
    obj.connect_local("locked", false, |_| {
        log!("locked");
        None
    });
    let refused = Rc::new(std::cell::Cell::new(false));
    let r = refused.clone();
    obj.connect_local("finished", false, move |_| {
        // The compositor ended the lock (or never gave it).
        let unlocked = with_state(|s| s.unlocked).unwrap_or(false);
        if !unlocked {
            r.set(true);
            gtk::main_quit();
        }
        None
    });
    // SAFETY: the live lock object.
    unsafe { (api.lock)(lock) };
    log!("locking for {} (PAM service {service})", user.login);
    STATE.with(|s| {
        *s.borrow_mut() = Some(State {
            lock,
            api,
            views: Vec::new(),
            monitors: Vec::new(),
            html,
            service,
            login: user.login.clone(),
            checking: false,
            failures: 0,
            unlocked: false,
        })
    });
    let display = gdk::Display::default().expect("a display after gtk::init");
    for i in 0..display.n_monitors() {
        if let Some(m) = display.monitor(i) {
            with_state(|s| add_window(s, &m));
        }
    }
    // A monitor plugged in while locked gets a lock window too.
    display.connect_monitor_added(|_, m| {
        let m = m.clone();
        with_state(|s| add_window(s, &m));
    });
    gtk::main();
    drop(obj);
    if refused.get() {
        fall_back("the lock was refused or ended by the compositor");
    }
    // Nothing to tidy: the lock is gone and the windows go with the process.
    log!("unlocked");
}

#[cfg(test)]
mod tests {
    /// Talks to the real PAM with a wrong password (takes ~3 s; logs one failure in auth.log):
    /// `cargo test --bin kompact-lock -- --ignored`.
    #[test]
    #[ignore]
    fn pam_says_no_to_a_wrong_password() {
        let user = std::env::var("USER").unwrap_or_default();
        let r = crate::pam::check(crate::look::pam_service(|p| p.exists()), &user, b"definitely-not-the-password".to_vec());
        assert!(r.is_err(), "{r:?}");
        eprintln!("PAM said: {}", r.unwrap_err());
    }
}
