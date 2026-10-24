//! kompact-shell: kompact's desktop shell (Phase 6), rebuilt on Tauri 2 (2026-10-08) after the
//! GTK4/WebKitGTK 6 shell drew only white on the GT 710 (that version is in `attic/shell-gtk4`).
//!
//! Each shell surface is a Tauri webview window turned into a wlr-layer-shell surface with
//! gtk-layer-shell (GTK3) before GTK maps it: the window is built hidden, made a layer surface,
//! then shown. Pages live in `../web` and are embedded into the binary at build time.
//!
//! 6t-a: the top panel (clock), reserving its height so maximized windows stay below it.
//! 6t-b: a wallpaper on the background layer of every monitor present at start-up.
//! 6t-c: macOS-style look: a see-through menu bar with an Apps button that opens a Launchpad-
//! style launcher.
//! 6m-a: Launchpad and Spotlight share one overlay surface (`overlay.rs`); the menu bar and
//! Launchpad show a blurred copy of the wallpaper (`blur.rs`), since kompact can't blur.
//! 6m-d: the dock (`dock.rs`), a bottom layer surface reserving its height.
//! 6m-c: kompact's private channel (`compositor.rs`): window list, Super+Space, Log Out.
//! 6m-b: the logo menu (About, Restart Shell, Restart…/Shut Down…) is a third overlay view.

mod apps;
mod assistant;
mod blur;
mod bluetooth;
mod calc;
mod compositor;
mod config;
mod datetime;
mod defaults;
mod dock;
mod keyboard;
mod login_prefs;
mod media;
mod menubar;
mod memstats;
mod network;
mod notify;
mod overlay;
mod panels;
mod polkit;
mod power;
mod screensaver;
mod settings;
mod sharing;
mod sound;
mod system;
mod thumbs;
mod users;
mod wallpaper;

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::OnceLock;

use gtk::prelude::*;
use gtk_layer_shell::{Edge, KeyboardMode, Layer, LayerShell};
use tauri::{AppHandle, Manager, WebviewUrl, WebviewWindowBuilder};
use tracing::{error, info, warn};
use tracing_subscriber::EnvFilter;
use webkit2gtk::{HardwareAccelerationPolicy, SettingsExt, WebContextExt, WebViewExt};

use config::{Cache, Gpu, WebProcess};
use overlay::Overlay;

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")))
        .init();

    // 6p-a: how much WebKit the pages use (see `config::WebProcess`, `config::Cache`).
    let lean = WebProcess::from_env(std::env::var("KOMPACT_SHELL_WEB_PROCESS").ok().as_deref())
        .and_then(|p| Ok((p, Cache::from_env(std::env::var("KOMPACT_SHELL_CACHE").ok().as_deref())?)));
    match lean {
        Ok((process, cache)) => {
            let _ = WEBKIT.set((process, cache));
            info!("WebKit: {process:?} web processes, {cache:?} cache (KOMPACT_SHELL_WEB_PROCESS, KOMPACT_SHELL_CACHE)");
        }
        Err(e) => {
            error!("{e}");
            std::process::exit(1);
        }
    }
    let gpu = match Gpu::from_env(std::env::var("KOMPACT_SHELL_GPU").ok().as_deref()) {
        Ok(g) => g,
        Err(e) => {
            error!("{e}");
            std::process::exit(1);
        }
    };
    let _ = SHELL_GPU.set(gpu);
    let compositing = match config::compositing_from_env(std::env::var("KOMPACT_SHELL_COMPOSITING").ok().as_deref()) {
        Ok(c) => c,
        Err(e) => {
            error!("{e}");
            std::process::exit(1);
        }
    };
    // 6t-a diagnostics: where to write the panel snapshots, if anywhere.
    let snapshot_dir = std::env::var_os("KOMPACT_SHELL_SNAPSHOT").map(PathBuf::from);
    // One shell per compositor: a second one would stack a second panel under the first.
    let _lock = match single_instance_lock() {
        Ok(lock) => lock,
        Err(e) => {
            error!("{e}");
            // 6d: tells kompact's supervisor not to start us again.
            std::process::exit(EXIT_ALREADY_RUNNING);
        }
    };
    // Before GTK and WebKit start: nothing else is running yet, so changing the environment is
    // safe.
    let set: Vec<_> = gpu.env_defaults(compositing, |k| std::env::var(k).ok());
    for (key, value) in &set {
        std::env::set_var(key, value);
    }
    info!(
        "kompact-shell (tauri) starting: WAYLAND_DISPLAY={}, gpu {:?} (KOMPACT_SHELL_GPU), compositing {} (KOMPACT_SHELL_COMPOSITING), set {:?}",
        std::env::var("WAYLAND_DISPLAY").unwrap_or_else(|_| "(unset)".into()),
        gpu,
        if compositing { "on" } else { "off" },
        set
    );

    // 10d-2: KOMPACT_WALLPAPER, else System Settings' choice, else KOMPACT_WALLPAPER_DEFAULT.
    let wallpapers = wallpaper::Wallpapers::load();
    let wallpaper_off = wallpapers.is_off();
    let our_env = set.iter().map(|(key, _)| *key).collect();
    let result = tauri::Builder::default()
        .manage(Overlay::new(our_env))
        .manage(compositor::Compositor::default())
        .manage(power::LockWatch::default())
        .manage(screensaver::ScreenSaver::default())
        .manage(assistant::run::Assistant::default())
        .manage(thumbs::Thumbs::default())
        .manage(notify::Notifications::new())
        .manage(wallpapers)
        .invoke_handler(tauri::generate_handler![
            overlay::toggle_overlay,
            overlay::select_region,
            overlay::close_overlay,
            overlay::switcher_state,
            overlay::open_status,
            overlay::status_target,
            overlay::overlay_ready,
            overlay::overlay_cleared,
            overlay::list_apps,
            overlay::launch,
            overlay::calculate,
            overlay::show_overlay,
            overlay::system_info,
            overlay::power,
            overlay::restart_shell,
            dock::set_pin_order,
            dock::dock_prefs,
            dock::set_dock_prefs,
            dock::dock_surface,
            dock::dock_bar,
            dock::dock_trash_full,
            dock::dock_open_trash,
            dock::dock_empty_trash,
            dock::dock_cycle,
            compositor::request_thumbs,
            compositor::compositor_state,
            compositor::sharing_state,
            compositor::keymap_state,
            settings::open_settings,
            settings::take_settings_pane,
            compositor::dock_icons,
            compositor::kompact_set,
            compositor::kompact_settings_state,
            compositor::keyboard_state,
            compositor::displays_state,
            compositor::idle_state,
            power::power_options,
            compositor::display_move,
            compositor::display_main,
            compositor::menu_bars,
            keyboard::keyboard_layouts,
            keyboard::keyboard_layout,
            datetime::datetime_state,
            defaults::defaults_state,
            users::users_state,
            users::set_real_name,
            users::set_login_prefs,
            users::choose_user_picture,
            users::choose_login_image,
            users::preview_lock,
            menubar::menubar_prefs,
            menubar::set_menubar_pref,
            defaults::defaults_set,
            system::about_state,
            datetime::datetime_zones,
            datetime::datetime_set_zone,
            datetime::datetime_set_ntp,
            datetime::set_clock_prefs,
            compositor::kompact_set_path,
            compositor::choose_shots_folder,
            compositor::open_folder,
            sharing::sharing_settings,
            sharing::set_sharing_fps,
            sharing::set_up_sharing,
            network::network_summary,
            network::network_state,
            network::network_wifi,
            network::network_scan,
            network::network_join,
            network::network_disconnect,
            network::network_forget,
            bluetooth::bluetooth_state,
            bluetooth::bluetooth_power,
            bluetooth::bluetooth_discover,
            bluetooth::bluetooth_connect,
            bluetooth::bluetooth_disconnect,
            bluetooth::bluetooth_pair,
            bluetooth::bluetooth_forget,
            bluetooth::bluetooth_pending,
            bluetooth::bluetooth_answer,
            sound::sound_state,
            sound::sound_level,
            sound::sound_volume,
            sound::sound_mute,
            sound::sound_default,
            media::media_state,
            media::media_control,
            screensaver::keep_awake_state,
            assistant::hud::assistant_toggle,
            assistant::hud::hud_hide,
            assistant::hud::hud_input,
            assistant::hud::hud_keyboard,
            assistant::run::assistant_listen,
            assistant::speech::speech_state,
            assistant::speech::set_speak,
            assistant::speech::speech_hush,
            assistant::run::assistant_ask,
            assistant::run::assistant_answer,
            assistant::run::assistant_undo,
            assistant::run::assistant_heard,
            assistant::voice::voice_state,
            assistant::voice::voice_stop,
            assistant::voice::voice_cancel,
            assistant::activity::assistant_activity,
            assistant::activity::assistant_activity_clear,
            screensaver::set_keep_awake,
            sound::osd_state,
            sound::osd_hide,
            polkit::polkit_request,
            polkit::polkit_answer,
            polkit::polkit_cancel,
            wallpaper::wallpaper_state,
            wallpaper::wallpaper_pictures,
            wallpaper::set_wallpaper,
            wallpaper::choose_wallpaper,
            settings::keys_command,
            compositor::window_action,
            dock::dock_items,
            dock::dock_click,
            dock::show_dock_menu,
            dock::dock_menu_info,
            dock::set_pinned,
            dock::dock_open,
            notify::notify_ready,
            notify::notify_closed,
            notify::notify_action,
            notify::notify_size,
            notify::notify_center,
            notify::notify_center_clear,
            notify::notify_center_open,
            notify::notify_prefs,
            notify::set_notify_prefs
        ])
        .register_asynchronous_uri_scheme_protocol(config::WALLPAPER_SCHEME, |ctx, request, responder| {
            ctx.app_handle().state::<wallpaper::Wallpapers>().serve(request.uri().path(), responder)
        })
        .register_uri_scheme_protocol(thumbs::THUMB_SCHEME, |ctx, request| {
            serve_thumb(&ctx.app_handle().state::<thumbs::Thumbs>(), request.uri().path())
        })
        .register_uri_scheme_protocol(config::ICON_SCHEME, |ctx, request| {
            serve_icon(&ctx.app_handle().state::<Overlay>(), request.uri().path())
        })
        .register_uri_scheme_protocol(media::ART_SCHEME, |_ctx, request| serve_art(request.uri().path()))
        .setup(move |app| {
            if !gtk_layer_shell::is_supported() {
                error!("the compositor doesn't offer wlr-layer-shell (is this running inside kompact?)");
                std::process::exit(1);
            }
            compositor::start(app.handle().clone());
            // 12a: NetworkManager's changes → kompact-network-changed.
            network::start(app.handle().clone());
            // 12b: BlueZ's changes and the pairing agent.
            bluetooth::start(app.handle().clone());
            // 12c: PipeWire's changes → kompact-sound-changed.
            sound::start(app.handle().clone());
            // 18a: the media players' changes → kompact-media-changed.
            media::start(app.handle().clone());
            // 12e: our polkit authentication agent for this session.
            polkit::start(app.handle().clone());
            // 12i-2: lock the screen before the computer sleeps.
            power::start(app.handle().clone());
            screensaver::start(app.handle().clone());
            // 14d: the login screen's copy of your lock look (nothing until it's installed).
            users::publish_soon();
            memstats::start();
            if wallpaper_off {
                info!("wallpaper: off (KOMPACT_WALLPAPER=none)");
            } else {
                build_wallpapers(app.handle(), gpu, snapshot_dir.clone())?;
            }
            // 12h-2: more menu bars come with kompact's `displays` message (`sync_panels`).
            app.manage(PanelMaker { gpu, snapshot_dir: snapshot_dir.clone(), count: std::sync::Mutex::new(1), main: Default::default() });
            build_panel(app.handle(), panels::FIRST, gpu, snapshot_dir.clone())?;
            if dock_disabled() {
                info!("dock: off (KOMPACT_SHELL_DOCK=off)");
            } else {
                build_dock(app.handle(), gpu, snapshot_dir.clone())?;
            }
            // 12c: the on-screen volume level (not tied to notifications).
            build_osd(app.handle(), gpu, snapshot_dir.clone())?;
            // 19h: the assistant's HUD, hidden until the menu bar's assistant item opens it.
            build_hud(app.handle(), gpu, snapshot_dir.clone())?;
            if notify::enabled_from_env(std::env::var("KOMPACT_SHELL_NOTIFICATIONS").ok().as_deref()) {
                build_notify(app.handle(), gpu, snapshot_dir.clone())?;
                notify::start(app.handle().clone());
            } else {
                info!("notifications: off (KOMPACT_SHELL_NOTIFICATIONS=off)");
            }
            if overlay_disabled() {
                info!("overlay: off (KOMPACT_SHELL_OVERLAY=off): no Launchpad or Spotlight");
            } else {
                build_overlay(app.handle(), gpu, snapshot_dir.clone())?;
            }
            Ok(())
        })
        .run(tauri::generate_context!());
    if let Err(e) = result {
        error!("kompact-shell: {e}");
        std::process::exit(1);
    }
    info!("kompact-shell exiting");
}

/// Lock `$XDG_RUNTIME_DIR/kompact-shell-<WAYLAND_DISPLAY>.lock`, or fail if another shell for
/// the same compositor holds it. `None` when there's no runtime dir (nothing to lock in).
/// 6d: exit codes kompact's shell supervisor understands (`kompact/src/shell_supervisor.rs`).
pub const EXIT_ALREADY_RUNNING: i32 = 3;
pub const EXIT_RESTART: i32 = 75;
/// Set by kompact when it started us (and will start us again when we exit with `EXIT_RESTART`).
pub const SUPERVISED_ENV: &str = "KOMPACT_SHELL_SUPERVISED";

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
            Err(format!("kompact-shell is already running on {display} (pkill -f kompact-shell to restart it)"))
        }
        Err(std::fs::TryLockError::Error(e)) => Err(format!("can't lock {}: {e}", path.display())),
    }
}

/// 6p-a: the WebKit switches, read once in `main`.
static WEBKIT: OnceLock<(WebProcess, Cache)> = OnceLock::new();
/// 10c: the GPU choice, for windows built after start-up (System Settings).
static SHELL_GPU: OnceLock<Gpu> = OnceLock::new();

/// 10c: the System Settings window, a normal app window (not a layer surface); one at a time,
/// destroyed when closed. Call on the GTK main thread. No GTK title bar: GTK3 can't ask kompact
/// for its bar (no xdg-decoration), so the page draws a macOS one (`web/settings.*`; moving and
/// resizing through `capabilities/settings.json`).
pub fn build_settings(app: &AppHandle) -> tauri::Result<()> {
    if let Some(window) = app.get_webview_window(settings::LABEL) {
        info!("settings: already open, bringing it forward");
        let _ = window.unminimize();
        window.show()?;
        return window.set_focus();
    }
    let window = WebviewWindowBuilder::new(app, settings::LABEL, WebviewUrl::App("settings.html".into()))
        .title("System Settings")
        .decorations(false)
        .inner_size(860.0, 620.0)
        .min_inner_size(680.0, 440.0)
        .shared()
        .build()?;
    let gpu = SHELL_GPU.get().copied().unwrap_or(Gpu::Off);
    configure_webview(&window, settings::LABEL.into(), gpu, (0.96, 0.96, 0.97, 1.0), None);
    window.on_window_event(|event| {
        if let tauri::WindowEvent::Destroyed = event {
            info!("settings: closed");
        }
    });
    info!("settings: opened");
    Ok(())
}

thread_local! {
    /// 6p-a: the first page's view; with `WebProcess::Shared` the others are built related to
    /// it, which puts them in its web process. GTK objects stay on the main thread.
    static FIRST_VIEW: std::cell::RefCell<Option<webkit2gtk::WebView>> = const { std::cell::RefCell::new(None) };
}

trait SharedProcess {
    /// Related to the first page's view when the pages share a web process (6p-a).
    fn shared(self) -> Self;
}

impl SharedProcess for WebviewWindowBuilder<'_, tauri::Wry, AppHandle> {
    fn shared(self) -> Self {
        if WEBKIT.get().map(|w| w.0) != Some(WebProcess::Shared) {
            return self;
        }
        match FIRST_VIEW.with(|v| v.borrow().clone()) {
            Some(view) => self.with_related_view(view),
            None => self,
        }
    }
}

/// The GT 710-friendly WebKit settings, and logging of what the page does.
/// `bg`: what the view shows before (and under) the page; alpha 0 for see-through surfaces.
fn configure_webview(window: &tauri::WebviewWindow, name: String, gpu: Gpu, bg: (f64, f64, f64, f64), snapshot_dir: Option<PathBuf>) {
    let result = window.with_webview(move |platform| {
        let name = name.as_str();
        let view = platform.inner();
        if let Some(settings) = WebViewExt::settings(&view) {
            settings.set_hardware_acceleration_policy(match gpu {
                Gpu::Off => HardwareAccelerationPolicy::Never,
                Gpu::On => HardwareAccelerationPolicy::Always,
            });
            // console.log() from the page lands in our log.
            settings.set_enable_write_console_messages_to_stdout(true);
        }
        let (r, g, b, a) = bg;
        view.set_background_color(&gtk::gdk::RGBA::new(r, g, b, a));
        // 6p-a: the first page: remember it, and set the cache model on the context all pages share.
        if FIRST_VIEW.with(|v| v.borrow().is_none()) {
            FIRST_VIEW.with(|v| *v.borrow_mut() = Some(view.clone()));
            if WEBKIT.get().map(|w| w.1) == Some(Cache::Viewer) {
                if let Some(context) = WebViewExt::context(&view) {
                    context.set_cache_model(webkit2gtk::CacheModel::DocumentViewer);
                    info!("WebKit: document-viewer cache model");
                }
            }
        }
        info!(
            "{name}: WebKit hardware acceleration policy {:?}",
            WebViewExt::settings(&view).map(|s| s.hardware_acceleration_policy())
        );
        // No WebKit right-click menu (Save Image, Reload…) on shell surfaces; the dock draws its
        // own (6m-d).
        view.connect_context_menu(|_, _, _, _| true);
        let n = name.to_owned();
        view.connect_size_allocate(move |_, a| {
            let name = &n;
            info!("{name}: webview allocated {}x{} at {},{}", a.width(), a.height(), a.x(), a.y());
        });
        let n = name.to_owned();
        view.connect_load_changed(move |view, event| {
            let name = n.clone();
            if event == webkit2gtk::LoadEvent::Finished {
                info!("{name}: page loaded");
                if let Some(dir) = snapshot_dir.clone() {
                    let view = view.clone();
                    // Give the page a moment to paint before looking at it.
                    gtk::glib::timeout_add_seconds_local_once(3, move || snapshot(&view, &name, &dir));
                }
            }
        });
        let n = name.to_owned();
        view.connect_load_failed(move |_, _, uri, err| {
            let name = &n;
            error!("{name}: loading {uri} failed: {err}");
            false
        });
        let n = name.to_owned();
        view.connect_web_process_terminated(move |view, reason| {
            let name = &n;
            warn!("{name}: web process ended ({reason:?}), reloading");
            view.reload();
        });
    });
    if let Err(e) = result {
        warn!("can't configure a webview: {e}");
    }
}

/// 6t-a diagnostics: write `<name>-webkit.png` (what WebKit rendered) and `<name>-gtk.png` (what
/// GTK draws for the whole window, the same path that fills the buffer sent to kompact).
fn snapshot(view: &webkit2gtk::WebView, name: &str, dir: &Path) {
    let gtk_path = dir.join(format!("{name}-gtk.png"));
    match view.toplevel() {
        Some(top) => {
            let (w, h) = (top.allocated_width(), top.allocated_height());
            info!("{name}: GTK window allocated {w}x{h}, mapped {}", top.is_mapped());
            let result = cairo::ImageSurface::create(cairo::Format::ARgb32, w.max(1), h.max(1))
                .map_err(|e| e.to_string())
                .and_then(|surface| {
                    let cr = cairo::Context::new(&surface).map_err(|e| e.to_string())?;
                    top.draw(&cr);
                    drop(cr);
                    write_png(&surface, &gtk_path)
                });
            match result {
                Ok(()) => info!("{name}: wrote {}", gtk_path.display()),
                Err(e) => warn!("{name}: can't write {}: {e}", gtk_path.display()),
            }
        }
        None => warn!("{name}: webview has no toplevel window"),
    }
    let webkit_path = dir.join(format!("{name}-webkit.png"));
    let name = name.to_owned();
    view.snapshot(
        webkit2gtk::SnapshotRegion::Visible,
        webkit2gtk::SnapshotOptions::NONE,
        None::<&gtk::gio::Cancellable>,
        move |result| {
            let written = result.map_err(|e| e.to_string()).and_then(|surface| {
                let image = cairo::ImageSurface::try_from(surface).map_err(|_| "not an image surface".to_string())?;
                info!("{name}: WebKit snapshot is {}x{}", image.width(), image.height());
                write_png(&image, &webkit_path)
            });
            match written {
                Ok(()) => info!("{name}: wrote {}", webkit_path.display()),
                Err(e) => warn!("{name}: WebKit snapshot failed: {e}"),
            }
        },
    );
}

fn write_png(surface: &cairo::ImageSurface, path: &Path) -> Result<(), String> {
    let mut file = std::fs::File::create(path).map_err(|e| e.to_string())?;
    surface.write_to_png(&mut file).map_err(|e| e.to_string())
}

const TRANSPARENT: (f64, f64, f64, f64) = (0.0, 0.0, 0.0, 0.0);

fn dock_disabled() -> bool {
    std::env::var("KOMPACT_SHELL_DOCK").is_ok_and(|v| v.trim().eq_ignore_ascii_case("off"))
}

/// 6m-d: the dock. Top layer, anchored bottom/left/right, `DOCK_HEIGHT` high and reserved
/// (maximized windows stop above it), see-through (the page draws the rounded bar in the middle),
/// never takes the keyboard.
fn build_dock(app: &AppHandle, gpu: Gpu, snapshot_dir: Option<PathBuf>) -> tauri::Result<()> {
    let window = WebviewWindowBuilder::new(app, "dock", WebviewUrl::App("dock.html".into()))
        .title("kompact dock")
        .visible(false)
        .decorations(false)
        .resizable(false)
        .inner_size(800.0, dock::DOCK_SURFACE_HEIGHT as f64)
        .transparent(true)
        .shared()
        .build()?;
    configure_webview(&window, "dock".into(), gpu, TRANSPARENT, snapshot_dir);
    let gtk_window = window.gtk_window()?;
    if gtk_window.is_realized() {
        warn!("dock: GTK window already realized; it may not become a layer surface");
    }
    gtk_window.init_layer_shell();
    gtk_window.set_namespace("kompact-dock");
    gtk_window.set_layer(Layer::Top);
    gtk_window.set_keyboard_mode(KeyboardMode::None);
    // 10d-1b: as big as the size set (the page keeps it up to date); 10d-1c: on its edge.
    let prefs = dock::dock_prefs();
    let zone = prefs.zone();
    anchor_dock(&gtk_window, prefs.position, zone);
    // 6m-d2 fix: tell the page when the pointer enters and leaves the dock, straight from GTK
    // (the wl_pointer enter/leave kompact sends). WebKit doesn't reliably pass a leave on as a
    // DOM event, and it can deliver a queued mouse move after this message, so the page goes
    // by these and ignores moves that arrive just after a leave (GT 710, 2026-10-09). The enter
    // carries the position, so a pointer arriving at the bottom edge reveals a hidden dock even
    // if no move follows.
    let (leave_page, enter_page) = (window.clone(), window.clone());
    let hooked = window.with_webview(move |platform| {
        let view = platform.inner();
        view.connect_leave_notify_event(move |_, ev| {
            if ev.detail() != gtk::gdk::NotifyType::Inferior {
                let _ = leave_page.eval("window.kompactDockPointer && window.kompactDockPointer(false)");
            }
            gtk::glib::Propagation::Proceed
        });
        view.connect_enter_notify_event(move |_, ev| {
            if ev.detail() != gtk::gdk::NotifyType::Inferior {
                let (x, y) = ev.position();
                let _ = enter_page.eval(&format!(
                    "window.kompactDockPointer && window.kompactDockPointer(true, {x:.1}, {y:.1})"
                ));
            }
            gtk::glib::Propagation::Proceed
        });
    });
    if let Err(e) = hooked {
        warn!("dock: can't watch the pointer leaving: {e}");
    }
    // 5j-3: kompact closes layer surfaces on a monitor that's unplugged, and gtk-layer-shell
    // turns that into a window close. The dock stays, and is remapped onto a remaining monitor.
    let handle = app.clone();
    window.on_window_event(move |event| {
        if let tauri::WindowEvent::CloseRequested { api, .. } = event {
            api.prevent_close();
            info!("dock: closed by kompact (its monitor went away), mapping it again");
            gtk::glib::timeout_add_local_once(std::time::Duration::from_millis(200), {
                let handle = handle.clone();
                move || remap_dock(&handle, "its monitor went away")
            });
        }
    });
    window.show()?;
    info!(
        "dock: layer surface created ({}px, top layer, {} edge, exclusive zone {}px)",
        dock::DOCK_SURFACE_HEIGHT,
        prefs.position.name(),
        zone
    );
    Ok(())
}

/// 5j-3: unmap and map the dock again. It asks for no monitor, so kompact puts the new layer
/// surface on the output under the pointer (the one kompact asked for with `dock_to`).
pub fn remap_dock(app: &AppHandle, why: &str) {
    let Some(window) = app.get_webview_window("dock") else { return };
    if let Err(e) = window.hide().and_then(|_| window.show()) {
        warn!("dock: can't remap ({why}): {e}");
        return;
    }
    info!("dock: remapped ({why})");
}

/// 10d-1c: anchor the dock's layer surface to `side`: along that edge, `DOCK_SURFACE_HEIGHT`
/// (`DOCK_SURFACE_SIDE` on a side) deep (taller than what it reserves, 6m-d2: room for magnified icons, labels and previews),
/// reserving `zone`, and taking the pointer only in the strip by the edge until the page has
/// laid the bar out and narrowed it (`dock_surface`).
fn anchor_dock(gtk_window: &gtk::ApplicationWindow, side: dock::Side, zone: i32) {
    let depth = if side == dock::Side::Bottom { dock::DOCK_SURFACE_HEIGHT } else { dock::DOCK_SURFACE_SIDE };
    let (edges, size, strip) = match side {
        dock::Side::Bottom => ([Edge::Bottom, Edge::Left, Edge::Right], (-1, depth), (0, depth - zone, 8192, zone)),
        dock::Side::Left => ([Edge::Left, Edge::Top, Edge::Bottom], (depth, -1), (0, 0, zone, 8192)),
        dock::Side::Right => ([Edge::Right, Edge::Top, Edge::Bottom], (depth, -1), (depth - zone, 0, zone, 8192)),
    };
    for edge in [Edge::Top, Edge::Bottom, Edge::Left, Edge::Right] {
        gtk_window.set_anchor(edge, edges.contains(&edge));
    }
    gtk_window.set_size_request(size.0, size.1);
    gtk_window.set_exclusive_zone(zone);
    let (x, y, w, h) = strip;
    let region = gtk::cairo::Region::create_rectangle(&gtk::cairo::RectangleInt::new(x, y, w, h));
    gtk_window.input_shape_combine_region(Some(&region));
}

/// 10d-1c: move the dock to another edge (Settings → Desktop & Dock → Position on screen).
pub fn place_dock(app: &AppHandle, side: dock::Side, zone: i32) -> Result<(), String> {
    let window = app.get_webview_window("dock").ok_or("no dock window")?;
    let gtk_window = window.gtk_window().map_err(|e| e.to_string())?;
    anchor_dock(&gtk_window, side, zone);
    // GTK resizes the window; the page lays the bar out again on its resize.
    let (w, h) = if side == dock::Side::Bottom { (800, dock::DOCK_SURFACE_HEIGHT) } else { (dock::DOCK_SURFACE_SIDE, 600) };
    gtk_window.resize(w, h);
    info!("dock: now on the {} edge", side.name());
    Ok(())
}

/// 8c: the notification banners, top-right: top layer, no reserved space, never the keyboard.
/// Hidden while there are none; the page sizes it to its banners (`notify::notify_size`). It
/// asks for no monitor, so it opens on the one under the pointer.
fn build_notify(app: &AppHandle, gpu: Gpu, snapshot_dir: Option<PathBuf>) -> tauri::Result<()> {
    let window = WebviewWindowBuilder::new(app, notify::LABEL, WebviewUrl::App("notify.html".into()))
        .title("kompact notifications")
        .visible(false)
        .decorations(false)
        .resizable(false)
        .inner_size(notify::WIDTH as f64, 120.0)
        .transparent(true)
        .shared()
        .build()?;
    configure_webview(&window, notify::LABEL.into(), gpu, TRANSPARENT, snapshot_dir);
    let gtk_window = window.gtk_window()?;
    if gtk_window.is_realized() {
        warn!("notify: GTK window already realized; it may not become a layer surface");
    }
    gtk_window.init_layer_shell();
    gtk_window.set_namespace("kompact-notify");
    gtk_window.set_layer(Layer::Top);
    for edge in [Edge::Top, Edge::Right] {
        gtk_window.set_anchor(edge, true);
    }
    gtk_window.set_layer_shell_margin(Edge::Top, 6);
    gtk_window.set_layer_shell_margin(Edge::Right, 6);
    gtk_window.set_exclusive_zone(0);
    gtk_window.set_keyboard_mode(KeyboardMode::None);
    gtk_window.set_size_request(notify::WIDTH, 120);
    // Like the dock: an unplugged monitor's layer surface is closed; keep the window.
    window.on_window_event(|event| {
        if let tauri::WindowEvent::CloseRequested { api, .. } = event {
            api.prevent_close();
        }
    });
    info!("notify: built (hidden until a notification comes, top layer, top-right)");
    Ok(())
}

/// 12c: the on-screen volume level (`sound.rs`): a 200 px square at the bottom middle, above
/// the dock, overlay layer (over full-screen windows too), never the pointer or keyboard. Hidden
/// until a volume key; the page hides it again (`osd_hide`).
fn build_osd(app: &AppHandle, gpu: Gpu, snapshot_dir: Option<PathBuf>) -> tauri::Result<()> {
    let window = WebviewWindowBuilder::new(app, sound::OSD_LABEL, WebviewUrl::App("osd.html".into()))
        .title("kompact level")
        .visible(false)
        .decorations(false)
        .resizable(false)
        .inner_size(200.0, 200.0)
        .transparent(true)
        .shared()
        .build()?;
    configure_webview(&window, sound::OSD_LABEL.into(), gpu, TRANSPARENT, snapshot_dir);
    let gtk_window = window.gtk_window()?;
    gtk_window.init_layer_shell();
    gtk_window.set_namespace("kompact-osd");
    gtk_window.set_layer(Layer::Overlay);
    gtk_window.set_anchor(Edge::Bottom, true);
    gtk_window.set_layer_shell_margin(Edge::Bottom, 140);
    gtk_window.set_exclusive_zone(0);
    gtk_window.set_keyboard_mode(KeyboardMode::None);
    gtk_window.set_size_request(200, 200);
    // Clicks go through to what's under it.
    gtk_window.input_shape_combine_region(Some(&gtk::cairo::Region::create()));
    window.on_window_event(|event| {
        if let tauri::WindowEvent::CloseRequested { api, .. } = event {
            api.prevent_close();
        }
    });
    info!("osd: built (hidden until a volume key)");
    Ok(())
}

/// 19h: the assistant's HUD (`assistant::hud`): a transparent surface on the top layer (above
/// windows, below full-screen ones and the overlay), top-right under the menu bar, hidden until
/// used; the keyboard only while typing to it, the pointer only on what the page draws.
fn build_hud(app: &AppHandle, gpu: Gpu, snapshot_dir: Option<PathBuf>) -> tauri::Result<()> {
    use assistant::hud;
    let window = WebviewWindowBuilder::new(app, hud::LABEL, WebviewUrl::App("hud.html".into()))
        .title("Siva")
        .visible(false)
        .decorations(false)
        .resizable(false)
        .inner_size(hud::WIDTH as f64, hud::HEIGHT as f64)
        .transparent(true)
        .shared()
        .build()?;
    configure_webview(&window, hud::LABEL.into(), gpu, TRANSPARENT, snapshot_dir);
    let gtk_window = window.gtk_window()?;
    gtk_window.init_layer_shell();
    gtk_window.set_namespace("kompact-assistant");
    gtk_window.set_layer(Layer::Top);
    gtk_window.set_anchor(Edge::Top, true);
    gtk_window.set_anchor(Edge::Right, true);
    gtk_window.set_layer_shell_margin(Edge::Top, hud::MARGIN_TOP);
    gtk_window.set_layer_shell_margin(Edge::Right, hud::MARGIN_RIGHT);
    gtk_window.set_exclusive_zone(0);
    gtk_window.set_keyboard_mode(KeyboardMode::None);
    gtk_window.set_size_request(hud::WIDTH, hud::HEIGHT);
    // Nothing takes the pointer until the page says what it draws.
    gtk_window.input_shape_combine_region(Some(&gtk::cairo::Region::create()));
    window.on_window_event(|event| {
        if let tauri::WindowEvent::CloseRequested { api, .. } = event {
            api.prevent_close();
        }
    });
    Ok(())
}

fn overlay_disabled() -> bool {
    std::env::var("KOMPACT_SHELL_OVERLAY").is_ok_and(|v| v.trim().eq_ignore_ascii_case("off"))
}

/// The overlay's layer surface (Launchpad, Spotlight), built hidden at start-up so opening it is
/// quick. Overlay layer, all edges, exclusive zone -1 (over the panel too); keyboard only while
/// open.
fn build_overlay(app: &AppHandle, gpu: Gpu, snapshot_dir: Option<PathBuf>) -> tauri::Result<()> {
    let window = WebviewWindowBuilder::new(app, overlay::LABEL, WebviewUrl::App("overlay.html".into()))
        .title("kompact overlay")
        .visible(false)
        .decorations(false)
        .resizable(false)
        // The page draws its own backdrop over the desktop.
        .transparent(true)
        .shared()
        .build()?;
    configure_webview(&window, overlay::LABEL.into(), gpu, TRANSPARENT, snapshot_dir);
    let gtk_window = window.gtk_window()?;
    if gtk_window.is_realized() {
        warn!("overlay: GTK window already realized; it may not become a layer surface");
    }
    gtk_window.init_layer_shell();
    gtk_window.set_namespace("kompact-overlay");
    gtk_window.set_layer(Layer::Overlay);
    for edge in [Edge::Top, Edge::Bottom, Edge::Left, Edge::Right] {
        gtk_window.set_anchor(edge, true);
    }
    gtk_window.set_exclusive_zone(-1);
    gtk_window.set_keyboard_mode(KeyboardMode::None);
    // 5j-3: open on a monitor that's unplugged, kompact closes it: hide it, don't lose it.
    let handle = app.clone();
    window.on_window_event(move |event| {
        if let tauri::WindowEvent::CloseRequested { api, .. } = event {
            api.prevent_close();
            handle.state::<Overlay>().close(&handle, "its monitor went away");
        }
    });
    info!("overlay: built (hidden, overlay layer)");
    Ok(())
}

/// 6t-c: answer `kompact-icon://localhost/<desktop id>` with that app's icon file.
fn serve_icon(overlay: &Overlay, path: &str) -> tauri::http::Response<Vec<u8>> {
    let not_found = || tauri::http::Response::builder().status(404).body(Vec::new()).unwrap_or_default();
    let Some(id) = path.strip_prefix('/').and_then(config::percent_decode) else { return not_found() };
    let Some(file) = overlay.icon_path(&id) else { return not_found() };
    match std::fs::read(&file) {
        Ok(bytes) => {
            // 14b: a user's picture has no extension: by its first bytes.
            let Some(mime) = config::image_mime(&file).or_else(|| config::sniff_mime(&bytes)) else { return not_found() };
            tauri::http::Response::builder()
            .header(tauri::http::header::CONTENT_TYPE, mime)
            .body(bytes)
            .unwrap_or_else(|_| not_found())
        }
        Err(e) => {
            warn!("icon for {id}: can't read {}: {e}", file.display());
            not_found()
        }
    }
}

/// 18a: `kompact-art://localhost/<n>`, a cover a media player named (`media::art_file`).
fn serve_art(path: &str) -> tauri::http::Response<Vec<u8>> {
    let not_found = || tauri::http::Response::builder().status(404).body(Vec::new()).unwrap_or_default();
    let Some(file) = media::art_file(path) else { return not_found() };
    let Ok(bytes) = std::fs::read(&file) else { return not_found() };
    let Some(mime) = config::image_mime(&file).or_else(|| config::sniff_mime(&bytes)) else { return not_found() };
    tauri::http::Response::builder()
        .header(tauri::http::header::CONTENT_TYPE, mime)
        .body(bytes)
        .unwrap_or_else(|_| not_found())
}

/// `kompact-thumb://localhost/<window id>` (6m-d3); any query (`?v=…`) is ignored.
fn serve_thumb(thumbs: &thumbs::Thumbs, path: &str) -> tauri::http::Response<Vec<u8>> {
    let png = path.trim_start_matches('/').parse::<u64>().ok().and_then(|id| thumbs.png(id));
    match png {
        Some(png) => tauri::http::Response::builder()
            .header(tauri::http::header::CONTENT_TYPE, "image/png")
            .header(tauri::http::header::CACHE_CONTROL, "no-store")
            .body(png)
            .unwrap_or_default(),
        None => tauri::http::Response::builder().status(404).body(Vec::new()).unwrap_or_default(),
    }
}

/// 12h-2: what `sync_panels` needs to make more menu bars.
pub struct PanelMaker {
    gpu: Gpu,
    snapshot_dir: Option<PathBuf>,
    /// Panel windows: `panel`, `panel-2`, ….
    count: std::sync::Mutex<usize>,
    /// The main display as of the last `displays` message.
    main: std::sync::Mutex<Option<String>>,
}

/// 12h-2: as many menu bars as Settings → Displays says, from kompact's `displays` message
/// (GTK main thread). kompact places each new one (main display first, then a display without
/// one); with one menu bar, it's remapped when it may be on the wrong display.
pub fn sync_panels(app: &AppHandle, msg: &serde_json::Value) {
    let Some(maker) = app.try_state::<PanelMaker>() else { return };
    let main = msg.get("primary").and_then(serde_json::Value::as_str).map(str::to_string);
    let before = std::mem::replace(&mut *maker.main.lock().unwrap_or_else(|e| e.into_inner()), main.clone());
    let mut count = maker.count.lock().unwrap_or_else(|e| e.into_inner());
    let plan = panels::plan(msg, *count, before.as_deref());
    while *count > plan.wanted {
        *count -= 1;
        let label = panels::label(*count);
        match app.get_webview_window(&label).map(|w| w.destroy()) {
            Some(Err(e)) => warn!("{label}: couldn't destroy it: {e}"),
            _ => info!("{label}: menu bar removed ({} wanted)", plan.wanted),
        }
    }
    while *count < plan.wanted {
        let label = panels::label(*count);
        if let Err(e) = build_panel(app, &label, maker.gpu, maker.snapshot_dir.clone()) {
            warn!("{label}: can't make another menu bar: {e}");
            break;
        }
        *count += 1;
    }
    if plan.remap_first {
        remap_panel(app, panels::FIRST, &format!("main display {}", main.as_deref().unwrap_or("?")));
    }
}

/// 12h-2: unmap and map a menu bar again; it asks for no monitor, so kompact places it anew.
fn remap_panel(app: &AppHandle, label: &str, why: &str) {
    let Some(window) = app.get_webview_window(label) else { return };
    match window.hide().and_then(|_| window.show()) {
        Ok(()) => info!("{label}: remapped ({why})"),
        Err(e) => warn!("{label}: can't remap ({why}): {e}"),
    }
}

fn build_panel(app: &AppHandle, label: &str, gpu: Gpu, snapshot_dir: Option<PathBuf>) -> tauri::Result<()> {
    let window = WebviewWindowBuilder::new(app, label, WebviewUrl::App("panel.html".into()))
        .title("kompact panel")
        // Hidden: gtk-layer-shell has to take the GTK window over before it is mapped.
        .visible(false)
        .decorations(false)
        .resizable(false)
        .inner_size(800.0, config::PANEL_HEIGHT as f64)
        // 6t-c: see-through like the macOS menu bar; the wallpaper shows under it (6m-a: or its
        // blurred copy, drawn by the page).
        .transparent(true)
        .shared()
        .build()?;
    configure_webview(&window, label.into(), gpu, TRANSPARENT, snapshot_dir);

    let gtk_window = window.gtk_window()?;
    if gtk_window.is_realized() {
        warn!("{label}: GTK window already realized; it may not become a layer surface");
    }
    gtk_window.init_layer_shell();
    gtk_window.set_namespace("kompact-panel");
    gtk_window.set_layer(Layer::Top);
    for edge in [Edge::Top, Edge::Left, Edge::Right] {
        gtk_window.set_anchor(edge, true);
    }
    // The panel never takes the keyboard.
    gtk_window.set_keyboard_mode(KeyboardMode::None);
    // Width comes from the left+right anchors; the height is ours, and is reserved.
    gtk_window.set_size_request(-1, config::PANEL_HEIGHT);
    gtk_window.auto_exclusive_zone_enable();
    // 12h-2: kompact closes a menu bar on a monitor that's unplugged; keep the window and map it
    // again (kompact puts it on a display without one, else the main display). `sync_panels`
    // removes it if there are now more than wanted.
    let (handle, name) = (app.clone(), label.to_string());
    window.on_window_event(move |event| {
        if let tauri::WindowEvent::CloseRequested { api, .. } = event {
            api.prevent_close();
            info!("{name}: closed by kompact (its monitor went away), mapping it again");
            let (handle, name) = (handle.clone(), name.clone());
            gtk::glib::timeout_add_local_once(std::time::Duration::from_millis(200), move || {
                remap_panel(&handle, &name, "its monitor went away")
            });
        }
    });
    window.show()?;
    info!("{label}: layer surface created ({}px, top layer, exclusive zone)", config::PANEL_HEIGHT);
    Ok(())
}

/// Longer side of the blurred copy, and its blur radius at that size: shown scaled up to the
/// screen, so it's small and cheap, and the blur hides the scaling.
pub(crate) const BLUR_SIZE: i32 = 480;
pub(crate) const BLUR_RADIUS: usize = 10;

/// Decode `file` small (`BLUR_SIZE`), blur it by `radius` (0: not at all), and encode it as PNG.
pub(crate) fn small_png(file: &Path, radius: usize) -> Option<Vec<u8>> {
    use gtk::gdk_pixbuf::{Colorspace, Pixbuf};
    let started = std::time::Instant::now();
    let result = (|| -> Result<Vec<u8>, String> {
        let (_, w, h) = Pixbuf::file_info(file).ok_or("unknown image format")?;
        let (sw, sh) = blur::small_size(w, h, BLUR_SIZE);
        let small = Pixbuf::from_file_at_scale(file, sw, sh, false).map_err(|e| e.to_string())?;
        let rgba = if small.has_alpha() { small } else { small.add_alpha(false, 0, 0, 0).map_err(|e| e.to_string())? };
        let (w, h, stride) = (rgba.width() as usize, rgba.height() as usize, rgba.rowstride() as usize);
        // The last row may be unpadded; pad it so every row is `stride` long.
        let mut pixels = rgba.read_pixel_bytes().to_vec();
        pixels.resize(stride * h, 0);
        blur::blur_rgba(&mut pixels, w, h, stride, radius);
        let out = Pixbuf::from_mut_slice(pixels, Colorspace::Rgb, true, 8, w as i32, h as i32, stride as i32);
        out.save_to_bufferv("png", &[]).map_err(|e| e.to_string())
    })();
    match result {
        Ok(bytes) => {
            let what = if radius > 0 { "blurred" } else { "small" };
            info!("wallpaper: {what} copy made in {} ms ({} KiB)", started.elapsed().as_millis(), bytes.len() / 1024);
            Some(bytes)
        }
        Err(e) => {
            warn!("wallpaper: can't make the small copy of {}: {e}", file.display());
            None
        }
    }
}

/// 6t-b: one wallpaper per monitor. 5g: also for monitors that appear later (kompact drives
/// every monitor since Phase 5f, and an HDMI monitor can be unplugged and plugged back in), and a
/// removed monitor's wallpaper window is destroyed, so replugging doesn't pile windows up.
fn build_wallpapers(app: &AppHandle, gpu: Gpu, snapshot_dir: Option<PathBuf>) -> tauri::Result<()> {
    let Some(display) = gtk::gdk::Display::default() else {
        warn!("wallpaper: no GDK display");
        return Ok(());
    };
    // (monitor, window label); GDK signals and these closures all run on the GTK main thread.
    let built: Rc<RefCell<Vec<(gtk::gdk::Monitor, String)>>> = Rc::default();
    for i in 0..display.n_monitors() {
        let Some(monitor) = display.monitor(i) else { continue };
        let name = build_wallpaper(app, &monitor, gpu, snapshot_dir.clone())?;
        built.borrow_mut().push((monitor, name));
    }
    {
        let (app, built) = (app.clone(), built.clone());
        display.connect_monitor_added(move |_, monitor| {
            // Built from an idle callback, after GDK has finished adding the monitor.
            let (app, built, monitor, dir) = (app.clone(), built.clone(), monitor.clone(), snapshot_dir.clone());
            gtk::glib::idle_add_local_once(move || match build_wallpaper(&app, &monitor, gpu, dir) {
                Ok(name) => built.borrow_mut().push((monitor, name)),
                Err(e) => warn!("wallpaper: none for the new monitor: {e}"),
            });
        });
    }
    {
        let (app, built) = (app.clone(), built.clone());
        display.connect_monitor_removed(move |_, monitor| {
            let mut built = built.borrow_mut();
            let Some(i) = built.iter().position(|(m, _)| m == monitor) else { return };
            let (_, name) = built.remove(i);
            match app.get_webview_window(&name).map(|w| w.destroy()) {
                Some(Ok(())) => info!("{name}: monitor removed, wallpaper window destroyed"),
                Some(Err(e)) => warn!("{name}: monitor removed, couldn't destroy its window: {e}"),
                None => info!("{name}: monitor removed (window already gone)"),
            }
        });
    }
    Ok(())
}

/// One wallpaper window, a background layer surface on `monitor`. Returns its window label.
fn build_wallpaper(app: &AppHandle, monitor: &gtk::gdk::Monitor, gpu: Gpu, snapshot_dir: Option<PathBuf>) -> tauri::Result<String> {
    // Labels are never reused: a window being destroyed may still hold its old one.
    static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let name = format!("wallpaper-{}", NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed));
    let window = WebviewWindowBuilder::new(app, &name, WebviewUrl::App("wallpaper.html".into()))
        .title("kompact wallpaper")
        .visible(false)
        .decorations(false)
        .resizable(false)
        .shared()
        .build()?;
    let (r, g, b) = config::PANEL_BG;
    configure_webview(&window, name.clone(), gpu, (r, g, b, 1.0), snapshot_dir);

    let gtk_window = window.gtk_window()?;
    if gtk_window.is_realized() {
        warn!("{name}: GTK window already realized; it may not become a layer surface");
    }
    gtk_window.init_layer_shell();
    gtk_window.set_namespace("kompact-wallpaper");
    gtk_window.set_layer(Layer::Background);
    for edge in [Edge::Top, Edge::Bottom, Edge::Left, Edge::Right] {
        gtk_window.set_anchor(edge, true);
    }
    // -1: cover the whole output, under panels too, and reserve nothing.
    gtk_window.set_exclusive_zone(-1);
    gtk_window.set_keyboard_mode(KeyboardMode::None);
    gtk_window.set_monitor(monitor);
    window.show()?;
    info!(
        "{name}: layer surface created on {} {:?} (background layer)",
        monitor.model().map(|m| m.to_string()).unwrap_or_else(|| "?".into()),
        monitor.geometry()
    );
    Ok(name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use gtk::gdk_pixbuf::{Colorspace, Pixbuf};

    #[test]
    fn blurred_copy_of_a_png() {
        // A 960x540 opaque image, half black, half white: the blurred copy is small, a PNG, and
        // grey along the seam.
        let (w, h) = (960, 540);
        let src = Pixbuf::new(Colorspace::Rgb, false, 8, w, h).unwrap();
        src.fill(0x000000ff);
        let white = Pixbuf::new(Colorspace::Rgb, false, 8, w / 2, h).unwrap();
        white.fill(0xffffffff);
        white.copy_area(0, 0, w / 2, h, &src, w / 2, 0);
        let path = std::env::temp_dir().join(format!("kompact-shell-blur-{}.png", std::process::id()));
        src.savev(&path, "png", &[]).unwrap();

        let png = small_png(&path, BLUR_RADIUS).expect("blurred copy");
        std::fs::remove_file(&path).unwrap();
        assert_eq!(&png[1..4], b"PNG");
        let out = gtk::gdk_pixbuf::PixbufLoader::new();
        out.write(&png).unwrap();
        out.close().unwrap();
        let out = out.pixbuf().unwrap();
        assert_eq!((out.width(), out.height()), blur::small_size(w, h, BLUR_SIZE));
        let px = out.read_pixel_bytes();
        let at = |x: i32| px[(out.height() / 2 * out.rowstride() + x * out.n_channels()) as usize];
        assert_eq!(at(0), 0, "far left stays black");
        assert_eq!(at(out.width() - 1), 255, "far right stays white");
        let seam = at(out.width() / 2);
        assert!(seam > 60 && seam < 200, "seam is grey: {seam}");
    }

    /// `KOMPACT_BLUR_SAMPLE=<image> cargo test -- --ignored`: write its blurred copy next to
    /// the scratch dir to look at, and print how long it took.
    #[test]
    #[ignore]
    fn blurred_copy_sample() {
        let Some(file) = std::env::var_os("KOMPACT_BLUR_SAMPLE") else { return };
        let started = std::time::Instant::now();
        let png = small_png(Path::new(&file), BLUR_RADIUS).expect("blurred copy");
        println!("blurred in {} ms, {} bytes", started.elapsed().as_millis(), png.len());
        std::fs::write(std::env::temp_dir().join("kompact-blur-sample.png"), png).unwrap();
    }
}
