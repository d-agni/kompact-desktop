//! 12j: System Settings → Default Apps: the web browser, mail app and file manager other apps
//! open (links, `mailto:`, folders; `xdg-open` and GLib read the same), and the terminal
//! kompact's Open a terminal shortcut starts.
//!
//! The first three are the freedesktop `mimeapps.list` defaults: read in the spec's order
//! (`mimeapps_files`), written to the user's `~/.config/mimeapps.list` only, other lines kept.
//! The terminal is kompact's `terminal` setting (`set-path terminal <command>`; empty: the one
//! it was started with), since no standard names a default terminal.

use std::path::PathBuf;

use serde_json::{json, Value};
use tauri::{AppHandle, Manager, State};
use tracing::{info, warn};

use crate::apps::{self, App};
use crate::compositor::Compositor;
use crate::overlay::Overlay;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    Browser,
    Mail,
    Files,
    Terminal,
}

pub const ROLES: [Role; 4] = [Role::Browser, Role::Mail, Role::Files, Role::Terminal];

impl Role {
    pub fn parse(s: &str) -> Option<Role> {
        ROLES.into_iter().find(|r| r.name() == s)
    }

    pub fn name(self) -> &'static str {
        match self {
            Role::Browser => "browser",
            Role::Mail => "mail",
            Role::Files => "files",
            Role::Terminal => "terminal",
        }
    }

    /// The types it's the default for; the first decides which apps can be it.
    pub fn mimes(self) -> &'static [&'static str] {
        match self {
            Role::Browser => &["x-scheme-handler/http", "x-scheme-handler/https", "text/html", "application/xhtml+xml"],
            Role::Mail => &["x-scheme-handler/mailto"],
            Role::Files => &["inode/directory"],
            Role::Terminal => &[],
        }
    }
}

fn list(v: &str) -> impl Iterator<Item = &str> {
    v.split(';').map(str::trim).filter(|i| !i.is_empty())
}

/// Whether `app` can be the default for `role`.
pub fn handles(app: &App, role: Role) -> bool {
    match role.mimes().first() {
        Some(mime) => list(&app.mime_types).any(|m| m == *mime),
        None => list(&app.categories).any(|c| c == "TerminalEmulator"),
    }
}

/// Apps that can be it, by name.
pub fn candidates(apps: &[App], role: Role) -> Vec<&App> {
    let mut out: Vec<&App> = apps.iter().filter(|a| handles(a, role)).collect();
    out.sort_by_key(|a| a.name.to_lowercase());
    out
}

/// The desktop ids listed for `mime` under `[Default Applications]`.
pub fn defaults_in(text: &str, mime: &str) -> Vec<String> {
    let mut section = false;
    for line in text.lines().map(str::trim) {
        if line.starts_with('[') {
            section = line == "[Default Applications]";
        } else if section {
            if let Some((k, v)) = line.split_once('=') {
                if k.trim() == mime {
                    return list(v).map(str::to_string).collect();
                }
            }
        }
    }
    Vec::new()
}

/// The default for `mime`: the first installed id in the first file (highest priority first)
/// that names one.
pub fn current(files: &[String], mime: &str, installed: impl Fn(&str) -> bool) -> Option<String> {
    files.iter().find_map(|text| defaults_in(text, mime).into_iter().find(|id| installed(id)))
}

/// `text` (a `mimeapps.list`, maybe empty) with `id` the default for each of `mimes`; other
/// sections, lines and comments kept.
pub fn set_default(text: &str, mimes: &[&str], id: &str) -> String {
    let mut out: Vec<String> = Vec::new();
    let mut section = false;
    let mut found = false;
    let new_lines = || mimes.iter().map(|m| format!("{m}={id};")).collect::<Vec<_>>();
    for line in text.lines() {
        let t = line.trim();
        if t.starts_with('[') {
            if section {
                out.extend(new_lines());
            }
            section = t == "[Default Applications]";
            found |= section;
            out.push(line.to_string());
            continue;
        }
        if section && t.split_once('=').is_some_and(|(k, _)| mimes.contains(&k.trim())) {
            continue;
        }
        out.push(line.to_string());
    }
    if section {
        out.extend(new_lines());
    }
    if !found {
        if out.last().is_some_and(|l| !l.trim().is_empty()) {
            out.push(String::new());
        }
        out.push("[Default Applications]".into());
        out.extend(new_lines());
    }
    let mut s = out.join("\n");
    s.push('\n');
    s
}

/// `mimeapps.list` files in the spec's order: config home, config dirs, data home, data dirs
/// (`applications/`), each with the desktop's own `<desktop>-mimeapps.list` first; the old
/// `defaults.list` last in the data dirs.
pub fn mimeapps_files(get: impl Fn(&str) -> Option<String>) -> Vec<PathBuf> {
    let home = get("HOME").map(PathBuf::from);
    let nonempty = |k: &str| get(k).filter(|v| !v.is_empty());
    let config_home = nonempty("XDG_CONFIG_HOME").map(PathBuf::from).or_else(|| home.as_ref().map(|h| h.join(".config")));
    let data_home = nonempty("XDG_DATA_HOME").map(PathBuf::from).or_else(|| home.as_ref().map(|h| h.join(".local/share")));
    let config_dirs = nonempty("XDG_CONFIG_DIRS").unwrap_or_else(|| "/etc/xdg".into());
    let data_dirs = nonempty("XDG_DATA_DIRS").unwrap_or_else(|| "/usr/local/share:/usr/share".into());
    let desktops: Vec<String> = get("XDG_CURRENT_DESKTOP").unwrap_or_default().split(':').filter(|d| !d.is_empty()).map(str::to_lowercase).collect();
    let mut out = Vec::new();
    let mut add = |dir: PathBuf, legacy: bool| {
        for d in &desktops {
            out.push(dir.join(format!("{d}-mimeapps.list")));
        }
        out.push(dir.join("mimeapps.list"));
        if legacy {
            out.push(dir.join("defaults.list"));
        }
    };
    if let Some(c) = config_home {
        add(c, false);
    }
    for d in config_dirs.split(':').filter(|d| !d.is_empty()) {
        add(PathBuf::from(d), false);
    }
    if let Some(d) = data_home {
        add(d.join("applications"), false);
    }
    for d in data_dirs.split(':').filter(|d| !d.is_empty()) {
        add(PathBuf::from(d).join("applications"), true);
    }
    out
}

/// The command kompact runs for a terminal app: its `Exec=` words, or `None` if one has a space
/// in it (kompact splits the command at spaces) or it starts a server, not a window
/// (`foot --server`).
pub fn terminal_command(app: &App) -> Option<String> {
    let argv = apps::exec_argv(&app.exec)?;
    let server = argv.iter().any(|a| a == "--server" || a == "--daemon");
    (!server && argv.iter().all(|a| !a.contains(char::is_whitespace))).then(|| argv.join(" "))
}

/// `set-path terminal <command>` (empty: kompact's own default).
pub fn terminal_line(command: &str) -> Option<String> {
    let ok = command.len() <= 300 && !command.contains(|c: char| c.is_control());
    ok.then(|| format!("set-path terminal {command}\n"))
}

fn user_file() -> Option<PathBuf> {
    mimeapps_files(|k| std::env::var(k).ok()).into_iter().find(|p| p.file_name().is_some_and(|n| n == "mimeapps.list"))
}

fn read_files() -> Vec<String> {
    mimeapps_files(|k| std::env::var(k).ok()).iter().filter_map(|p| std::fs::read_to_string(p).ok()).collect()
}

fn entry(a: &App) -> Value {
    json!({"id": a.id, "name": a.name})
}

/// Each role's choices and what's chosen now (`null`: none set, or not installed). For the
/// terminal each choice also has the command kompact would run, to match its setting.
#[tauri::command]
pub fn defaults_state(overlay: State<'_, Overlay>) -> Value {
    overlay.refresh_apps();
    let all = overlay.apps();
    let files = read_files();
    let mut out = serde_json::Map::new();
    for role in ROLES {
        let apps = candidates(&all, role);
        let value = match role.mimes().first() {
            Some(mime) => json!({
                "apps": apps.iter().map(|a| entry(a)).collect::<Vec<_>>(),
                "current": current(&files, mime, |id| apps.iter().any(|a| a.id == id)),
            }),
            None => json!({
                "apps": apps.iter().filter_map(|a| terminal_command(a).map(|c| json!({"id": a.id, "name": a.name, "command": c}))).collect::<Vec<_>>(),
            }),
        };
        out.insert(role.name().into(), value);
    }
    Value::Object(out)
}

/// Make `id` the default for `role` (`""` for the terminal: kompact's own).
#[tauri::command]
pub fn defaults_set(app: AppHandle, overlay: State<'_, Overlay>, role: String, id: String) -> Result<(), String> {
    let role = Role::parse(&role).ok_or_else(|| format!("no default app {role:?}"))?;
    let chosen = if id.is_empty() && role == Role::Terminal {
        None
    } else {
        let a = overlay.app_entry(&id).filter(|a| handles(a, role)).ok_or_else(|| format!("{id} can't be the {} app", role.name()))?;
        Some(a)
    };
    if role == Role::Terminal {
        let command = match &chosen {
            Some(a) => terminal_command(a).ok_or_else(|| format!("{}'s command has a space kompact can't pass", a.name))?,
            None => String::new(),
        };
        let line = terminal_line(&command).ok_or("that command can't be sent")?;
        info!("default apps: terminal {}", if command.is_empty() { "kompact's own" } else { &command });
        return app.state::<Compositor>().send_line_pub("set", &line);
    }
    let path = user_file().ok_or("no home directory")?;
    let old = std::fs::read_to_string(&path).unwrap_or_default();
    let text = set_default(&old, role.mimes(), &id);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    // Written whole and renamed into place, so a reader never sees half a file.
    let tmp = path.with_extension("list.kompact-tmp");
    std::fs::write(&tmp, text).and_then(|_| std::fs::rename(&tmp, &path)).map_err(|e| {
        warn!("default apps: {}: {e}", path.display());
        format!("{}: {e}", path.display())
    })?;
    info!("default apps: {} = {id} (saved to {})", role.name(), path.display());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app(id: &str, name: &str, mimes: &str, cats: &str, exec: &str) -> App {
        App {
            id: id.into(),
            name: name.into(),
            comment: String::new(),
            keywords: String::new(),
            exec: exec.into(),
            try_exec: None,
            icon: String::new(),
            wm_class: String::new(),
            mime_types: mimes.into(),
            categories: cats.into(),
        }
    }

    #[test]
    fn who_can_be_which() {
        let apps = [
            app("firefox_firefox.desktop", "Firefox", "text/html;x-scheme-handler/http;x-scheme-handler/https;", "Network;WebBrowser;", "firefox %u"),
            app("thunderbird.desktop", "Thunderbird", "message/rfc822;x-scheme-handler/mailto;", "Network;Email;", "thunderbird %u"),
            app("org.gnome.Nautilus.desktop", "Files", "inode/directory;application/x-7z-compressed;", "GNOME;Utility;", "nautilus --new-window %U"),
            app("org.gnome.Ptyxis.desktop", "Ptyxis", "", "GNOME;GTK;System;TerminalEmulator;", "ptyxis --new-window"),
            app("foot.desktop", "Foot", "", "System;TerminalEmulator;", "foot"),
            app("gedit.desktop", "Text Editor", "text/plain;", "Utility;", "gedit %U"),
        ];
        let ids = |r| candidates(&apps, r).iter().map(|a| a.id.as_str()).collect::<Vec<_>>();
        assert_eq!(ids(Role::Browser), ["firefox_firefox.desktop"]);
        assert_eq!(ids(Role::Mail), ["thunderbird.desktop"]);
        assert_eq!(ids(Role::Files), ["org.gnome.Nautilus.desktop"]);
        // By name.
        assert_eq!(ids(Role::Terminal), ["foot.desktop", "org.gnome.Ptyxis.desktop"]);
        assert_eq!(terminal_command(&apps[3]).as_deref(), Some("ptyxis --new-window"));
        assert_eq!(terminal_command(&app("t", "T", "", "", "\"/opt/My Term/term\"")), None);
        assert_eq!(terminal_command(&app("foot-server.desktop", "Foot Server", "", "TerminalEmulator;", "foot --server")), None);
        assert_eq!(terminal_line("foot").as_deref(), Some("set-path terminal foot\n"));
        assert_eq!(terminal_line("").as_deref(), Some("set-path terminal \n"));
        assert_eq!(terminal_line("foot\nquit"), None);
        assert_eq!(Role::parse("mail"), Some(Role::Mail));
        assert_eq!(Role::parse("editor"), None);
    }

    #[test]
    fn reading_defaults() {
        let user = "[Added Associations]\nx-scheme-handler/http=chromium.desktop;\n\n[Default Applications]\nx-scheme-handler/claude-cli=claude.desktop\n";
        let system = "[Default Applications]\nx-scheme-handler/http=firefox_firefox.desktop;firefox.desktop;\ninode/directory=org.gnome.Nautilus.desktop;\n";
        let files = [user.to_string(), system.to_string()];
        let installed = |id: &str| id != "firefox_firefox.desktop";
        // Not in [Added Associations]; the first installed one in the list.
        assert_eq!(current(&files, "x-scheme-handler/http", installed).as_deref(), Some("firefox.desktop"));
        assert_eq!(current(&files, "inode/directory", installed).as_deref(), Some("org.gnome.Nautilus.desktop"));
        assert_eq!(current(&files, "x-scheme-handler/mailto", installed), None);
        // The user's file first.
        let mine = "[Default Applications]\nx-scheme-handler/http=chromium.desktop;\n".to_string();
        assert_eq!(current(&[mine, system.into()], "x-scheme-handler/http", |_| true).as_deref(), Some("chromium.desktop"));
    }

    #[test]
    fn writing_defaults() {
        let mimes = Role::Browser.mimes();
        // An empty file.
        let t = set_default("", &["x-scheme-handler/mailto"], "thunderbird.desktop");
        assert_eq!(t, "[Default Applications]\nx-scheme-handler/mailto=thunderbird.desktop;\n");
        // Old entries replaced, others and other sections kept.
        let old = "# mine\n[Default Applications]\nx-scheme-handler/claude-cli=claude.desktop\nx-scheme-handler/http=firefox.desktop;\ntext/html=firefox.desktop\n\n[Added Associations]\ntext/plain=gedit.desktop;\n";
        let t = set_default(old, mimes, "chromium.desktop");
        for m in mimes {
            assert_eq!(defaults_in(&t, m), ["chromium.desktop"]);
        }
        assert_eq!(defaults_in(&t, "x-scheme-handler/claude-cli"), ["claude.desktop"]);
        assert!(t.starts_with("# mine\n[Default Applications]\n"));
        assert!(t.ends_with("[Added Associations]\ntext/plain=gedit.desktop;\n"));
        assert_eq!(t.matches("x-scheme-handler/http=").count(), 1);
        // Only other sections: one added at the end.
        let t = set_default("[Added Associations]\ntext/plain=gedit.desktop;", &["inode/directory"], "nemo.desktop");
        assert_eq!(t, "[Added Associations]\ntext/plain=gedit.desktop;\n\n[Default Applications]\ninode/directory=nemo.desktop;\n");
        // Set twice: the same.
        let once = set_default(old, mimes, "chromium.desktop");
        assert_eq!(set_default(&once, mimes, "chromium.desktop"), once);
    }

    #[test]
    fn where_the_files_are() {
        let env = |k: &str| match k {
            "HOME" => Some("/home/u".to_string()),
            "XDG_CURRENT_DESKTOP" => Some("kompact".to_string()),
            "XDG_DATA_DIRS" => Some("/usr/share".to_string()),
            _ => None,
        };
        let files: Vec<String> = mimeapps_files(env).iter().map(|p| p.display().to_string()).collect();
        assert_eq!(
            files,
            [
                "/home/u/.config/kompact-mimeapps.list",
                "/home/u/.config/mimeapps.list",
                "/etc/xdg/kompact-mimeapps.list",
                "/etc/xdg/mimeapps.list",
                "/home/u/.local/share/applications/kompact-mimeapps.list",
                "/home/u/.local/share/applications/mimeapps.list",
                "/usr/share/applications/kompact-mimeapps.list",
                "/usr/share/applications/mimeapps.list",
                "/usr/share/applications/defaults.list",
            ]
        );
    }
}
