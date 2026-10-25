//! 19a: what Siva can do: every tool the assistant may propose, with its arguments, how safe it
//! is (`Tier`) and whether it can be undone. Only what is listed here exists for the assistant;
//! Tier 3 (passwords, the screen's contents, display modes, installing) is left out on purpose,
//! so no proposal can reach it. 21a (decided 2026-10-25): files and commands are in (`WORK`),
//! with their own asking first in `work.rs` (sudo, deleting, outside your home folder…).
//!
//! Pure: the tools are data; running them (through the shell's existing commands) comes with the
//! assistant's page (19c). `schema` prints the list as JSON Schema for a local model (19f).

use serde_json::{json, Map, Value};

/// How much a tool can change (ROADMAP Phase 19, "Responsibility").
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Tier {
    /// Looks, changes nothing.
    Read,
    /// Small, local, and undone with one click (or harmless, like locking the screen).
    Reversible,
    /// Hard to undo, can lose work, or ends the session: asks every time.
    Consequential,
}

/// One argument's type and its limits.
#[derive(Debug, Clone, Copy)]
pub enum Arg {
    /// A whole number in `min..=max`.
    Int { min: i64, max: i64 },
    Bool,
    /// One of these words.
    OneOf(&'static [&'static str]),
    /// Free text (a window's or an app's name), at most this many characters, no control
    /// characters.
    Text { max: usize },
    /// 21a: text with line ends and tabs (a file's contents, a command), at most this many
    /// characters.
    Body { max: usize },
}

#[derive(Debug, Clone, Copy)]
pub struct Param {
    pub name: &'static str,
    pub arg: Arg,
    pub required: bool,
    pub about: &'static str,
}

#[derive(Debug, Clone, Copy)]
pub struct Tool {
    pub name: &'static str,
    pub about: &'static str,
    pub tier: Tier,
    /// Whether the activity line offers Undo (the old value is kept before it runs).
    pub undo: bool,
    pub params: &'static [Param],
}

const fn p(name: &'static str, arg: Arg, about: &'static str) -> Param {
    Param { name, arg, required: true, about }
}

const fn opt(name: &'static str, arg: Arg, about: &'static str) -> Param {
    Param { name, arg, required: false, about }
}

const WINDOW: Arg = Arg::Text { max: 120 };
/// 20b: what `show_part` opens.
pub const PARTS: &[&str] = &["menu", "spotlight", "launchpad", "control-center", "notifications", "overview", "switcher"];
/// 20b: Settings' panes (`web/settings.html`'s `data-pane`).
pub const PANES: &[&str] = &[
    "network", "bluetooth", "notifications", "sound", "about", "appearance", "control", "assistant", "datetime", "defaults", "dock", "displays", "wallpaper",
    "screenshots", "sharing", "lock", "users", "input", "keys",
];
pub const FOLDERS: &[&str] = &["home", "downloads", "documents", "pictures", "music", "videos", "desktop", "screenshots", "trash"];
pub const SITES: &[&str] = &["web", "youtube", "wikipedia", "maps", "images", "news", "github"];
/// kompact's on/off settings (`prefs.rs`).
pub const DESKTOP_SETTINGS: &[&str] = &["animations", "focus-follows-mouse", "tap-to-click", "left-handed"];
const ON: Arg = Arg::Bool;

pub const TOOLS: &[Tool] = &[
    // ---- Tier 0: read ----
    Tool { name: "list_windows", about: "The open windows: app, title, desktop, minimized, focused.", tier: Tier::Read, undo: false, params: &[opt("app", WINDOW, "only this app's windows")] },
    Tool { name: "sound_status", about: "Output and input devices, the volume, muted or not.", tier: Tier::Read, undo: false, params: &[] },
    Tool { name: "network_status", about: "Wi-Fi and wired connections.", tier: Tier::Read, undo: false, params: &[] },
    Tool { name: "bluetooth_status", about: "Bluetooth on or off, and the devices.", tier: Tier::Read, undo: false, params: &[] },
    // 19g: why things are as they are.
    Tool { name: "system_status", about: "How busy the computer is: CPU, memory and the busiest apps (why it's slow).", tier: Tier::Read, undo: false, params: &[] },
    Tool { name: "date_time", about: "The time and the date now.", tier: Tier::Read, undo: false, params: &[] },
    Tool { name: "disk_space", about: "How much space is left on the disks.", tier: Tier::Read, undo: false, params: &[] },
    Tool { name: "sound_check", about: "Why the sound may be bad or cutting out: muted, the output, a Bluetooth headset in phone-quality mode.", tier: Tier::Read, undo: false, params: &[] },
    Tool { name: "now_playing", about: "What the media players are playing.", tier: Tier::Read, undo: false, params: &[] },
    // 20d: online (web.rs), as Settings → Assistant allows.
    Tool { name: "weather", about: "The weather now, tomorrow or this week, in your town or a place.", tier: Tier::Read, undo: false, params: &[opt("place", Arg::Text { max: 80 }, "a town; yours if left out"), opt("day", Arg::OneOf(&["today", "tomorrow", "week"]), "today if left out")] },
    Tool { name: "news", about: "The latest news headlines, or the news on a topic.", tier: Tier::Read, undo: false, params: &[opt("topic", Arg::Text { max: 80 }, "e.g. cricket; the top stories if left out")] },
    Tool { name: "look_up", about: "Look something up online: a fact about a person, place or thing, or search results.", tier: Tier::Read, undo: false, params: &[p("query", Arg::Text { max: 200 }, "what to look up"), opt("want", Arg::OneOf(&["answer", "results"]), "answer if left out")] },
    // ---- Tier 1: reversible ----
    Tool { name: "set_volume", about: "Set the output volume.", tier: Tier::Reversible, undo: true, params: &[p("percent", Arg::Int { min: 0, max: 100 }, "0 to 100")] },
    Tool { name: "change_volume", about: "Turn the output volume up or down by some points.", tier: Tier::Reversible, undo: true, params: &[p("by", Arg::Int { min: -100, max: 100 }, "points, negative is quieter")] },
    Tool { name: "mute", about: "Mute or unmute the output or the microphone.", tier: Tier::Reversible, undo: true, params: &[p("on", ON, "true mutes"), opt("device", Arg::OneOf(&["output", "microphone"]), "output if left out")] },
    Tool { name: "sound_output", about: "Send the sound to another output device.", tier: Tier::Reversible, undo: true, params: &[p("device", Arg::Text { max: 120 }, "its name, as sound_status lists it")] },
    Tool { name: "media", about: "Play/pause, next or previous in the media player that's playing.", tier: Tier::Reversible, undo: false, params: &[p("action", Arg::OneOf(&["play-pause", "next", "previous"]), "")] },
    Tool { name: "open_app", about: "Start an app.", tier: Tier::Reversible, undo: false, params: &[p("app", Arg::Text { max: 80 }, "its name, e.g. Firefox")] },
    Tool { name: "focus_window", about: "Bring a window to the front (restores it if minimized).", tier: Tier::Reversible, undo: true, params: &[p("window", WINDOW, "its title or app")] },
    Tool { name: "minimize_window", about: "Minimize a window.", tier: Tier::Reversible, undo: true, params: &[p("window", WINDOW, "its title or app")] },
    Tool {
        name: "place_window",
        about: "Put a window on the left or right half, fill the screen, or restore its size.",
        tier: Tier::Reversible,
        undo: true,
        params: &[p("window", WINDOW, "its title or app"), p("where", Arg::OneOf(&["left", "right", "fill", "restore"]), "")],
    },
    Tool { name: "switch_desktop", about: "Show another desktop (workspace).", tier: Tier::Reversible, undo: true, params: &[p("number", Arg::Int { min: 1, max: 9 }, "1 to 9")] },
    Tool { name: "do_not_disturb", about: "Turn Do Not Disturb on or off.", tier: Tier::Reversible, undo: true, params: &[p("on", ON, "")] },
    Tool { name: "appearance", about: "Light, dark or automatic appearance.", tier: Tier::Reversible, undo: true, params: &[p("mode", Arg::OneOf(&["light", "dark", "auto"]), "")] },
    Tool { name: "keep_awake", about: "Keep the screen awake (no lock, no screens off) until turned off.", tier: Tier::Reversible, undo: true, params: &[p("on", ON, "")] },
    Tool { name: "screenshot", about: "Take a screenshot of the screen, or let the user drag out an area.", tier: Tier::Reversible, undo: false, params: &[p("of", Arg::OneOf(&["screen", "area"]), "")] },
    Tool { name: "lock_screen", about: "Lock the screen.", tier: Tier::Reversible, undo: false, params: &[] },
    // 20b: the rest of the desktop.
    Tool { name: "show_part", about: "Open a part of the desktop: the logo menu, Spotlight, Launchpad, Control Center, the notifications, all desktops, the window switcher.", tier: Tier::Reversible, undo: false, params: &[p("part", Arg::OneOf(PARTS), "")] },
    Tool { name: "open_settings", about: "Open System Settings, at a pane.", tier: Tier::Reversible, undo: false, params: &[opt("pane", Arg::OneOf(PANES), "the start page if left out")] },
    Tool { name: "open_folder", about: "Open a folder in Files.", tier: Tier::Reversible, undo: false, params: &[p("folder", Arg::OneOf(FOLDERS), "")] },
    Tool { name: "open_link", about: "Open a web address in the browser.", tier: Tier::Reversible, undo: false, params: &[p("url", Arg::Text { max: 300 }, "e.g. youtube.com")] },
    Tool { name: "search_web", about: "Search for something in the browser, on the web or a site.", tier: Tier::Reversible, undo: false, params: &[p("query", Arg::Text { max: 200 }, ""), opt("site", Arg::OneOf(SITES), "the web if left out")] },
    Tool { name: "move_to_desktop", about: "Move a window to another desktop (1 to 9), staying here.", tier: Tier::Reversible, undo: true, params: &[p("window", WINDOW, "its title or app"), p("number", Arg::Int { min: 1, max: 9 }, "")] },
    Tool { name: "show_desktop", about: "Move every window out of the way to show the desktop, or bring them back.", tier: Tier::Reversible, undo: true, params: &[] },
    Tool { name: "dock", about: "Change the dock: where it is, when it hides, how big it is.", tier: Tier::Reversible, undo: true, params: &[opt("position", Arg::OneOf(&["left", "bottom", "right"]), ""), opt("hide", Arg::OneOf(&["never", "smart", "always"]), "smart: when a window is maximized"), opt("size", Arg::OneOf(&["small", "medium", "large"]), "")] },
    Tool { name: "set_place", about: "Remember the user's town, for the weather.", tier: Tier::Reversible, undo: true, params: &[p("place", Arg::Text { max: 80 }, "a town")] },
    // 20e: in the app in front (or the window named), through kompact's keyboard. No Undo;
    // into a terminal, or keys that quit or delete for good, it asks first (`policy::typing_asks`).
    Tool { name: "type_text", about: "Type text into the app in front, or a window.", tier: Tier::Reversible, undo: false, params: &[p("text", Arg::Text { max: 500 }, "exactly what to type"), opt("window", WINDOW, "the app in front if left out")] },
    Tool { name: "write_text", about: "Write something new (a joke, a note, an email, a poem) and type it into the app in front, or a window.", tier: Tier::Reversible, undo: false, params: &[p("about", Arg::Text { max: 300 }, "what to write, e.g. a short joke"), opt("window", WINDOW, "the app in front if left out")] },
    Tool { name: "save_as", about: "Save the file in the app in front (or a window) under a name: Save As, the name, Enter.", tier: Tier::Reversible, undo: false, params: &[p("name", Arg::Text { max: 100 }, "the file name, e.g. notes.txt"), opt("window", WINDOW, "the app in front if left out")] },
    Tool { name: "press_keys", about: "Press keys in the app in front, or a window: \"ctrl+t\" a new tab, \"ctrl+s\" save, \"enter\".", tier: Tier::Reversible, undo: false, params: &[p("keys", Arg::Text { max: 60 }, "like ctrl+shift+t; several with commas"), opt("window", WINDOW, "the app in front if left out")] },
    Tool { name: "desktop_setting", about: "Turn a desktop setting on or off.", tier: Tier::Reversible, undo: true, params: &[p("setting", Arg::OneOf(DESKTOP_SETTINGS), ""), p("on", ON, "")] },
    // 21a: real work. Relative names are in your home folder; commands run in Siva's terminal.
    // Outside your home folder, hidden files and drastic commands ask first (`work.rs`).
    Tool { name: "read_file", about: "Read a text file.", tier: Tier::Read, undo: false, params: &[p("path", Arg::Text { max: 300 }, "e.g. notes.txt or ~/code/main.py")] },
    Tool { name: "list_files", about: "List the files in a folder.", tier: Tier::Read, undo: false, params: &[opt("folder", Arg::Text { max: 300 }, "your home folder if left out")] },
    Tool { name: "write_file", about: "Write a text file (made if new, replaced if there), e.g. a program.", tier: Tier::Reversible, undo: true, params: &[p("path", Arg::Text { max: 300 }, "e.g. hello.py"), p("content", Arg::Body { max: 20_000 }, "the whole file")] },
    Tool { name: "run_command", about: "Run a command in Siva's terminal, where the user sees it, and get its output.", tier: Tier::Reversible, undo: false, params: &[p("command", Arg::Body { max: 2_000 }, "e.g. python3 hello.py"), opt("folder", Arg::Text { max: 300 }, "your home folder if left out")] },
    // ---- Tier 2: consequential, asks every time ----
    Tool { name: "close_window", about: "Close a window. Its app may lose unsaved work.", tier: Tier::Consequential, undo: false, params: &[p("window", WINDOW, "its title or app")] },
    Tool { name: "wifi", about: "Turn Wi-Fi on or off.", tier: Tier::Reversible, undo: true, params: &[p("on", ON, "")] },
    Tool { name: "bluetooth", about: "Turn Bluetooth on or off (disconnects its devices).", tier: Tier::Reversible, undo: true, params: &[p("on", ON, "")] },
    Tool { name: "log_out", about: "Log out (closes every app).", tier: Tier::Consequential, undo: false, params: &[] },
    Tool { name: "power", about: "Sleep, restart or power off the computer.", tier: Tier::Consequential, undo: false, params: &[p("action", Arg::OneOf(&["sleep", "restart", "power-off"]), "")] },
];

/// 21a: the tools for real work, not given to the small model on this PC (it can't use them
/// safely; 21b/21c give them to a model that plans).
pub const WORK: [&str; 4] = ["read_file", "list_files", "write_file", "run_command"];

/// The tools the small model on this PC is told about.
pub fn for_small_model() -> impl Iterator<Item = &'static Tool> {
    TOOLS.iter().filter(|t| !WORK.contains(&t.name))
}

/// Names are unique (`names_are_unique_and_plain`), so a tool is its name.
impl PartialEq for Tool {
    fn eq(&self, other: &Self) -> bool {
        self.name == other.name
    }
}

pub fn find(name: &str) -> Option<&'static Tool> {
    TOOLS.iter().find(|t| t.name == name)
}

/// What's wrong with a proposal's arguments, said so a person understands it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ArgError {
    NotAnObject,
    Missing(&'static str),
    Unknown(String),
    Wrong { name: &'static str, expected: String },
}

impl std::fmt::Display for ArgError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ArgError::NotAnObject => write!(f, "the details aren't in the right form"),
            ArgError::Missing(n) => write!(f, "{n} is missing"),
            ArgError::Unknown(n) => write!(f, "{n} isn't something this takes"),
            ArgError::Wrong { name, expected } => write!(f, "{name} should be {expected}"),
        }
    }
}

fn check(arg: Arg, v: &Value) -> Result<(), String> {
    match arg {
        Arg::Int { min, max } => match v.as_i64() {
            Some(n) if (min..=max).contains(&n) => Ok(()),
            _ => Err(format!("a whole number from {min} to {max}")),
        },
        Arg::Bool => v.as_bool().map(|_| ()).ok_or_else(|| "true or false".into()),
        Arg::OneOf(words) => match v.as_str() {
            Some(s) if words.contains(&s) => Ok(()),
            _ => Err(format!("one of {}", words.join(", "))),
        },
        Arg::Text { max } => match v.as_str() {
            Some(s) if !s.trim().is_empty() && s.chars().count() <= max && !s.chars().any(char::is_control) => Ok(()),
            _ => Err(format!("a name of at most {max} characters")),
        },
        Arg::Body { max } => match v.as_str() {
            Some(s) if !s.trim().is_empty() && s.chars().count() <= max && !s.chars().any(|c| c.is_control() && !matches!(c, '\n' | '\t' | '\r')) => Ok(()),
            _ => Err(format!("text of at most {max} characters")),
        },
    }
}

impl Tool {
    /// Arguments exactly as declared: the required ones there, nothing extra, each in range.
    pub fn validate(&self, args: &Value) -> Result<(), ArgError> {
        let empty = Map::new();
        let obj = match args {
            Value::Object(m) => m,
            Value::Null => &empty,
            _ => return Err(ArgError::NotAnObject),
        };
        if let Some(extra) = obj.keys().find(|k| !self.params.iter().any(|p| p.name == k.as_str())) {
            return Err(ArgError::Unknown(extra.clone()));
        }
        for p in self.params {
            match obj.get(p.name) {
                None if p.required => return Err(ArgError::Missing(p.name)),
                None => {}
                Some(v) => check(p.arg, v).map_err(|expected| ArgError::Wrong { name: p.name, expected })?,
            }
        }
        Ok(())
    }
}

fn arg_schema(param: &Param) -> Value {
    let mut s = match param.arg {
        Arg::Int { min, max } => json!({"type": "integer", "minimum": min, "maximum": max}),
        Arg::Bool => json!({"type": "boolean"}),
        Arg::OneOf(words) => json!({"type": "string", "enum": words}),
        Arg::Text { max } | Arg::Body { max } => json!({"type": "string", "minLength": 1, "maxLength": max}),
    };
    if !param.about.is_empty() {
        s["description"] = json!(param.about);
    }
    s
}

/// The tools as JSON Schema (name, description, parameters), for a local model (19f).
pub fn schema() -> Value {
    Value::Array(
        TOOLS
            .iter()
            .map(|t| {
                let props: Map<String, Value> = t.params.iter().map(|p| (p.name.to_string(), arg_schema(p))).collect();
                let required: Vec<&str> = t.params.iter().filter(|p| p.required).map(|p| p.name).collect();
                json!({
                    "name": t.name,
                    "description": t.about,
                    "parameters": {"type": "object", "properties": props, "required": required, "additionalProperties": false},
                })
            })
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_unique_and_plain() {
        let mut names: Vec<&str> = TOOLS.iter().map(|t| t.name).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), TOOLS.len(), "two tools share a name");
        for t in TOOLS {
            assert!(t.name.bytes().all(|b| b.is_ascii_lowercase() || b == b'_'), "{}", t.name);
            assert!(!t.about.is_empty(), "{} has no description", t.name);
            // Reading can't be undone, because it changes nothing.
            assert!(!(t.tier == Tier::Read && t.undo), "{}", t.name);
        }
    }

    #[test]
    fn nothing_dangerous_is_listed() {
        // Tier 3 stays out: no tool for these, whatever it would be called. 21a: files and
        // commands came in (asking in `work.rs`), only those four.
        for word in ["password", "sudo", "shell", "delete", "install", "display", "polkit", "send", "email"] {
            assert!(TOOLS.iter().all(|t| !t.name.contains(word)), "a tool named after {word}");
        }
        for word in ["command", "file"] {
            assert!(TOOLS.iter().filter(|t| t.name.contains(word)).all(|t| WORK.contains(&t.name)), "{word}");
        }
        // The ones that lose work or end the session ask first.
        // 20a (2026-10-25): only the drastic ones; Wi-Fi and Bluetooth run at once, with Undo.
        for name in ["close_window", "log_out", "power"] {
            assert_eq!(find(name).unwrap().tier, Tier::Consequential, "{name}");
        }
    }

    #[test]
    fn arguments() {
        let vol = find("set_volume").unwrap();
        assert!(vol.validate(&json!({"percent": 40})).is_ok());
        assert_eq!(vol.validate(&json!({"percent": 140})), Err(ArgError::Wrong { name: "percent", expected: "a whole number from 0 to 100".into() }));
        assert_eq!(vol.validate(&json!({"percent": "40"})).unwrap_err().to_string(), "percent should be a whole number from 0 to 100");
        assert_eq!(vol.validate(&json!({})), Err(ArgError::Missing("percent")));
        assert_eq!(vol.validate(&json!({"percent": 40, "also": "rm -rf"})), Err(ArgError::Unknown("also".into())));
        assert_eq!(vol.validate(&json!([40])), Err(ArgError::NotAnObject));

        let place = find("place_window").unwrap();
        assert!(place.validate(&json!({"window": "Firefox", "where": "left"})).is_ok());
        assert!(place.validate(&json!({"window": "Firefox", "where": "upside-down"})).is_err());
        assert!(place.validate(&json!({"window": "   ", "where": "left"})).is_err());
        assert!(place.validate(&json!({"window": "a\nb", "where": "left"})).is_err());
        assert!(place.validate(&json!({"window": "x".repeat(121), "where": "left"})).is_err());

        // No parameters: nothing, `{}` or null.
        let lock = find("lock_screen").unwrap();
        assert!(lock.validate(&Value::Null).is_ok() && lock.validate(&json!({})).is_ok());
        // An optional one may be left out.
        assert!(find("mute").unwrap().validate(&json!({"on": true})).is_ok());
        // 21a: a file's contents have line ends; a name doesn't.
        let write = find("write_file").unwrap();
        assert!(write.validate(&json!({"path": "hello.py", "content": "def f():\n\treturn 1\n"})).is_ok());
        assert!(write.validate(&json!({"path": "a\nb", "content": "x"})).is_err());
        assert!(write.validate(&json!({"path": "a.txt", "content": "bell\u{7}"})).is_err());
        assert!(find("run_command").unwrap().validate(&json!({"command": "python3 hello.py"})).is_ok());
        assert!(for_small_model().all(|t| !WORK.contains(&t.name)));
        assert_eq!(for_small_model().count(), TOOLS.len() - WORK.len());
    }

    #[test]
    fn schema_matches_the_tools() {
        let s = schema();
        let list = s.as_array().unwrap();
        assert_eq!(list.len(), TOOLS.len());
        let place = list.iter().find(|t| t["name"] == "place_window").unwrap();
        assert_eq!(place["parameters"]["required"], json!(["window", "where"]));
        assert_eq!(place["parameters"]["properties"]["where"]["enum"], json!(["left", "right", "fill", "restore"]));
        assert_eq!(place["parameters"]["additionalProperties"], json!(false));
        let mute = list.iter().find(|t| t["name"] == "mute").unwrap();
        assert_eq!(mute["parameters"]["required"], json!(["on"]));
    }
}
