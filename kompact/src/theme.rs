//! 6m-f: a macOS-like look for apps in the kompact session only.
//!
//! `tools/install-mac-theme.sh` (no sudo) installs the WhiteSur GTK theme, icons and cursors in
//! your home and writes a dconf profile, `~/.config/kompact/dconf/profile`:
//!
//! ```text
//! user-db:user
//! file-db:~/.config/kompact/dconf/kompact
//! ```
//!
//! Your own settings (`user-db:user`) still apply, except the keys the `file-db` locks (theme,
//! icons, cursor, title-bar buttons): those take its values. Your GNOME/Ubuntu session doesn't
//! use this profile, so it keeps its own look (a GSettings override wouldn't do: values you set
//! yourself, such as `Yaru-dark`, win over it).
//!
//! When that profile exists, kompact (DRM backend, at start, before anything is spawned) sets
//! `DCONF_PROFILE` to it and exports it to the user session with `WAYLAND_DISPLAY`
//! (`session_env.rs`), so apps started by D-Bus get it too. GTK 3 apps take the theme from it;
//! libadwaita apps (Nautilus, Text Editor) keep their own look, with the icons, cursor and
//! dark/light. 13e: kompact used to force the theme on them with `GTK_THEME`, but WhiteSur's
//! GTK 4 stylesheet predates libadwaita 1.6's style variables, so Nautilus (libadwaita 1.9) drew
//! no window backgrounds at all, only text and icons; `GTK_THEME` is no longer set, and a
//! leftover one in the user session is cleared at start (`session_env`). `KOMPACT_THEME=off`
//! leaves the profile unset. Snap apps (Firefox) can't see themes in your home and keep their own.

//!
//! 10d-3b: without that profile, apps follow System Settings → Appearance live. kompact writes
//! its own profile, `~/.config/kompact/dconf/live-profile`:
//!
//! ```text
//! user-db:kompact
//! user-db:user
//! ```
//!
//! so in kompact sessions your settings (`user`) still apply, but writes go to a database of the
//! session's own (`~/.config/dconf/kompact`). On every Appearance change kompact sets there the
//! colour scheme, the light or dark variant of your GTK theme (Yaru ↔ Yaru-dark), the accent
//! and the title-bar button layout (`gsettings`, with that profile); dconf tells running apps,
//! and the settings portal tells Firefox and libadwaita apps. Your GNOME session never reads it.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// The profile the install script writes, under `$XDG_CONFIG_HOME` (or `~/.config`).
pub fn profile_path(config_home: &Path) -> PathBuf {
    config_home.join("kompact/dconf/profile")
}

/// The dconf keyfile it is compiled from (next to it), which kompact reads the theme name from.
pub fn keyfile_path(config_home: &Path) -> PathBuf {
    config_home.join("kompact/dconf/kompact.d/00-kompact")
}

/// `gtk-theme` from the `[org/gnome/desktop/interface]` group of a dconf keyfile.
pub fn gtk_theme(keyfile: &str) -> Option<String> {
    let mut in_group = false;
    for line in keyfile.lines().map(str::trim) {
        if line.starts_with('[') {
            in_group = line == "[org/gnome/desktop/interface]";
        } else if in_group {
            if let Some(value) = line.strip_prefix("gtk-theme") {
                let value = value.trim_start().strip_prefix('=')?.trim();
                let name = value.strip_prefix('\'')?.strip_suffix('\'')?;
                return (!name.is_empty()).then(|| name.to_string());
            }
        }
    }
    None
}

/// What the environment needs: `DCONF_PROFILE`, only if not set already (13e: no `GTK_THEME`).
pub fn vars(off: bool, profile: Option<&Path>, set_already: impl Fn(&str) -> bool) -> Vec<(&'static str, String)> {
    let Some(profile) = profile.filter(|_| !off) else { return Vec::new() };
    if set_already("DCONF_PROFILE") {
        return Vec::new();
    }
    vec![("DCONF_PROFILE", profile.display().to_string())]
}

/// 10d-3b: the live profile: the session's own writable database over yours.
pub const LIVE_PROFILE: &str = "user-db:kompact\nuser-db:user\n";

pub fn live_profile_path(config_home: &Path) -> PathBuf {
    config_home.join("kompact/dconf/live-profile")
}

/// The live profile in use (set by `apply`), if apps follow Appearance.
static LIVE: OnceLock<Option<PathBuf>> = OnceLock::new();

/// The theme without its dark ending (`Yaru-dark` → `Yaru`).
pub fn base_theme(name: &str) -> &str {
    for end in ["-dark", "-Dark", "_dark", "-darker"] {
        if let Some(b) = name.strip_suffix(end) {
            return b;
        }
    }
    name
}

/// The light or dark variant of `base`, if that theme is installed (`exists`), else `base`.
pub fn theme_variant(base: &str, dark: bool, exists: impl Fn(&str) -> bool) -> String {
    if dark {
        for name in [format!("{base}-dark"), format!("{base}-Dark")] {
            if exists(&name) {
                return name;
            }
        }
    }
    base.to_string()
}

/// kompact's accents in GNOME's names (`org.gnome.desktop.interface accent-color`).
pub fn gnome_accent(accent: &str) -> &'static str {
    match accent {
        "purple" => "purple",
        "pink" => "pink",
        "red" => "red",
        "orange" => "orange",
        "yellow" => "yellow",
        "green" => "green",
        "graphite" => "slate",
        _ => "blue",
    }
}

/// The settings apps get for this look: (schema, key, GVariant text).
pub fn app_settings(dark: bool, accent: &str, buttons_right: bool, theme: &str) -> Vec<(&'static str, &'static str, String)> {
    let layout = if buttons_right { ":minimize,maximize,close" } else { "close,minimize,maximize:" };
    vec![
        ("org.gnome.desktop.interface", "color-scheme", format!("'{}'", if dark { "prefer-dark" } else { "prefer-light" })),
        ("org.gnome.desktop.interface", "gtk-theme", format!("'{theme}'")),
        ("org.gnome.desktop.interface", "accent-color", format!("'{}'", gnome_accent(accent))),
        ("org.gnome.desktop.wm.preferences", "button-layout", format!("'{layout}'")),
    ]
}

fn theme_installed(name: &str) -> bool {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let mut dirs = vec![PathBuf::from("/usr/share/themes")];
    if let Some(h) = &home {
        dirs.push(h.join(".themes"));
        dirs.push(h.join(".local/share/themes"));
    }
    dirs.iter().any(|d| d.join(name).join("gtk-3.0").is_dir())
}

/// Your own GTK theme (read without kompact's profile), once.
fn user_theme() -> String {
    static THEME: OnceLock<String> = OnceLock::new();
    THEME
        .get_or_init(|| {
            let out = std::process::Command::new("gsettings")
                .args(["get", "org.gnome.desktop.interface", "gtk-theme"])
                .env_remove("DCONF_PROFILE")
                .output();
            let name = out.ok().and_then(|o| String::from_utf8(o.stdout).ok()).unwrap_or_default();
            let name = name.trim().trim_matches('\'').to_string();
            if name.is_empty() { "Adwaita".into() } else { name }
        })
        .clone()
}

/// 10d-3b: give apps this look (only what changed since last time, on its own thread).
pub fn sync_apps(dark: bool, accent: &str, buttons_right: bool) {
    let Some(Some(profile)) = LIVE.get() else { return };
    static LAST: std::sync::Mutex<String> = std::sync::Mutex::new(String::new());
    let sig = format!("{dark} {accent} {buttons_right}");
    {
        let mut last = LAST.lock().unwrap_or_else(|e| e.into_inner());
        if *last == sig {
            return;
        }
        *last = sig;
    }
    let (profile, accent) = (profile.clone(), accent.to_string());
    std::thread::spawn(move || {
        let user = user_theme();
        let theme = theme_variant(base_theme(&user), dark, theme_installed);
        let mut failed = Vec::new();
        for (schema, key, value) in app_settings(dark, &accent, buttons_right, &theme) {
            let ok = std::process::Command::new("gsettings")
                .args(["set", schema, key, &value])
                .env("DCONF_PROFILE", &profile)
                .stderr(std::process::Stdio::null())
                .status()
                .is_ok_and(|s| s.success());
            if !ok {
                failed.push(key); // e.g. accent-color on an older GNOME
            }
        }
        tracing::info!(
            "theme: apps now {} ({theme}, accent {}, buttons {}){}",
            if dark { "dark" } else { "light" },
            gnome_accent(&accent),
            if buttons_right { "right" } else { "left" },
            if failed.is_empty() { String::new() } else { format!("; not set: {}", failed.join(", ")) }
        );
    });
}

/// Set them (called from `main` before any thread or client starts, so changing the environment
/// is safe) and return them for `session_env` to export.
pub fn apply() -> Vec<(&'static str, String)> {
    let off = std::env::var("KOMPACT_THEME").is_ok_and(|v| v.trim().eq_ignore_ascii_case("off"));
    let config = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")));
    let Some(config) = config else { return Vec::new() };
    let profile = profile_path(&config);
    let found = profile.is_file().then_some(profile.as_path());
    let keyfile = std::fs::read_to_string(keyfile_path(&config)).ok();
    let set_already = |name: &str| std::env::var_os(name).is_some_and(|v| !v.is_empty());
    if !off && found.is_none() && !set_already("DCONF_PROFILE") {
        // 10d-3b: apps follow Appearance live.
        let live = live_profile_path(&config);
        let written = live.parent().map(std::fs::create_dir_all).transpose().and_then(|_| std::fs::write(&live, LIVE_PROFILE));
        match written {
            Ok(()) => {
                std::env::set_var("DCONF_PROFILE", &live);
                let _ = LIVE.set(Some(live.clone()));
                tracing::info!("theme: apps follow Settings → Appearance (DCONF_PROFILE={}: the session's own settings over yours)", live.display());
                return vec![("DCONF_PROFILE", live.display().to_string())];
            }
            Err(e) => tracing::warn!("theme: can't write {}: {e}; apps keep your theme", live.display()),
        }
    }
    let _ = LIVE.set(None);
    let set = vars(off, found, set_already);
    if let Some(theme) = keyfile.as_deref().and_then(gtk_theme).filter(|_| !off && found.is_some()) {
        tracing::info!("theme: {theme} for GTK 3 apps; libadwaita apps (Nautilus) keep their own look, with its icons and cursor");
    }
    for (name, value) in &set {
        std::env::set_var(name, value);
    }
    match (off, found) {
        (true, _) => tracing::info!("theme: off (KOMPACT_THEME=off)"),
        (false, None) => tracing::info!("theme: none installed (tools/install-mac-theme.sh)"),
        (false, Some(p)) => tracing::info!("theme: {} from {} (locked: apps don't follow Appearance)", describe(&set), p.display()),
    }
    set
}

fn describe(set: &[(&str, String)]) -> String {
    if set.is_empty() {
        return "DCONF_PROFILE already set, kept".into();
    }
    set.iter().map(|(n, v)| format!("{n}={v}")).collect::<Vec<_>>().join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;

    const KEYFILE: &str = "[org/gnome/desktop/wm/preferences]\ngtk-theme='Not-this'\n\n[org/gnome/desktop/interface]\nicon-theme='WhiteSur'\ngtk-theme = 'WhiteSur-Light'\n";

    #[test]
    fn theme_from_keyfile() {
        assert_eq!(gtk_theme(KEYFILE).as_deref(), Some("WhiteSur-Light"));
        assert_eq!(gtk_theme("[org/gnome/desktop/interface]\nicon-theme='x'\n"), None);
        assert_eq!(gtk_theme("[org/gnome/desktop/interface]\ngtk-theme=''\n"), None);
        assert_eq!(gtk_theme("[other]\ngtk-theme='x'\n"), None);
    }

    #[test]
    fn live_switching() {
        assert_eq!(base_theme("Yaru-dark"), "Yaru");
        assert_eq!(base_theme("Adwaita-dark"), "Adwaita");
        assert_eq!(base_theme("Yaru-blue-dark"), "Yaru-blue");
        assert_eq!(base_theme("Yaru"), "Yaru");
        let has = |n: &str| ["Yaru-dark", "WhiteSur-Dark"].contains(&n);
        assert_eq!(theme_variant("Yaru", true, has), "Yaru-dark");
        assert_eq!(theme_variant("Yaru", false, has), "Yaru");
        assert_eq!(theme_variant("WhiteSur", true, has), "WhiteSur-Dark");
        assert_eq!(theme_variant("Plain", true, has), "Plain", "no dark one installed");
        assert_eq!(gnome_accent("graphite"), "slate");
        assert_eq!(gnome_accent("mauve"), "blue");
        let set = app_settings(false, "green", true, "Yaru");
        assert_eq!(set[0], ("org.gnome.desktop.interface", "color-scheme", "'prefer-light'".to_string()));
        assert_eq!(set[1].2, "'Yaru'");
        assert_eq!(set[2].2, "'green'");
        assert_eq!(set[3], ("org.gnome.desktop.wm.preferences", "button-layout", "':minimize,maximize,close'".to_string()));
        assert_eq!(app_settings(true, "blue", false, "Yaru-dark")[3].2, "'close,minimize,maximize:'");
        assert!(LIVE_PROFILE.starts_with("user-db:kompact\n"), "the session's own database is the one written");
        assert_eq!(live_profile_path(Path::new("/c")), Path::new("/c/kompact/dconf/live-profile"));
    }

    #[test]
    fn paths() {
        let c = Path::new("/home/u/.config");
        assert_eq!(profile_path(c), Path::new("/home/u/.config/kompact/dconf/profile"));
        assert_eq!(keyfile_path(c), Path::new("/home/u/.config/kompact/dconf/kompact.d/00-kompact"));
    }

    #[test]
    fn only_what_is_missing() {
        let p = Path::new("/c/kompact/dconf/profile");
        let none = |_: &str| false;
        // 13e: the profile only, never GTK_THEME.
        assert_eq!(vars(false, Some(p), none), [("DCONF_PROFILE", "/c/kompact/dconf/profile".to_string())]);
        // Not installed, or switched off: nothing.
        assert!(vars(false, None, none).is_empty());
        assert!(vars(true, Some(p), none).is_empty());
        // Something the user set themselves is kept.
        assert!(vars(false, Some(p), |n| n == "DCONF_PROFILE").is_empty());
        assert_eq!(gtk_theme(KEYFILE).as_deref(), Some("WhiteSur-Light"));
    }
}
