//! 13d: which status items the menu bar shows (System Settings → Control Center), as macOS:
//! each one Always ("Show in Menu Bar"), When Active, or Never ("Don't Show"); Control Center
//! has them all anyway. Kept in `~/.config/kompact/menu-bar` (`network=active` lines). What
//! "active" means for each item, and which fold away first when the bar is too narrow, is the
//! panel's (`web/menubar.js`, tested by `web/menubar.test.js` under gjs).

use std::path::PathBuf;

use serde_json::{json, Map, Value};
use tauri::{AppHandle, Emitter};
use tracing::info;

/// The items, in the order Settings lists them, with their default.
pub const ITEMS: [(&str, Mode); 6] = [
    ("network", Mode::Active),
    ("bluetooth", Mode::Active),
    ("sound", Mode::Active),
    ("input", Mode::Active),
    ("spaces", Mode::Active),
    ("spotlight", Mode::Always),
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Always,
    Active,
    Never,
}

impl Mode {
    pub fn parse(s: &str) -> Option<Mode> {
        match s {
            "always" => Some(Mode::Always),
            "active" => Some(Mode::Active),
            "never" => Some(Mode::Never),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Mode::Always => "always",
            Mode::Active => "active",
            Mode::Never => "never",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Prefs(Vec<(&'static str, Mode)>);

impl Default for Prefs {
    fn default() -> Self {
        Prefs(ITEMS.to_vec())
    }
}

impl Prefs {
    /// Unknown items and bad values keep their defaults.
    pub fn parse(text: &str) -> Prefs {
        let mut p = Prefs::default();
        for line in text.lines().map(str::trim).filter(|l| !l.starts_with('#')) {
            if let Some((k, v)) = line.split_once('=') {
                let _ = p.set(k.trim(), v.trim());
            }
        }
        p
    }

    pub fn set(&mut self, item: &str, mode: &str) -> Result<(), String> {
        let mode = Mode::parse(mode).ok_or_else(|| format!("{mode:?} isn't always, active or never"))?;
        let slot = self.0.iter_mut().find(|(k, _)| *k == item).ok_or_else(|| format!("no menu-bar item {item:?}"))?;
        slot.1 = mode;
        Ok(())
    }

    #[cfg(test)]
    pub fn get(&self, item: &str) -> Option<Mode> {
        self.0.iter().find(|(k, _)| *k == item).map(|(_, m)| *m)
    }

    pub fn to_text(&self) -> String {
        let mut s = String::from("# kompact menu bar (written by System Settings → Control Center): always, active or never\n");
        for (k, m) in &self.0 {
            s.push_str(&format!("{k}={}\n", m.name()));
        }
        s
    }

    pub fn to_json(&self) -> Value {
        let mut m = Map::new();
        for (k, mode) in &self.0 {
            m.insert((*k).into(), json!(mode.name()));
        }
        Value::Object(m)
    }
}

fn path() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
    Some(base.join("kompact").join("menu-bar"))
}

fn load() -> Prefs {
    path().and_then(|p| std::fs::read_to_string(p).ok()).map(|t| Prefs::parse(&t)).unwrap_or_default()
}

#[tauri::command]
pub fn menubar_prefs() -> Value {
    load().to_json()
}

/// Change one item; every menu bar (one per display, 12h-2) and Settings hear `kompact-menubar`.
#[tauri::command]
pub fn set_menubar_pref(app: AppHandle, item: String, mode: String) -> Result<(), String> {
    let mut p = load();
    p.set(&item, &mode)?;
    let path = path().ok_or("no home directory")?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    std::fs::write(&path, p.to_text()).map_err(|e| format!("{}: {e}", path.display()))?;
    info!("menu bar: {item} = {mode} (saved to {})", path.display());
    let _ = app.emit("kompact-menubar", p.to_json());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn menu_bar_prefs() {
        let d = Prefs::default();
        assert_eq!(d.get("network"), Some(Mode::Active));
        assert_eq!(d.get("spotlight"), Some(Mode::Always));
        assert_eq!(Prefs::parse(&d.to_text()), d);
        let mut p = Prefs::parse("# x\nsound=always\nbluetooth=never\nnetwork=maybe\nclock=never\n");
        assert_eq!(p.get("sound"), Some(Mode::Always));
        assert_eq!(p.get("bluetooth"), Some(Mode::Never));
        assert_eq!(p.get("network"), Some(Mode::Active), "a bad value keeps the default");
        assert_eq!(p.get("clock"), None, "the clock and Control Center always show");
        assert!(p.set("spaces", "never").is_ok());
        assert!(p.set("spaces", "sometimes").is_err());
        assert!(p.set("control", "never").is_err());
        assert_eq!(Prefs::parse(&p.to_text()), p);
        assert_eq!(p.to_json()["spaces"], "never");
    }
}
