mod activation;
mod anim;
mod backend_udev;
mod backend_winit;
mod cursor;
mod decoration;
mod dock_edge;
mod dmabuf;
mod focus;
mod frames;
mod geometry_memory;
mod greeter;
mod idle;
mod input;
mod lock;
mod ipc;
mod outputs;
mod overlay;
mod perf;
mod quit_signal;
mod placement;
mod popup;
mod prefs;
mod resize;
mod render;
mod screencopy;
mod keymap;
mod screenshot;
mod scroll;
mod session_env;
mod shell;
mod shell_supervisor;
mod snap;
mod state;
mod stats;
mod switcher;
mod theme;
mod thumbnail;
mod title_text;
mod titlebar;
mod typing;
mod uploads;
mod virtual_display;
mod vt;
mod workspace;

use clap::{Parser, ValueEnum};

/// calloop's `InsertError` and friends hold non-Send/Sync handles, so `?` can't turn them into
/// `anyhow::Error` directly. `.ctx()?` flattens them to a message first.
pub trait Ctx<T> {
    fn ctx(self) -> anyhow::Result<T>;
}
impl<T, E: std::fmt::Display> Ctx<T> for Result<T, E> {
    fn ctx(self) -> anyhow::Result<T> {
        self.map_err(|e| anyhow::anyhow!("{e}"))
    }
}

#[derive(Copy, Clone, Debug, ValueEnum)]
enum Backend {
    /// Nested window inside an existing Wayland/X11 session — use this in your Alpine VM.
    Winit,
    /// Direct DRM/KMS on real hardware, driven through libseat. Run from a TTY, no X/Wayland
    /// session underneath.
    Drm,
}

#[derive(Parser, Debug)]
#[command(name = "kompact", version, about = "A minimal Wayland compositor")]
struct Args {
    /// Which backend to run. If omitted, kompact picks drm when $WAYLAND_DISPLAY and
    /// $DISPLAY are both unset (i.e. you're on a bare TTY), winit otherwise.
    #[arg(long, value_enum)]
    backend: Option<Backend>,

    /// Command to launch on startup (e.g. a terminal). Defaults to `foot`, falling back to
    /// `alacritty`, falling back to `xterm` if run under XWayland.
    #[arg(long)]
    spawn: Option<String>,

    /// Give keyboard focus to whatever window the pointer moves onto (without raising it).
    /// Off by default: click to focus. Can also be toggled at runtime with Super+M.
    #[arg(long)]
    focus_follows_mouse: bool,

    /// 14d: start no app at startup (a login session, not a test run); Super+Return still opens
    /// the terminal found on $PATH (or the one Settings names).
    #[arg(long)]
    no_startup_app: bool,

    /// 14c-2: run only this login screen (greetd's greeter, `kompact-greeter`) and quit when
    /// it exits: no shell, no lock, no shortcuts, nothing exported to the user session.
    #[arg(long, value_name = "COMMAND")]
    greeter: Option<String>,
}

fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info".into()),
        )
        .init();

    let args = Args::parse();
    // 15a: which build this is, first thing in every log.
    tracing::info!("kompact {} starting", env!("CARGO_PKG_VERSION"));
    ensure_runtime_dir();

    let backend = args.backend.unwrap_or_else(|| {
        let headless_tty = std::env::var_os("WAYLAND_DISPLAY").is_none()
            && std::env::var_os("DISPLAY").is_none();
        if headless_tty {
            tracing::info!("no WAYLAND_DISPLAY/DISPLAY set, assuming bare TTY -> drm backend");
            Backend::Drm
        } else {
            tracing::info!("nested session detected -> winit backend");
            Backend::Winit
        }
    });

    focus::set_focus_follows_mouse_at_start(args.focus_follows_mouse);
    tracing::info!("focus-follows-mouse: {}", if args.focus_follows_mouse { "on" } else { "off" });
    // 6m-e7: read the title font now, not on the first frame with a title bar.
    if titlebar::enabled() {
        title_text::init();
    }

    let mut spawn_cmd = if args.no_startup_app { String::new() } else { args.spawn.unwrap_or_else(default_spawn_command) };
    // 14c-2: the login screen is the startup command; the desktop's parts stay off.
    if let Some(cmd) = args.greeter.as_deref().map(str::trim).filter(|c| !c.is_empty()) {
        greeter::set(cmd);
        spawn_cmd = cmd.to_string();
        for (name, value) in greeter::env_changes() {
            // Before any thread starts, as set_desktop_name does.
            match value {
                Some(v) => std::env::set_var(name, v),
                None => std::env::remove_var(name),
            }
        }
        tracing::info!("greeter mode: only {cmd:?}, quitting when it exits (no shell, lock, shortcuts or session exports)");
    }

    // Only on bare metal: nested under winit we must not touch the host session's environment.
    // 14c-2: nor in greeter mode (the greeter's user isn't the one logging in).
    session_env::set_enabled(matches!(backend, Backend::Drm) && !greeter::active());
    session_env::set_desktop_name();
    session_env::set_session_type();
    if matches!(backend, Backend::Drm) && !greeter::active() {
        session_env::also_export(theme::apply());
    }
    quit_signal::install();

    let result = match backend {
        Backend::Winit => backend_winit::run(spawn_cmd),
        Backend::Drm => backend_udev::run(spawn_cmd),
    };
    session_env::clear();
    ipc::cleanup();
    thumbnail::cleanup();
    shell_supervisor::cleanup();
    result
}

pub(crate) fn default_spawn_command() -> String {
    for candidate in ["foot", "alacritty", "xterm"] {
        if which(candidate) {
            return candidate.to_string();
        }
    }
    "foot".to_string()
}

fn which(bin: &str) -> bool {
    std::env::var_os("PATH")
        .map(|paths| {
            std::env::split_paths(&paths).any(|dir| dir.join(bin).is_file())
        })
        .unwrap_or(false)
}

/// Wayland sockets live in $XDG_RUNTIME_DIR. On a bare Alpine VT nothing (no logind/pam_rundir)
/// sets it, so create a private per-user fallback rather than failing to open the socket.
fn ensure_runtime_dir() {
    if std::env::var_os("XDG_RUNTIME_DIR").is_some() {
        return;
    }
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    let Ok(meta) = std::fs::metadata("/proc/self") else { return };
    let dir = std::path::PathBuf::from(format!("/tmp/kompact-runtime-{}", meta.uid()));
    if std::fs::create_dir_all(&dir).is_ok() {
        let _ = std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700));
        tracing::warn!("XDG_RUNTIME_DIR was unset, using {:?}", dir);
        std::env::set_var("XDG_RUNTIME_DIR", &dir);
    }
}
