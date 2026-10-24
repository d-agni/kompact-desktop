//! 19a: what Siva can do: every tool the assistant may propose, with its arguments, how safe it
//! is (`Tier`) and whether it can be undone. Only what is listed here exists for the assistant;
//! Tier 3 (passwords, `sudo`, shell commands, files, the screen's contents, display modes) is
//! left out on purpose, so no proposal can reach it.
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
const ON: Arg = Arg::Bool;

pub const TOOLS: &[Tool] = &[
    // ---- Tier 0: read ----
    Tool { name: "list_windows", about: "The open windows: app, title, desktop, minimized, focused.", tier: Tier::Read, undo: false, params: &[] },
    Tool { name: "sound_status", about: "Output and input devices, the volume, muted or not.", tier: Tier::Read, undo: false, params: &[] },
    Tool { name: "network_status", about: "Wi-Fi and wired connections.", tier: Tier::Read, undo: false, params: &[] },
    Tool { name: "bluetooth_status", about: "Bluetooth on or off, and the devices.", tier: Tier::Read, undo: false, params: &[] },
    // 19g: why things are as they are.
    Tool { name: "system_status", about: "How busy the computer is: CPU, memory and the busiest apps (why it's slow).", tier: Tier::Read, undo: false, params: &[] },
    Tool { name: "disk_space", about: "How much space is left on the disks.", tier: Tier::Read, undo: false, params: &[] },
    Tool { name: "sound_check", about: "Why the sound may be bad or cutting out: muted, the output, a Bluetooth headset in phone-quality mode.", tier: Tier::Read, undo: false, params: &[] },
    Tool { name: "now_playing", about: "What the media players are playing.", tier: Tier::Read, undo: false, params: &[] },
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
    // ---- Tier 2: consequential, asks every time ----
    Tool { name: "close_window", about: "Close a window. Its app may lose unsaved work.", tier: Tier::Consequential, undo: false, params: &[p("window", WINDOW, "its title or app")] },
    Tool { name: "wifi", about: "Turn Wi-Fi on or off.", tier: Tier::Consequential, undo: true, params: &[p("on", ON, "")] },
    Tool { name: "bluetooth", about: "Turn Bluetooth on or off (disconnects its devices).", tier: Tier::Consequential, undo: true, params: &[p("on", ON, "")] },
    Tool { name: "log_out", about: "Log out (closes every app).", tier: Tier::Consequential, undo: false, params: &[] },
    Tool { name: "power", about: "Sleep, restart or power off the computer.", tier: Tier::Consequential, undo: false, params: &[p("action", Arg::OneOf(&["sleep", "restart", "power-off"]), "")] },
];

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
        Arg::Text { max } => json!({"type": "string", "minLength": 1, "maxLength": max}),
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
        // Tier 3 stays out: no tool for these, whatever it would be called.
        for word in ["password", "sudo", "shell", "command", "file", "delete", "install", "display", "polkit", "send", "email"] {
            assert!(TOOLS.iter().all(|t| !t.name.contains(word)), "a tool named after {word}");
        }
        // The ones that lose work or end the session ask first.
        for name in ["close_window", "log_out", "power", "wifi", "bluetooth"] {
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
