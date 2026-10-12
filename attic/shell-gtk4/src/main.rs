//! kompact-shell: kompact's desktop shell (Phase 6). One GTK4 process; each shell surface is a
//! wlr-layer-shell window holding a WebKit view of a local HTML page from `web/`.
//!
//! 6a: the top panel (clock), reserving its height so maximized windows stay below it.
//! 6b: a wallpaper on the background layer of every monitor.
//! 6c-2: the launcher, opened from the panel's Apps button: an overlay surface that holds the
//! keyboard while open, listing the installed applications.
//!
//! `kompact-shell --list-apps` prints what the launcher would list, without starting the shell.

mod apps;
mod config;

use std::cell::{Cell, RefCell};
use std::path::Path;
use std::process::{Command, Stdio};
use std::rc::{Rc, Weak};

use gtk4_layer_shell::{Edge, KeyboardMode, Layer, LayerShell};
use tracing::{error, info, warn};
use tracing_subscriber::EnvFilter;
use webkit6::prelude::*;
use webkit6::{gdk, gio, glib, gtk, HardwareAccelerationPolicy, LoadEvent, UserContentManager, WebView};

use apps::{App, Message};
use config::{Config, Gpu, Wallpaper};

fn main() -> glib::ExitCode {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")))
        .init();

    if std::env::args().nth(1).as_deref() == Some("--list-apps") {
        let list = installed_apps();
        for a in &list {
            println!("{}\t{}\t{}", a.name, a.id, a.exec);
        }
        eprintln!("{} apps", list.len());
        return glib::ExitCode::SUCCESS;
    }

    let get = |key: &str| std::env::var(key).ok();
    let default_web = Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/web"));
    let cfg = match Config::from_env(get, default_web) {
        Ok(c) => c,
        Err(e) => {
            error!("{e}");
            return glib::ExitCode::FAILURE;
        }
    };
    // One shell per compositor: a second one would stack a second panel under the first (seen on
    // the GT 710 when it was started twice). The lock is held until the process exits.
    let _lock = match single_instance_lock() {
        Ok(lock) => lock,
        Err(e) => {
            error!("{e}");
            return glib::ExitCode::FAILURE;
        }
    };
    let panel = cfg.page("panel.html");
    if !panel.is_file() {
        error!("{} not found (set KOMPACT_SHELL_WEB to the shell's web/ directory)", panel.display());
        return glib::ExitCode::FAILURE;
    }
    // Before GTK starts: nothing else is running yet, so changing the environment is safe.
    let mut our_env = Vec::new();
    for (key, value) in cfg.env_defaults(get) {
        std::env::set_var(key, value);
        our_env.push(key);
    }
    info!(
        "kompact-shell starting: WAYLAND_DISPLAY={}, web {}, gpu {:?} (KOMPACT_SHELL_GPU), GSK_RENDERER={}, GDK_DEBUG={}",
        std::env::var("WAYLAND_DISPLAY").unwrap_or_else(|_| "(unset)".into()),
        cfg.web_dir.display(),
        cfg.gpu,
        std::env::var("GSK_RENDERER").unwrap_or_else(|_| "(gtk default)".into()),
        std::env::var("GDK_DEBUG").unwrap_or_default(),
    );

    // NON_UNIQUE: no D-Bus name needed, so it starts even without a session bus.
    let app = gtk::Application::builder()
        .application_id("org.kompact.Shell")
        .flags(gio::ApplicationFlags::NON_UNIQUE)
        .build();
    let state = RefCell::new(Some((cfg, our_env)));
    app.connect_activate(move |app| {
        let Some((cfg, our_env)) = state.borrow_mut().take() else { return };
        match gdk::Display::default() {
            Some(d) => info!("connected to display {}", d.name()),
            None => warn!("no display (not running inside kompact?)"),
        }
        if !gtk4_layer_shell::is_supported() {
            error!("the compositor doesn't offer wlr-layer-shell (is this running inside kompact?)");
            app.quit();
            return;
        }
        let shell = Rc::new(Shell {
            app: app.clone(),
            cfg,
            our_env,
            views: RefCell::default(),
            handlers: RefCell::default(),
            launcher: RefCell::default(),
        });
        start_wallpapers(&shell);
        build_panel(&shell);
        // The launcher is built on the first Apps click, not here: start-up stays what 6b was.
        if launcher_disabled() {
            info!("launcher: off (KOMPACT_SHELL_LAUNCHER=off)");
        }
    });
    // GTK would otherwise try to parse our arguments.
    let code = app.run_with_args::<&str>(&[]);
    info!("kompact-shell exiting ({code:?})");
    code
}

/// `KOMPACT_SHELL_LAUNCHER=off`: the Apps button does nothing.
fn launcher_disabled() -> bool {
    std::env::var("KOMPACT_SHELL_LAUNCHER").is_ok_and(|v| v.trim().eq_ignore_ascii_case("off"))
}

/// What every surface of the shell shares.
struct Shell {
    app: gtk::Application,
    cfg: Config,
    /// Environment variables the shell set for itself (`Config::env_defaults`); launched apps
    /// don't get them.
    our_env: Vec<&'static str>,
    /// Every page's view, so a new one can join an existing view's web process.
    views: RefCell<Vec<glib::WeakRef<WebView>>>,
    /// Content managers our `kompact` message handler is registered on.
    handlers: RefCell<Vec<UserContentManager>>,
    launcher: RefCell<Option<Rc<Launcher>>>,
}

/// Lock `$XDG_RUNTIME_DIR/kompact-shell-<WAYLAND_DISPLAY>.lock`, or fail if another shell for
/// the same compositor holds it. `None` when there's no runtime dir (nothing to lock in).
fn single_instance_lock() -> Result<Option<std::fs::File>, String> {
    let Some(dir) = std::env::var_os("XDG_RUNTIME_DIR") else { return Ok(None) };
    let display = std::env::var("WAYLAND_DISPLAY").unwrap_or_else(|_| "wayland-0".into());
    let path = Path::new(&dir).join(format!("kompact-shell-{}.lock", display.replace('/', "_")));
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&path)
        .map_err(|e| format!("can't open {}: {e}", path.display()))?;
    match file.try_lock() {
        Ok(()) => Ok(Some(file)),
        Err(std::fs::TryLockError::WouldBlock) => {
            Err(format!("kompact-shell is already running on {display} (pkill kompact-shell to restart it)"))
        }
        Err(std::fs::TryLockError::Error(e)) => Err(format!("can't lock {}: {e}", path.display())),
    }
}

/// A WebKit view for one shell page, with the GT 710-friendly settings.
///
/// All views share one web process: each new view is created "related" to a live one. WebKit
/// otherwise starts a web process per view (55–60 MB each on the GT 710 machine). Related views
/// also share their `Settings`. If the shared process crashes, every page reloads.
fn web_view(shell: &Rc<Shell>, name: &str, share_process: bool) -> WebView {
    let name: Rc<str> = name.into();
    let related = if share_process { shell.views.borrow().iter().find_map(|w| w.upgrade()) } else { None };
    let view = match &related {
        Some(r) => WebView::builder().related_view(r).build(),
        None => WebView::new(),
    };
    {
        let mut views = shell.views.borrow_mut();
        views.retain(|w| w.upgrade().is_some());
        views.push(view.downgrade());
    }
    info!(
        "{name}: {}",
        match (share_process, related.is_some()) {
            (true, true) => "joining the shared web process",
            (true, false) => "starting the shared web process",
            (false, _) => "starting its own web process",
        }
    );
    if let Some(settings) = WebViewExt::settings(&view) {
        // Pages are local files; the wallpaper loads an image from anywhere on disk.
        settings.set_allow_file_access_from_file_urls(true);
        settings.set_hardware_acceleration_policy(match shell.cfg.gpu {
            Gpu::Off => HardwareAccelerationPolicy::Never,
            Gpu::On => HardwareAccelerationPolicy::Always,
        });
        // console.log() from the page lands in our log.
        settings.set_enable_write_console_messages_to_stdout(true);
    }
    listen_for_messages(shell, &view);
    let (r, g, b) = config::PANEL_BG;
    view.set_background_color(&gdk::RGBA::new(r, g, b, 1.0));
    let n = name.clone();
    view.connect_load_changed(move |_, event| {
        if event == LoadEvent::Finished {
            info!("{n}: page loaded");
        }
    });
    let n = name.clone();
    view.connect_load_failed(move |_, _, uri, err| {
        error!("{n}: loading {uri} failed: {err}");
        false
    });
    view.connect_web_process_terminated(move |view, reason| {
        // Reload once; a page that keeps crashing stays blank rather than looping.
        warn!("{name}: web process ended ({reason:?}), reloading");
        view.reload();
    });
    view
}

/// Pages talk to us with `window.webkit.messageHandlers.kompact.postMessage("...")`. Related
/// views share a content manager, so the handler goes on each manager once.
fn listen_for_messages(shell: &Rc<Shell>, view: &WebView) {
    let Some(ucm) = view.user_content_manager() else { return };
    if shell.handlers.borrow().contains(&ucm) {
        return;
    }
    if !ucm.register_script_message_handler("kompact", None) {
        warn!("could not register the kompact message handler");
        return;
    }
    let weak: Weak<Shell> = Rc::downgrade(shell);
    ucm.connect_script_message_received(Some("kompact"), move |_, value| {
        let text = value.to_str();
        let Some(shell) = weak.upgrade() else { return };
        match apps::parse_message(&text) {
            Some(msg) => on_message(&shell, msg),
            None => warn!("ignoring unknown page message {text:?}"),
        }
    });
    shell.handlers.borrow_mut().push(ucm);
}

fn on_message(shell: &Rc<Shell>, msg: Message) {
    if launcher_disabled() {
        info!("ignoring {msg:?}: launcher is off");
        return;
    }
    let existing = shell.launcher.borrow().clone();
    let launcher = match existing {
        Some(l) => l,
        None => {
            let l = build_launcher(shell);
            *shell.launcher.borrow_mut() = Some(l.clone());
            l
        }
    };
    match msg {
        Message::ToggleLauncher => {
            if launcher.open.get() {
                launcher.close("panel button");
            } else {
                launcher.open();
            }
        }
        Message::CloseLauncher => launcher.close("closed from the page"),
        Message::Launch(id) => {
            let app = launcher.apps.borrow().iter().find(|a| a.id == id).cloned();
            launcher.close("app chosen");
            match app {
                Some(app) => launch(&app, &shell.our_env),
                None => warn!("launch: no app {id:?}"),
            }
        }
    }
}

/// The applications the launcher lists, from the XDG data directories.
fn installed_apps() -> Vec<App> {
    let path = std::env::var("PATH").ok();
    let dirs = apps::data_dirs(|k| std::env::var(k).ok());
    apps::scan(&dirs, |p| apps::program_exists(p, path.as_deref()))
}

/// Start an app as its own process (own process group, no stdio), so it outlives the shell.
fn launch(app: &App, our_env: &[&str]) {
    let Some(argv) = apps::exec_argv(&app.exec) else {
        warn!("launch {}: can't parse Exec={:?}", app.id, app.exec);
        return;
    };
    let mut cmd = Command::new(&argv[0]);
    cmd.args(&argv[1..]).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    if let Some(home) = std::env::var_os("HOME") {
        cmd.current_dir(home);
    }
    for key in our_env {
        cmd.env_remove(key);
    }
    std::os::unix::process::CommandExt::process_group(&mut cmd, 0);
    match cmd.spawn() {
        Ok(mut child) => {
            info!("launched {} ({:?}) as pid {}", app.id, argv, child.id());
            // Reap it when it exits, so it doesn't linger as a zombie.
            std::thread::spawn(move || {
                let _ = child.wait();
            });
        }
        Err(e) => error!("launch {}: {e}", app.id),
    }
}

fn build_panel(shell: &Rc<Shell>) {
    let window = gtk::ApplicationWindow::new(&shell.app);
    window.set_title(Some("kompact panel"));
    window.init_layer_shell();
    window.set_namespace(Some("kompact-panel"));
    window.set_layer(Layer::Top);
    for edge in [Edge::Top, Edge::Left, Edge::Right] {
        window.set_anchor(edge, true);
    }
    // The panel never takes the keyboard; its button is clicked.
    window.set_keyboard_mode(KeyboardMode::None);
    // Width comes from the left+right anchors; the height is ours, and is reserved.
    window.set_default_size(0, config::PANEL_HEIGHT);
    window.auto_exclusive_zone_enable();

    let view = web_view(shell, "panel", true);
    view.set_size_request(-1, config::PANEL_HEIGHT);
    window.set_child(Some(&view));
    let uri = gio::File::for_path(shell.cfg.page("panel.html")).uri();
    view.load_uri(&uri);
    window.present();
    snapshot_later(&window, &view, "panel");
    info!("panel: layer surface created ({}px, top layer, exclusive zone), loading {uri}", config::PANEL_HEIGHT);
}

/// 6b: one wallpaper per monitor, now and for monitors that appear later (each closes itself
/// when its monitor goes away).
fn start_wallpapers(shell: &Rc<Shell>) {
    if shell.cfg.wallpaper == Wallpaper::Off {
        info!("wallpaper: off (KOMPACT_WALLPAPER=none)");
        return;
    }
    let Some(display) = gdk::Display::default() else { return };
    let monitors = display.monitors();
    for i in 0..monitors.n_items() {
        if let Some(monitor) = monitors.item(i).and_downcast::<gdk::Monitor>() {
            build_wallpaper(shell, &monitor);
        }
    }
    let weak = Rc::downgrade(shell);
    monitors.connect_items_changed(move |list, position, _removed, added| {
        let Some(shell) = weak.upgrade() else { return };
        for i in position..position + added {
            if let Some(monitor) = list.item(i).and_downcast::<gdk::Monitor>() {
                build_wallpaper(&shell, &monitor);
            }
        }
    });
}

fn build_wallpaper(shell: &Rc<Shell>, monitor: &gdk::Monitor) {
    let name = format!("wallpaper {}", monitor.connector().unwrap_or_else(|| "?".into()));
    let window = gtk::ApplicationWindow::new(&shell.app);
    window.set_title(Some("kompact wallpaper"));
    window.init_layer_shell();
    window.set_namespace(Some("kompact-wallpaper"));
    window.set_layer(Layer::Background);
    for edge in [Edge::Top, Edge::Bottom, Edge::Left, Edge::Right] {
        window.set_anchor(edge, true);
    }
    // -1: cover the whole output, under panels too, and reserve nothing.
    window.set_exclusive_zone(-1);
    window.set_keyboard_mode(KeyboardMode::None);
    window.set_monitor(Some(monitor));

    let view = web_view(shell, &name, true);
    let wallpaper = &shell.cfg.wallpaper;
    let image_uri = match wallpaper {
        Wallpaper::Image(path) if path.is_file() => Some(gio::File::for_path(path).uri().to_string()),
        Wallpaper::Image(path) => {
            warn!("{name}: {} not found (KOMPACT_WALLPAPER), using the gradient", path.display());
            None
        }
        _ => None,
    };
    window.set_child(Some(&view));
    let uri = format!(
        "{}{}",
        gio::File::for_path(shell.cfg.page("wallpaper.html")).uri(),
        wallpaper.query(image_uri.as_deref())
    );
    view.load_uri(&uri);
    window.present();
    snapshot_later(&window, &view, "wallpaper");
    info!("{name}: layer surface created ({:?}, background layer), loading {uri}", monitor.geometry());

    let weak = window.downgrade();
    monitor.connect_invalidate(move |_| {
        if let Some(window) = weak.upgrade() {
            info!("{name}: monitor gone, closing");
            window.close();
        }
    });
}

/// 6c-2: the launcher. Built once at start-up and kept hidden, so opening it is quick.
struct Launcher {
    window: gtk::ApplicationWindow,
    view: WebView,
    open: Cell<bool>,
    /// The page has loaded and defined `window.kompactLauncher`.
    ready: Cell<bool>,
    /// An `open(...)` call made before the page was ready.
    pending: RefCell<Option<String>>,
    /// What the page was last given, to look up the app a `launch:` message names.
    apps: RefCell<Vec<App>>,
}

impl Launcher {
    fn open(&self) {
        let list = installed_apps();
        info!("launcher: opening ({} apps)", list.len());
        let script = format!("window.kompactLauncher.open({})", apps::apps_json(&list));
        *self.apps.borrow_mut() = list;
        // Exclusive: kompact gives us the keyboard while the launcher is up (6c-1).
        self.window.set_keyboard_mode(KeyboardMode::Exclusive);
        self.window.present();
        self.view.grab_focus();
        self.open.set(true);
        if self.ready.get() {
            run_script(&self.view, &script);
        } else {
            *self.pending.borrow_mut() = Some(script);
        }
    }

    fn close(&self, why: &str) {
        if !self.open.replace(false) {
            return;
        }
        // Give up the keyboard first: whether hiding destroys or just unmaps the layer surface,
        // kompact then hands focus back to the window underneath.
        self.window.set_keyboard_mode(KeyboardMode::None);
        self.window.set_visible(false);
        info!("launcher: closed ({why})");
    }
}

fn run_script(view: &WebView, script: &str) {
    view.evaluate_javascript(script, None, None, None::<&gio::Cancellable>, |result| {
        if let Err(e) = result {
            warn!("launcher script failed: {e}");
        }
    });
}

fn build_launcher(shell: &Rc<Shell>) -> Rc<Launcher> {
    let window = gtk::ApplicationWindow::new(&shell.app);
    window.set_title(Some("kompact launcher"));
    window.init_layer_shell();
    window.set_namespace(Some("kompact-launcher"));
    window.set_layer(Layer::Overlay);
    for edge in [Edge::Top, Edge::Bottom, Edge::Left, Edge::Right] {
        window.set_anchor(edge, true);
    }
    window.set_exclusive_zone(-1);
    window.set_keyboard_mode(KeyboardMode::None);
    // See-through: the page draws its own dimmed backdrop.
    window.add_css_class("kompact-launcher");
    if let Some(display) = gdk::Display::default() {
        let css = gtk::CssProvider::new();
        css.load_from_string("window.kompact-launcher { background: transparent; }");
        gtk::style_context_add_provider_for_display(&display, &css, gtk::STYLE_PROVIDER_PRIORITY_APPLICATION);
    }

    // Its own web process for now: while the white-screen report (6c-2) is open, nothing about
    // the launcher should touch the panel's and wallpaper's process.
    let view = web_view(shell, "launcher", false);
    view.set_background_color(&gdk::RGBA::new(0.0, 0.0, 0.0, 0.0));
    window.set_child(Some(&view));

    let launcher = Rc::new(Launcher {
        window,
        view: view.clone(),
        open: Cell::new(false),
        ready: Cell::new(false),
        pending: RefCell::new(None),
        apps: RefCell::new(Vec::new()),
    });
    let weak = Rc::downgrade(&launcher);
    view.connect_load_changed(move |view, event| {
        let Some(l) = weak.upgrade() else { return };
        if event == LoadEvent::Started {
            l.ready.set(false);
        } else if event == LoadEvent::Finished {
            l.ready.set(true);
            if let Some(script) = l.pending.borrow_mut().take() {
                run_script(view, &script);
            }
        }
    });
    // If its web process dies while open, close it rather than leave a blank surface holding
    // the keyboard (web_view() reloads the page).
    let weak = Rc::downgrade(&launcher);
    view.connect_web_process_terminated(move |_, _| {
        if let Some(l) = weak.upgrade() {
            l.close("web process ended");
        }
    });
    let uri = gio::File::for_path(shell.cfg.page("launcher.html")).uri();
    view.load_uri(&uri);
    info!("launcher: built on first use (overlay layer), loading {uri}");
    launcher
}

/// Diagnostics for the 6c-2 white-screen report: 2 s after start, save what WebKit drew for the
/// page (`<name>-page.png`) and what GTK draws for the page widget (`<name>-window.png`) to
/// `~/.cache/kompact-shell/`. If the page is right and the window isn't, the problem is GTK's;
/// if both are right but the screen is white, it's the compositor's.
fn snapshot_later(window: &gtk::ApplicationWindow, view: &WebView, name: &'static str) {
    let Some(dir) = std::env::var_os("HOME").map(|h| Path::new(&h).join(".cache/kompact-shell")) else { return };
    let (window, view) = (window.downgrade(), view.downgrade());
    glib::timeout_add_local_once(std::time::Duration::from_secs(2), move || {
        let (Some(window), Some(view)) = (window.upgrade(), view.upgrade()) else { return };
        if let Err(e) = std::fs::create_dir_all(&dir) {
            warn!("snapshot: can't create {}: {e}", dir.display());
            return;
        }
        let page = dir.join(format!("{name}-page.png"));
        view.snapshot(webkit6::SnapshotRegion::Visible, webkit6::SnapshotOptions::NONE, None::<&gio::Cancellable>, move |r| match r {
            Ok(t) => match t.save_to_png(&page) {
                Ok(()) => info!("snapshot: {name} page {}x{} -> {}", t.width(), t.height(), page.display()),
                Err(e) => warn!("snapshot: saving {}: {e}", page.display()),
            },
            Err(e) => warn!("snapshot: {name} page failed: {e}"),
        });
        // The page widget as GTK draws it into the window (a whole toplevel can't be captured).
        let (w, h) = (view.width(), view.height());
        let paintable = gtk::WidgetPaintable::new(Some(&view));
        let snap = gtk::Snapshot::new();
        paintable.snapshot(&snap, w as f64, h as f64);
        let renderer = window.native().and_then(|n| n.renderer());
        info!("snapshot: {name} window is {}x{}, visible {}, mapped {}", window.width(), window.height(), window.is_visible(), window.is_mapped());
        match (snap.to_node(), renderer) {
            (Some(node), Some(renderer)) => {
                let t = renderer.render_texture(&node, None);
                let path = dir.join(format!("{name}-window.png"));
                match t.save_to_png(&path) {
                    Ok(()) => info!("snapshot: {name} window {w}x{h} (renderer {}) -> {}", renderer.type_().name(), path.display()),
                    Err(e) => warn!("snapshot: saving {}: {e}", path.display()),
                }
            }
            (node, renderer) => warn!("snapshot: {name} window: nothing to render (node {}, renderer {})", node.is_some(), renderer.is_some()),
        }
    });
}
