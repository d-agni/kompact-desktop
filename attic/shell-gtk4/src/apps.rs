//! Installed applications for the launcher (6c-2): parse freedesktop `.desktop` files, turn an
//! `Exec=` line into an argv, and list the data directories to look in. Parsing is pure and
//! tested; `scan` and `launch` do the file system and process work.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct App {
    /// Desktop file id: path under `applications/` with `/` as `-` (`org.gnome.gedit.desktop`).
    pub id: String,
    pub name: String,
    pub comment: String,
    pub keywords: String,
    pub exec: String,
    /// `TryExec=`: the app is only listed if this program exists.
    pub try_exec: Option<String>,
}

/// Parse one `.desktop` file. `None` = not something to list: not an application, hidden,
/// `NoDisplay`, needs a terminal, only meant for other desktops, or no name/command.
pub fn parse_desktop_entry(id: &str, text: &str) -> Option<App> {
    let mut in_entry = false;
    let mut keys: BTreeMap<&str, &str> = BTreeMap::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if line.starts_with('[') {
            in_entry = line == "[Desktop Entry]";
            continue;
        }
        if !in_entry {
            continue;
        }
        if let Some((k, v)) = line.split_once('=') {
            // Localized keys (`Name[de]`) are skipped: the plain one is the default.
            let k = k.trim();
            if !k.contains('[') {
                keys.entry(k).or_insert(v.trim());
            }
        }
    }
    let flag = |k: &str| keys.get(k).is_some_and(|v| v.eq_ignore_ascii_case("true"));
    if keys.get("Type") != Some(&"Application") || flag("NoDisplay") || flag("Hidden") || flag("Terminal") {
        return None;
    }
    if keys.contains_key("OnlyShowIn") {
        // Meant for GNOME, KDE, ... only (settings panels and the like).
        return None;
    }
    let name = unescape(keys.get("Name")?);
    let exec = keys.get("Exec")?.to_string();
    if name.is_empty() || exec.is_empty() {
        return None;
    }
    Some(App {
        id: id.to_string(),
        name,
        comment: keys.get("Comment").map(|c| unescape(c)).unwrap_or_default(),
        keywords: keys.get("Keywords").map(|k| unescape(k).replace(';', " ").trim().to_string()).unwrap_or_default(),
        exec,
        try_exec: keys.get("TryExec").map(|t| t.to_string()).filter(|t| !t.is_empty()),
    })
}

/// The `\s \n \t \r \\` escapes of desktop-entry string values.
fn unescape(v: &str) -> String {
    let mut out = String::with_capacity(v.len());
    let mut chars = v.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('s') => out.push(' '),
            Some('n') => out.push('\n'),
            Some('t') => out.push('\t'),
            Some('r') => out.push('\r'),
            Some(other) => out.push(other),
            None => {}
        }
    }
    out
}

/// Split an `Exec=` value into argv, per the desktop entry spec: whitespace-separated, double
/// quotes group (with `\"`, `` \` ``, `\$` and `\\` escaped inside), field codes (`%f %u %F ...`)
/// dropped since we never pass files, `%%` is a literal `%`. `None` if it's empty or a quote is
/// left open.
pub fn exec_argv(exec: &str) -> Option<Vec<String>> {
    let mut args = Vec::new();
    let mut cur = String::new();
    let mut has_arg = false;
    let mut quoted = false;
    let mut chars = exec.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '"' => {
                quoted = !quoted;
                has_arg = true;
            }
            '\\' if quoted => {
                if let Some(n) = chars.next() {
                    cur.push(n);
                }
            }
            c if c.is_whitespace() && !quoted => {
                if has_arg {
                    args.push(std::mem::take(&mut cur));
                    has_arg = false;
                }
            }
            '%' => match chars.next() {
                Some('%') => {
                    cur.push('%');
                    has_arg = true;
                }
                // A field code; dropped (with its argument slot, if it stood alone).
                Some(_) | None => {}
            },
            c => {
                cur.push(c);
                has_arg = true;
            }
        }
    }
    if quoted {
        return None;
    }
    if has_arg {
        args.push(cur);
    }
    args.retain(|a| !a.is_empty());
    (!args.is_empty()).then_some(args)
}

/// Directories holding `applications/`, highest priority first: `$XDG_DATA_HOME` (default
/// `~/.local/share`), then `$XDG_DATA_DIRS` (default `/usr/local/share:/usr/share`), then the
/// snap and flatpak export dirs if those aren't listed already (a session started from a text
/// console often lacks them).
pub fn data_dirs(get: impl Fn(&str) -> Option<String>) -> Vec<PathBuf> {
    let home = get("HOME").filter(|h| !h.is_empty());
    let mut dirs = Vec::new();
    match get("XDG_DATA_HOME").filter(|d| !d.is_empty()) {
        Some(d) => dirs.push(PathBuf::from(d)),
        None => {
            if let Some(h) = &home {
                dirs.push(Path::new(h).join(".local/share"));
            }
        }
    }
    let system = get("XDG_DATA_DIRS").filter(|d| !d.is_empty()).unwrap_or_else(|| "/usr/local/share:/usr/share".into());
    dirs.extend(system.split(':').filter(|d| !d.is_empty()).map(PathBuf::from));
    let mut extra = vec![PathBuf::from("/var/lib/snapd/desktop"), PathBuf::from("/var/lib/flatpak/exports/share")];
    if let Some(h) = &home {
        extra.push(Path::new(h).join(".local/share/flatpak/exports/share"));
    }
    for d in extra {
        if !dirs.contains(&d) {
            dirs.push(d);
        }
    }
    let mut seen = Vec::new();
    dirs.retain(|d| {
        let new = !seen.contains(d);
        seen.push(d.clone());
        new
    });
    dirs
}

/// Every listable app under `dirs` (see `data_dirs`), sorted by name. A desktop file id found
/// in an earlier directory hides the same id in later ones, even if the earlier one is hidden
/// (that's how a user turns a system entry off).
pub fn scan(dirs: &[PathBuf], exists: impl Fn(&str) -> bool) -> Vec<App> {
    let mut by_id: BTreeMap<String, Option<App>> = BTreeMap::new();
    for dir in dirs {
        let root = dir.join("applications");
        let mut files = Vec::new();
        collect_desktop_files(&root, &mut files, 0);
        for path in files {
            let Ok(rel) = path.strip_prefix(&root) else { continue };
            let id = rel.to_string_lossy().replace('/', "-");
            if by_id.contains_key(&id) {
                continue;
            }
            let app = std::fs::read_to_string(&path)
                .ok()
                .and_then(|text| parse_desktop_entry(&id, &text))
                .filter(|a| a.try_exec.as_deref().is_none_or(&exists));
            by_id.insert(id, app);
        }
    }
    let mut apps: Vec<App> = by_id.into_values().flatten().collect();
    apps.sort_by_key(|a| a.name.to_lowercase());
    apps
}

fn collect_desktop_files(dir: &Path, out: &mut Vec<PathBuf>, depth: u32) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for e in entries.flatten() {
        let path = e.path();
        if path.is_dir() && depth < 3 {
            collect_desktop_files(&path, out, depth + 1);
        } else if path.extension().is_some_and(|x| x == "desktop") {
            out.push(path);
        }
    }
}

/// Whether `program` (a name or a path) can be run: a path must exist, a name must be on `path`.
pub fn program_exists(program: &str, path: Option<&str>) -> bool {
    if program.contains('/') {
        return Path::new(program).is_file();
    }
    path.unwrap_or("").split(':').filter(|d| !d.is_empty()).any(|d| Path::new(d).join(program).is_file())
}

/// The app list as JSON for the launcher page: `[{"id":..,"name":..,"comment":..,"keywords":..}]`.
pub fn apps_json(apps: &[App]) -> String {
    let mut out = String::from("[");
    for (i, a) in apps.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        out.push_str(&format!(
            "{{\"id\":{},\"name\":{},\"comment\":{},\"keywords\":{}}}",
            json_string(&a.id),
            json_string(&a.name),
            json_string(&a.comment),
            json_string(&a.keywords)
        ));
    }
    out.push(']');
    out
}

/// A JSON string literal. `<`, `>`, `&` and U+2028/2029 are escaped too, so it is also safe to
/// splice into a script.
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
            '<' | '>' | '&' | '\u{2028}' | '\u{2029}' => out.push_str(&format!("\\u{:04x}", c as u32)),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// A message from a shell page (`window.webkit.messageHandlers.kompact.postMessage(text)`).
#[derive(Debug, PartialEq, Eq)]
pub enum Message {
    ToggleLauncher,
    CloseLauncher,
    Launch(String),
}

pub fn parse_message(text: &str) -> Option<Message> {
    match text.trim() {
        "toggle-launcher" => Some(Message::ToggleLauncher),
        "close-launcher" => Some(Message::CloseLauncher),
        t => t.strip_prefix("launch:").filter(|id| !id.is_empty()).map(|id| Message::Launch(id.to_string())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const GEDIT: &str = "[Desktop Entry]\nName=Text Editor\nName[de]=Texteditor\nComment=Edit text files\n\
        Keywords=Text;Editor;Plaintext;\nExec=gedit %U\nTerminal=false\nType=Application\n\
        [Desktop Action new-window]\nName=New Window\nExec=gedit --new-window\n";

    #[test]
    fn parses_an_application() {
        let a = parse_desktop_entry("org.gnome.gedit.desktop", GEDIT).unwrap();
        assert_eq!(a.name, "Text Editor");
        assert_eq!(a.comment, "Edit text files");
        assert_eq!(a.keywords, "Text Editor Plaintext");
        // The [Desktop Action] group's Exec doesn't override the main one.
        assert_eq!(a.exec, "gedit %U");
        assert_eq!(a.try_exec, None);
    }

    #[test]
    fn skips_what_shouldnt_be_listed() {
        let with = |extra: &str| format!("[Desktop Entry]\nName=X\nExec=x\nType=Application\n{extra}\n");
        assert!(parse_desktop_entry("x", &with("")).is_some());
        for extra in ["NoDisplay=true", "Hidden=true", "Terminal=true", "OnlyShowIn=GNOME;"] {
            assert_eq!(parse_desktop_entry("x", &with(extra)), None, "{extra}");
        }
        assert_eq!(parse_desktop_entry("x", "[Desktop Entry]\nName=X\nExec=x\nType=Link\n"), None);
        assert_eq!(parse_desktop_entry("x", "[Desktop Entry]\nName=X\nType=Application\n"), None);
        assert_eq!(parse_desktop_entry("x", "Name=X\nExec=x\nType=Application\n"), None);
    }

    #[test]
    fn exec_lines() {
        let v = |s: &[&str]| Some(s.iter().map(|x| x.to_string()).collect::<Vec<_>>());
        assert_eq!(exec_argv("gedit %U"), v(&["gedit"]));
        assert_eq!(exec_argv("firefox %u --new-window"), v(&["firefox", "--new-window"]));
        assert_eq!(exec_argv("/snap/bin/firefox %u"), v(&["/snap/bin/firefox"]));
        assert_eq!(exec_argv(r#""/opt/My App/app" --name "a \"b\"" 100%%"#), v(&["/opt/My App/app", "--name", "a \"b\"", "100%"]));
        assert_eq!(exec_argv("env FOO=1 app --file=%f"), v(&["env", "FOO=1", "app", "--file="]));
        assert_eq!(exec_argv("   "), None);
        assert_eq!(exec_argv("%U"), None);
        assert_eq!(exec_argv("\"unterminated"), None);
    }

    #[test]
    fn data_dir_order() {
        let env = |vars: &'static [(&'static str, &'static str)]| {
            move |k: &str| vars.iter().find(|(n, _)| *n == k).map(|(_, v)| v.to_string())
        };
        let d = data_dirs(env(&[("HOME", "/home/u")]));
        assert_eq!(
            d,
            ["/home/u/.local/share", "/usr/local/share", "/usr/share", "/var/lib/snapd/desktop", "/var/lib/flatpak/exports/share", "/home/u/.local/share/flatpak/exports/share"]
                .map(PathBuf::from)
        );
        let d = data_dirs(env(&[("HOME", "/home/u"), ("XDG_DATA_HOME", "/x"), ("XDG_DATA_DIRS", "/usr/share:/var/lib/snapd/desktop:/usr/share")]));
        assert_eq!(d[..3], ["/x", "/usr/share", "/var/lib/snapd/desktop"].map(PathBuf::from));
        assert_eq!(d.iter().filter(|p| p.as_path() == Path::new("/var/lib/snapd/desktop")).count(), 1);
    }

    #[test]
    fn scan_overrides_and_try_exec() {
        let base = std::env::temp_dir().join(format!("kompact-shell-apps-{}", std::process::id()));
        let (user, sys) = (base.join("user"), base.join("sys"));
        std::fs::create_dir_all(user.join("applications")).unwrap();
        std::fs::create_dir_all(sys.join("applications/kde")).unwrap();
        let entry = |name: &str, extra: &str| format!("[Desktop Entry]\nType=Application\nName={name}\nExec=true\n{extra}\n");
        std::fs::write(sys.join("applications/a.desktop"), entry("Zeta", "")).unwrap();
        std::fs::write(sys.join("applications/b.desktop"), entry("beta", "")).unwrap();
        std::fs::write(sys.join("applications/kde/c.desktop"), entry("Gamma", "")).unwrap();
        std::fs::write(sys.join("applications/d.desktop"), entry("Delta", "TryExec=not-installed")).unwrap();
        // The user's own b.desktop hides the system one.
        std::fs::write(user.join("applications/b.desktop"), entry("beta", "Hidden=true")).unwrap();
        let apps = scan(&[user, sys], |p| p != "not-installed");
        let names: Vec<_> = apps.iter().map(|a| (a.id.as_str(), a.name.as_str())).collect();
        assert_eq!(names, [("kde-c.desktop", "Gamma"), ("a.desktop", "Zeta")]);
        std::fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn json_output() {
        assert_eq!(json_string("a\"b\\c\n</script>"), "\"a\\\"b\\\\c\\n\\u003c/script\\u003e\"");
        let a = App { id: "x.desktop".into(), name: "X".into(), comment: "".into(), keywords: "k".into(), exec: "x".into(), try_exec: None };
        assert_eq!(apps_json(&[a.clone(), a]), r#"[{"id":"x.desktop","name":"X","comment":"","keywords":"k"},{"id":"x.desktop","name":"X","comment":"","keywords":"k"}]"#);
        assert_eq!(apps_json(&[]), "[]");
    }

    #[test]
    fn messages() {
        assert_eq!(parse_message("toggle-launcher"), Some(Message::ToggleLauncher));
        assert_eq!(parse_message(" close-launcher "), Some(Message::CloseLauncher));
        assert_eq!(parse_message("launch:firefox_firefox.desktop"), Some(Message::Launch("firefox_firefox.desktop".into())));
        assert_eq!(parse_message("launch:"), None);
        assert_eq!(parse_message("rm -rf"), None);
    }
}
