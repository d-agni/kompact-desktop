//! kompact-shell as confirmed at 6b (2026-10-08), rebuilt unchanged as a separate program to test
//! whether the 6c-2 white screen comes from the new shell code or from the machine.
//!
//! 6a: the top panel (clock), reserving its height so maximized windows stay below it.
//! 6b: a wallpaper on the background layer of every monitor.

mod config;

use std::cell::RefCell;
use std::path::Path;
use std::rc::Rc;

use gtk4_layer_shell::{Edge, KeyboardMode, Layer, LayerShell};
use tracing::{error, info, warn};
use tracing_subscriber::EnvFilter;
use webkit6::prelude::*;
use webkit6::{gdk, gio, glib, gtk, HardwareAccelerationPolicy, LoadEvent, WebView};

use config::{Config, Gpu, Wallpaper};

fn main() -> glib::ExitCode {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")))
        .init();

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
    for (key, value) in cfg.env_defaults(get) {
        std::env::set_var(key, value);
    }
    info!(
        "kompact-shell-6b starting: WAYLAND_DISPLAY={}, web {}, gpu {:?} (KOMPACT_SHELL_GPU), GSK_RENDERER={}, GDK_DEBUG={}",
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
    let cfg = Rc::new(cfg);
    app.connect_activate(move |app| {
        match gdk::Display::default() {
            Some(d) => info!("connected to display {}", d.name()),
            None => warn!("no display (not running inside kompact?)"),
        }
        if !gtk4_layer_shell::is_supported() {
            error!("the compositor doesn't offer wlr-layer-shell (is this running inside kompact?)");
            app.quit();
            return;
        }
        let views = Views::default();
        start_wallpapers(app, &cfg, &views);
        build_panel(app, &cfg, &views);
    });
    // GTK would otherwise try to parse our arguments.
    let code = app.run_with_args::<&str>(&[]);
    info!("kompact-shell-6b exiting ({code:?})");
    code
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

/// Every shell page's view, so a new one can join an existing view's web process.
type Views = Rc<RefCell<Vec<glib::WeakRef<WebView>>>>;

/// A WebKit view for one shell page, with the GT 710-friendly settings.
///
/// All views share one web process: each new view is created "related" to a live one. WebKit
/// otherwise starts a web process per view, about 55–60 MB each on the GT 710 machine. Related
/// views also share their `Settings`. If the shared process crashes, every page reloads.
fn web_view(cfg: &Config, name: &str, views: &Views) -> WebView {
    let name: Rc<str> = name.into();
    let related = views.borrow().iter().find_map(|w| w.upgrade());
    let view = match &related {
        Some(r) => WebView::builder().related_view(r).build(),
        None => WebView::new(),
    };
    {
        let mut views = views.borrow_mut();
        views.retain(|w| w.upgrade().is_some());
        views.push(view.downgrade());
    }
    info!(
        "{name}: {}",
        if related.is_some() { "joining the shared web process" } else { "starting the shared web process" }
    );
    if let Some(settings) = WebViewExt::settings(&view) {
        // Pages are local files; the wallpaper loads an image from anywhere on disk.
        settings.set_allow_file_access_from_file_urls(true);
        settings.set_hardware_acceleration_policy(match cfg.gpu {
            Gpu::Off => HardwareAccelerationPolicy::Never,
            Gpu::On => HardwareAccelerationPolicy::Always,
        });
        // console.log() from the page lands in our log.
        settings.set_enable_write_console_messages_to_stdout(true);
    }
    let (r, g, b) = config::PANEL_BG;
    view.set_background_color(&gdk::RGBA::new(r, g, b, 1.0));
    let n = name.clone();
    view.connect_load_changed(move |_, event| {
        let name = &n;
        if event == LoadEvent::Finished {
            info!("{name}: page loaded");
        }
    });
    let n = name.clone();
    view.connect_load_failed(move |_, _, uri, err| {
        let name = &n;
        error!("{name}: loading {uri} failed: {err}");
        false
    });
    view.connect_web_process_terminated(move |view, reason| {
        // Reload once; a page that keeps crashing stays blank rather than looping.
        warn!("{name}: web process ended ({reason:?}), reloading");
        view.reload();
    });
    view
}

fn build_panel(app: &gtk::Application, cfg: &Config, views: &Views) {
    let window = gtk::ApplicationWindow::new(app);
    window.set_title(Some("kompact panel"));
    window.init_layer_shell();
    window.set_namespace(Some("kompact-panel"));
    window.set_layer(Layer::Top);
    for edge in [Edge::Top, Edge::Left, Edge::Right] {
        window.set_anchor(edge, true);
    }
    // The panel never takes the keyboard (nothing to type into yet).
    window.set_keyboard_mode(KeyboardMode::None);
    // Width comes from the left+right anchors; the height is ours, and is reserved.
    window.set_default_size(0, config::PANEL_HEIGHT);
    window.auto_exclusive_zone_enable();

    let view = web_view(cfg, "panel", views);
    view.set_size_request(-1, config::PANEL_HEIGHT);
    window.set_child(Some(&view));
    let uri = gio::File::for_path(cfg.page("panel.html")).uri();
    view.load_uri(&uri);
    window.present();
    info!("panel: layer surface created ({}px, top layer, exclusive zone), loading {uri}", config::PANEL_HEIGHT);
}

/// 6b: one wallpaper per monitor, now and for monitors that appear later (each closes itself
/// when its monitor goes away).
fn start_wallpapers(app: &gtk::Application, cfg: &Rc<Config>, views: &Views) {
    if cfg.wallpaper == Wallpaper::Off {
        info!("wallpaper: off (KOMPACT_WALLPAPER=none)");
        return;
    }
    let Some(display) = gdk::Display::default() else { return };
    let monitors = display.monitors();
    for i in 0..monitors.n_items() {
        if let Some(monitor) = monitors.item(i).and_downcast::<gdk::Monitor>() {
            build_wallpaper(app, cfg, &monitor, views);
        }
    }
    let (app, cfg, views) = (app.clone(), cfg.clone(), views.clone());
    monitors.connect_items_changed(move |list, position, _removed, added| {
        for i in position..position + added {
            if let Some(monitor) = list.item(i).and_downcast::<gdk::Monitor>() {
                build_wallpaper(&app, &cfg, &monitor, &views);
            }
        }
    });
}

fn build_wallpaper(app: &gtk::Application, cfg: &Config, monitor: &gdk::Monitor, views: &Views) {
    let name = format!("wallpaper {}", monitor.connector().unwrap_or_else(|| "?".into()));
    let window = gtk::ApplicationWindow::new(app);
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

    let view = web_view(cfg, &name, views);
    let image_uri = match &cfg.wallpaper {
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
        gio::File::for_path(cfg.page("wallpaper.html")).uri(),
        cfg.wallpaper.query(image_uri.as_deref())
    );
    view.load_uri(&uri);
    window.present();
    info!("{name}: layer surface created ({:?}, background layer), loading {uri}", monitor.geometry());

    let weak = window.downgrade();
    monitor.connect_invalidate(move |_| {
        if let Some(window) = weak.upgrade() {
            info!("{name}: monitor gone, closing");
            window.close();
        }
    });
}
