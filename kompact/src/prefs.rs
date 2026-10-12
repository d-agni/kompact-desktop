//! 11c: kompact's own settings, changed from System Settings → Desktop & Dock → Windows (`set
//! <key> <value>` on the shell channel) and kept in `~/.config/kompact/settings` (`key=value`
//! lines, written like the keys file). kompact applies each change at once and answers with a
//! `settings` message.
//!
//! - `animations=on|off` (11a/11b; `KOMPACT_ANIMATIONS`, if set, wins)
//! - `title-double-click=zoom|minimize|nothing` (kompact's own title bars)
//! - `focus-follows-mouse=on|off` (2d; `--focus-follows-mouse` turns it on for the session)
//! - 10d-3, Settings → Appearance: `appearance=light|dark|auto` (auto: light from 7:00 to
//!   19:00), `accent=<colour>` (`ACCENTS`), `buttons=left|right` (title-bar buttons). kompact
//!   draws its title bars to match, and the shell's pages take `dark` and `accent` from the
//!   `settings` message.
//! - 10d-4, Settings → Screenshots: `shots-dir=<folder>` (empty: `~/Pictures/Screenshots`;
//!   `KOMPACT_SCREENSHOT_DIR`, if set, wins), `shot-banner=on|off` (tell the shell, which shows
//!   "Screenshot saved"), `shot-pointer=on|off` (draw the pointer into the picture).
//! - 12f, Settings → Mouse & Keyboard: `pointer-speed=-1.0…1.0` (libinput's), `pointer-accel`
//!   (on: adaptive, off: flat), `natural-scroll` (touchpads; 13h: `mouse-natural-scroll` for
//!   mice, `scroll-speed=25…300` percent), `left-handed`, `tap-to-click`; `repeat-delay`
//!   (ms) and `repeat-rate` (per second, 0: no repeat); `layouts=us,fr(azerty)` (xkb layouts with
//!   an optional variant, the first is the default) and `caps=normal|escape|ctrl|off`.
//! - 12i, Settings → Lock Screen: `lock-after` and `screen-off-after` (idle minutes, 0: never).
//! - 12j, Settings → Default Apps: `terminal=<command>`, what Open a terminal runs (empty: the
//!   `--spawn` command kompact started with; split at spaces like it).

use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DoubleClick {
    /// Maximize, or restore a maximized window (macOS "Zoom").
    Zoom,
    Minimize,
    Nothing,
}

impl DoubleClick {
    pub fn name(self) -> &'static str {
        match self {
            DoubleClick::Zoom => "zoom",
            DoubleClick::Minimize => "minimize",
            DoubleClick::Nothing => "nothing",
        }
    }

    fn from_name(s: &str) -> Option<Self> {
        match s {
            "zoom" => Some(DoubleClick::Zoom),
            "minimize" => Some(DoubleClick::Minimize),
            "nothing" => Some(DoubleClick::Nothing),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Appearance {
    Light,
    Dark,
    /// Light by day (7:00–19:00 local time), dark at night.
    Auto,
}

impl Appearance {
    pub fn name(self) -> &'static str {
        match self {
            Appearance::Light => "light",
            Appearance::Dark => "dark",
            Appearance::Auto => "auto",
        }
    }

    fn from_name(s: &str) -> Option<Self> {
        match s {
            "light" => Some(Appearance::Light),
            "dark" => Some(Appearance::Dark),
            "auto" => Some(Appearance::Auto),
            _ => None,
        }
    }

    /// Light at this local hour (0–23)?
    pub fn is_light(self, hour: u32) -> bool {
        match self {
            Appearance::Light => true,
            Appearance::Dark => false,
            Appearance::Auto => (7..19).contains(&hour),
        }
    }
}

/// 12f: what Caps Lock does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Caps {
    Normal,
    /// Another Escape (vi users).
    Escape,
    /// Another Ctrl.
    Ctrl,
    Off,
}

impl Caps {
    pub fn name(self) -> &'static str {
        match self {
            Caps::Normal => "normal",
            Caps::Escape => "escape",
            Caps::Ctrl => "ctrl",
            Caps::Off => "off",
        }
    }

    fn from_name(s: &str) -> Option<Self> {
        match s {
            "normal" => Some(Caps::Normal),
            "escape" => Some(Caps::Escape),
            "ctrl" => Some(Caps::Ctrl),
            "off" => Some(Caps::Off),
            _ => None,
        }
    }

    /// The xkb option for it.
    pub fn xkb_option(self) -> Option<&'static str> {
        match self {
            Caps::Normal => None,
            Caps::Escape => Some("caps:escape"),
            Caps::Ctrl => Some("ctrl:nocaps"),
            Caps::Off => Some("caps:none"),
        }
    }
}

/// 12f: at most this many keyboard layouts.
pub const MAX_LAYOUTS: usize = 4;

/// `us`, `fr(azerty)`: an xkb layout and, in brackets, a variant.
pub fn valid_layout(item: &str) -> bool {
    let (name, variant) = match item.split_once('(') {
        Some((n, v)) => match v.strip_suffix(')') {
            Some(v) => (n, Some(v)),
            None => return false,
        },
        None => (item, None),
    };
    let word = |w: &str, extra: &[u8]| !w.is_empty() && w.len() <= 32 && w.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || extra.contains(&b));
    word(name, b"_") && variant.is_none_or(|v| word(v, b"_-"))
}

/// macOS's accents (the shell has their values).
pub const ACCENTS: [&str; 8] = ["blue", "purple", "pink", "red", "orange", "yellow", "green", "graphite"];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Prefs {
    pub animations: bool,
    pub double_click: DoubleClick,
    pub focus_follows_mouse: bool,
    pub appearance: Appearance,
    pub accent: &'static str,
    pub buttons_right: bool,
    /// 10d-4: where screenshots go (`None`: `~/Pictures/Screenshots`).
    pub shots_dir: Option<std::path::PathBuf>,
    pub shot_banner: bool,
    pub shot_pointer: bool,
    /// 12f: libinput's pointer speed, in tenths (-10…10).
    pub pointer_speed: i32,
    pub pointer_accel: bool,
    /// Touchpads (13h: the mouse wheel has its own, `mouse_natural_scroll`).
    pub natural_scroll: bool,
    /// 13h: a mouse wheel's direction; off by default (the wheel turns as the page moves).
    pub mouse_natural_scroll: bool,
    /// 13h: scrolling speed in percent (`scroll::MIN_SPEED`…`MAX_SPEED`).
    pub scroll_speed: u32,
    pub left_handed: bool,
    pub tap_to_click: bool,
    pub repeat_delay: u32,
    pub repeat_rate: u32,
    /// Comma-separated `layout(variant)` items.
    pub layouts: String,
    pub caps: Caps,
    /// 12i-1: lock the screen after this many idle minutes (0: never; `idle::LOCK_AFTER_CHOICES`).
    pub lock_after: u32,
    /// 12i-3: turn the screens off after this many idle minutes (0: never; same choices).
    pub screen_off_after: u32,
    /// 12j: Open a terminal's command (`None`: the `--spawn` one).
    pub terminal: Option<String>,
}

impl Default for Prefs {
    fn default() -> Self {
        Prefs {
            animations: true,
            double_click: DoubleClick::Zoom,
            focus_follows_mouse: false,
            appearance: Appearance::Dark,
            accent: "blue",
            buttons_right: false,
            shots_dir: None,
            shot_banner: true,
            shot_pointer: false,
            pointer_speed: 0,
            pointer_accel: true,
            natural_scroll: false,
            mouse_natural_scroll: false,
            scroll_speed: crate::scroll::DEFAULT_SPEED,
            left_handed: false,
            tap_to_click: true,
            // What kompact always had.
            repeat_delay: 200,
            repeat_rate: 25,
            layouts: "us".into(),
            caps: Caps::Normal,
            // As before: no idle lock.
            lock_after: 0,
            screen_off_after: 0,
            terminal: None,
        }
    }
}

impl Prefs {
    /// 12f: the xkb layout, variant and options strings for `layouts` and `caps`.
    pub fn xkb_parts(&self) -> (String, String, Option<String>) {
        let mut layouts = Vec::new();
        let mut variants = Vec::new();
        for item in self.layouts.split(',').filter(|i| !i.is_empty()) {
            match item.split_once('(') {
                Some((l, v)) => {
                    layouts.push(l.to_string());
                    variants.push(v.trim_end_matches(')').to_string());
                }
                None => {
                    layouts.push(item.to_string());
                    variants.push(String::new());
                }
            }
        }
        let variant = if variants.iter().all(String::is_empty) { String::new() } else { variants.join(",") };
        (layouts.join(","), variant, self.caps.xkb_option().map(str::to_string))
    }

    pub fn layout_count(&self) -> usize {
        self.layouts.split(',').filter(|i| !i.is_empty()).count()
    }
}

/// Longest folder path `set_path` takes.
pub const MAX_PATH: usize = 1000;
/// 12j: longest terminal command.
pub const MAX_COMMAND: usize = 300;

/// The local hour now (0–23), for `Appearance::Auto`.
pub fn local_hour() -> u32 {
    // SAFETY: `time(NULL)` and `localtime_r` with a valid out-pointer, no shared state.
    unsafe {
        let now = libc::time(std::ptr::null_mut());
        let mut tm: libc::tm = std::mem::zeroed();
        if libc::localtime_r(&now, &mut tm).is_null() {
            return 12;
        }
        tm.tm_hour.clamp(0, 23) as u32
    }
}

fn on_off(v: &str) -> Option<bool> {
    match v {
        "on" | "true" | "yes" => Some(true),
        "off" | "false" | "no" => Some(false),
        _ => None,
    }
}

fn word(b: bool) -> &'static str {
    if b { "on" } else { "off" }
}

impl Prefs {
    /// Change one setting (`animations`, `title-double-click`, `focus-follows-mouse`).
    pub fn set(&mut self, key: &str, value: &str) -> Result<(), String> {
        let bad = || format!("{value:?} isn't a value for {key}");
        match key {
            "animations" => self.animations = on_off(value).ok_or_else(bad)?,
            "focus-follows-mouse" => self.focus_follows_mouse = on_off(value).ok_or_else(bad)?,
            "title-double-click" => self.double_click = DoubleClick::from_name(value).ok_or_else(bad)?,
            "appearance" => self.appearance = Appearance::from_name(value).ok_or_else(bad)?,
            "accent" => self.accent = ACCENTS.iter().find(|a| **a == value).copied().ok_or_else(bad)?,
            "shot-banner" => self.shot_banner = on_off(value).ok_or_else(bad)?,
            "pointer-speed" => {
                let v: f64 = value.parse().map_err(|_| bad())?;
                if !(-1.0..=1.0).contains(&v) {
                    return Err(bad());
                }
                self.pointer_speed = (v * 10.0).round() as i32;
            }
            "pointer-accel" => self.pointer_accel = on_off(value).ok_or_else(bad)?,
            "natural-scroll" => self.natural_scroll = on_off(value).ok_or_else(bad)?,
            "mouse-natural-scroll" => self.mouse_natural_scroll = on_off(value).ok_or_else(bad)?,
            "scroll-speed" => {
                self.scroll_speed = value
                    .parse()
                    .ok()
                    .filter(|s| (crate::scroll::MIN_SPEED..=crate::scroll::MAX_SPEED).contains(s))
                    .ok_or_else(bad)?
            }
            "left-handed" => self.left_handed = on_off(value).ok_or_else(bad)?,
            "tap-to-click" => self.tap_to_click = on_off(value).ok_or_else(bad)?,
            "repeat-delay" => self.repeat_delay = value.parse().ok().filter(|d| (100..=2000).contains(d)).ok_or_else(bad)?,
            "repeat-rate" => self.repeat_rate = value.parse().ok().filter(|r| *r <= 100).ok_or_else(bad)?,
            "layouts" => {
                let items: Vec<&str> = value.split(',').collect();
                if items.is_empty() || items.len() > MAX_LAYOUTS || !items.iter().all(|i| valid_layout(i)) {
                    return Err(bad());
                }
                self.layouts = value.to_string();
            }
            "caps" => self.caps = Caps::from_name(value).ok_or_else(bad)?,
            "screen-off-after" => {
                self.screen_off_after = value.parse().ok().filter(|m| crate::idle::LOCK_AFTER_CHOICES.contains(m)).ok_or_else(bad)?
            }
            "lock-after" => {
                self.lock_after = value.parse().ok().filter(|m| crate::idle::LOCK_AFTER_CHOICES.contains(m)).ok_or_else(bad)?
            }
            "shot-pointer" => self.shot_pointer = on_off(value).ok_or_else(bad)?,
            "buttons" => {
                self.buttons_right = match value {
                    "left" => false,
                    "right" => true,
                    _ => return Err(bad()),
                }
            }
            _ => return Err(format!("no setting {key:?}")),
        }
        Ok(())
    }

    /// 10d-4: a setting that's a path (`shots-dir`: an absolute folder; empty for the default).
    /// 12j: or a command with spaces (`terminal`; empty for the default).
    pub fn set_path(&mut self, key: &str, value: &str) -> Result<(), String> {
        if key == "terminal" {
            let value = value.trim();
            if value.len() > MAX_COMMAND || value.contains(|c: char| c.is_control()) {
                return Err(format!("{value:?} isn't a command kompact can run"));
            }
            self.terminal = (!value.is_empty()).then(|| value.split_whitespace().collect::<Vec<_>>().join(" "));
            return Ok(());
        }
        if key != "shots-dir" {
            return Err(format!("no folder setting {key:?}"));
        }
        let value = value.trim();
        if value.is_empty() {
            self.shots_dir = None;
            return Ok(());
        }
        let path = std::path::Path::new(value);
        if !path.is_absolute() || value.len() > MAX_PATH || value.contains(['\n', '\r']) {
            return Err(format!("{value:?} isn't a full folder path"));
        }
        self.shots_dir = Some(path.to_path_buf());
        Ok(())
    }

    /// The file's lines; unknown keys and bad values keep their defaults.
    pub fn parse(text: &str) -> Prefs {
        let mut p = Prefs::default();
        for line in text.lines().map(str::trim).filter(|l| !l.starts_with('#')) {
            if let Some((k, v)) = line.split_once('=') {
                let k = k.trim();
                let _ = if matches!(k, "shots-dir" | "terminal") { p.set_path(k, v) } else { p.set(k, v.trim()) };
            }
        }
        p
    }

    pub fn to_text(&self) -> String {
        format!(
            "# kompact settings (written by System Settings → Desktop & Dock)\n\
             animations={}\ntitle-double-click={}\nfocus-follows-mouse={}\nappearance={}\naccent={}\nbuttons={}\n\
             shots-dir={}\nshot-banner={}\nshot-pointer={}\n\
             pointer-speed={:.1}\npointer-accel={}\nnatural-scroll={}\nmouse-natural-scroll={}\nscroll-speed={}\nleft-handed={}\ntap-to-click={}\n\
             repeat-delay={}\nrepeat-rate={}\nlayouts={}\ncaps={}\nlock-after={}\nscreen-off-after={}\nterminal={}\n",
            word(self.animations),
            self.double_click.name(),
            word(self.focus_follows_mouse),
            self.appearance.name(),
            self.accent,
            if self.buttons_right { "right" } else { "left" },
            self.shots_dir.as_ref().map(|p| p.display().to_string()).unwrap_or_default(),
            word(self.shot_banner),
            word(self.shot_pointer),
            self.pointer_speed as f64 / 10.0,
            word(self.pointer_accel),
            word(self.natural_scroll),
            word(self.mouse_natural_scroll),
            self.scroll_speed,
            word(self.left_handed),
            word(self.tap_to_click),
            self.repeat_delay,
            self.repeat_rate,
            self.layouts,
            self.caps.name(),
            self.lock_after,
            self.screen_off_after,
            self.terminal.as_deref().unwrap_or("")
        )
    }

    /// `{"type":"settings",…}` for the shell. `locked`: settings an environment variable holds
    /// for this session (the page shows them greyed out); `message`: why the last change failed.
    /// `dark`: what `appearance` comes to now.
    /// `shots`: the folder screenshots go to now (the setting, the variable or the default).
    /// `terminal_auto`: the `--spawn` command, used while `terminal` is empty.
    pub fn to_json(&self, locked: &[&str], message: Option<&str>, dark: bool, shots: &str, terminal_auto: &str) -> String {
        let locked: Vec<String> = locked.iter().map(|k| format!("\"{k}\"")).collect();
        let message = message.map(|m| format!(",\"message\":{}", crate::ipc::json_string(m))).unwrap_or_default();
        format!(
            "{{\"type\":\"settings\",\"version\":\"{}\",\"animations\":{},\"title_double_click\":\"{}\",\"focus_follows_mouse\":{},             \"appearance\":\"{}\",\"dark\":{dark},\"accent\":\"{}\",\"buttons\":\"{}\",\
             \"shots_dir\":{},\"shots_dir_default\":{},\"shot_banner\":{},\"shot_pointer\":{},\
             \"pointer_speed\":{:.1},\"pointer_accel\":{},\"natural_scroll\":{},\"mouse_natural_scroll\":{},\"scroll_speed\":{},\"left_handed\":{},\"tap_to_click\":{},\
             \"repeat_delay\":{},\"repeat_rate\":{},\"layouts\":{},\"caps\":\"{}\",\"lock_after\":{},\"screen_off_after\":{},\"terminal\":{},\"terminal_auto\":{},\"locked\":[{}]{message}}}\n",
            // 15a: the compositor's version, for Settings → About.
            env!("CARGO_PKG_VERSION"),
            self.animations,
            self.double_click.name(),
            self.focus_follows_mouse,
            self.appearance.name(),
            self.accent,
            if self.buttons_right { "right" } else { "left" },
            crate::ipc::json_string(shots),
            self.shots_dir.is_none(),
            self.shot_banner,
            self.shot_pointer,
            self.pointer_speed as f64 / 10.0,
            self.pointer_accel,
            self.natural_scroll,
            self.mouse_natural_scroll,
            self.scroll_speed,
            self.left_handed,
            self.tap_to_click,
            self.repeat_delay,
            self.repeat_rate,
            crate::ipc::json_string(&self.layouts),
            self.caps.name(),
            self.lock_after,
            self.screen_off_after,
            crate::ipc::json_string(self.terminal.as_deref().unwrap_or("")),
            crate::ipc::json_string(terminal_auto),
            locked.join(",")
        )
    }
}

pub fn path(config_home: Option<&Path>, home: Option<&Path>) -> Option<PathBuf> {
    config_home
        .map(Path::to_path_buf)
        .or_else(|| home.map(|h| h.join(".config")))
        .map(|c| c.join("kompact").join("settings"))
}

fn env_path() -> Option<PathBuf> {
    let config = std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from);
    let home = std::env::var_os("HOME").map(PathBuf::from);
    path(config.as_deref(), home.as_deref())
}

/// The saved settings, or the defaults.
pub fn load() -> Prefs {
    let Some(path) = env_path() else { return Prefs::default() };
    match std::fs::read_to_string(&path) {
        Ok(text) => {
            let p = Prefs::parse(&text);
            tracing::info!("settings: {} from {}", p.to_text().lines().skip(1).collect::<Vec<_>>().join(", "), path.display());
            p
        }
        Err(_) => Prefs::default(),
    }
}

/// Save to `~/.config/kompact/settings`; returns where.
pub fn save(p: &Prefs) -> Result<PathBuf, String> {
    let path = env_path().ok_or("no home directory")?;
    crate::keymap::save_to(&path, &p.to_text())?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        assert_eq!(Prefs::parse(""), Prefs::default());
        for animations in [true, false] {
            for double_click in [DoubleClick::Zoom, DoubleClick::Minimize, DoubleClick::Nothing] {
                for appearance in [Appearance::Light, Appearance::Dark, Appearance::Auto] {
                    let p = Prefs {
                        animations,
                        double_click,
                        focus_follows_mouse: !animations,
                        appearance,
                        accent: "green",
                        buttons_right: animations,
                        shots_dir: animations.then(|| "/home/u/My Shots".into()),
                        shot_banner: !animations,
                        shot_pointer: animations,
                        pointer_speed: if animations { -3 } else { 10 },
                        pointer_accel: !animations,
                        natural_scroll: animations,
                        mouse_natural_scroll: !animations,
                        scroll_speed: if animations { 25 } else { 300 },
                        left_handed: animations,
                        tap_to_click: !animations,
                        repeat_delay: 400,
                        repeat_rate: 30,
                        layouts: "us,fr(azerty)".into(),
                        caps: if animations { Caps::Escape } else { Caps::Ctrl },
                        lock_after: if animations { 5 } else { 0 },
                        screen_off_after: if animations { 0 } else { 2 },
                        terminal: animations.then(|| "ptyxis --new-window".into()),
                    };
                    assert_eq!(Prefs::parse(&p.to_text()), p.clone());
                }
            }
        }
        // Bad lines keep the defaults.
        let p = Prefs::parse("# x\nanimations = off\ntitle-double-click=shrug\nbogus=1\nfocus-follows-mouse");
        assert_eq!(p, Prefs { animations: false, ..Prefs::default() });
    }

    #[test]
    fn setting_one() {
        let mut p = Prefs::default();
        // 12i-1: only the choices Settings offers.
        assert!(p.set("lock-after", "10").is_ok() && p.lock_after == 10);
        assert!(p.set("lock-after", "7").is_err() && p.lock_after == 10);
        assert!(p.set("lock-after", "0").is_ok() && p.lock_after == 0);
        assert!(p.set("title-double-click", "minimize").is_ok());
        assert_eq!(p.double_click, DoubleClick::Minimize);
        assert!(p.set("focus-follows-mouse", "on").is_ok());
        assert!(p.focus_follows_mouse);
        assert!(p.set("animations", "maybe").is_err());
        assert!(p.set("wallpaper", "on").is_err());
        assert!(p.set("appearance", "auto").is_ok() && p.appearance == Appearance::Auto);
        assert!(p.set("accent", "pink").is_ok() && p.accent == "pink");
        assert!(p.set("accent", "mauve").is_err());
        assert!(p.set("buttons", "right").is_ok() && p.buttons_right);
        assert!(p.set("buttons", "up").is_err());
        assert!(p.set("shot-pointer", "on").is_ok() && p.shot_pointer);
        assert!(p.set_path("shots-dir", "/home/u/Desk top").is_ok());
        assert_eq!(p.shots_dir.as_deref(), Some(std::path::Path::new("/home/u/Desk top")));
        assert!(p.set_path("shots-dir", "relative/dir").is_err());
        assert!(p.set_path("shots-dir", "/a\nb").is_err());
        assert!(p.set_path("wallpaper", "/x").is_err());
        assert!(p.set_path("shots-dir", "  ").is_ok() && p.shots_dir.is_none());
        // 12j: a command, spaces squeezed; empty for kompact's own.
        assert!(p.set_path("terminal", " ptyxis   --new-window ").is_ok());
        assert_eq!(p.terminal.as_deref(), Some("ptyxis --new-window"));
        assert!(p.set_path("terminal", "foot\nquit").is_err() && p.terminal.is_some());
        assert!(p.set_path("terminal", "").is_ok() && p.terminal.is_none());
        assert!(p.animations, "a refused change changes nothing");
    }

    #[test]
    fn json() {
        let p = Prefs::default();
        assert_eq!(
            p.to_json(&["animations"], None, true, "/h/Pictures/Screenshots", "foot"),
            concat!("{\"type\":\"settings\",\"version\":\"", env!("CARGO_PKG_VERSION"), "\",\"animations\":true,\"title_double_click\":\"zoom\",\"focus_follows_mouse\":false,             \"appearance\":\"dark\",\"dark\":true,\"accent\":\"blue\",\"buttons\":\"left\",\
             \"shots_dir\":\"/h/Pictures/Screenshots\",\"shots_dir_default\":true,\"shot_banner\":true,\"shot_pointer\":false,\
             \"pointer_speed\":0.0,\"pointer_accel\":true,\"natural_scroll\":false,\"mouse_natural_scroll\":false,\"scroll_speed\":100,\"left_handed\":false,\"tap_to_click\":true,\
             \"repeat_delay\":200,\"repeat_rate\":25,\"layouts\":\"us\",\"caps\":\"normal\",\"lock_after\":0,\"screen_off_after\":0,\
             \"terminal\":\"\",\"terminal_auto\":\"foot\",\"locked\":[\"animations\"]}\n")
        );
        assert!(p.to_json(&[], Some("no \"x\""), true, "", "").ends_with(",\"message\":\"no \\\"x\\\"\"}\n"));
    }

    #[test]
    fn mouse_and_keyboard() {
        let mut p = Prefs::default();
        assert!(p.set("pointer-speed", "-0.35").is_ok());
        assert_eq!(p.pointer_speed, -4, "tenths, rounded");
        assert!(p.set("pointer-speed", "1.5").is_err());
        assert!(p.set("repeat-delay", "50").is_err());
        assert!(p.set("repeat-rate", "0").is_ok(), "0: no repeat");
        assert!(p.set("layouts", "us,de(nodeadkeys),in(eng)").is_ok());
        assert_eq!(p.layout_count(), 3);
        assert_eq!(p.xkb_parts(), ("us,de,in".into(), ",nodeadkeys,eng".into(), None));
        assert!(p.set("caps", "escape").is_ok());
        assert_eq!(p.xkb_parts().2.as_deref(), Some("caps:escape"));
        assert!(p.set("layouts", "us,de(").is_err());
        assert!(p.set("layouts", "US").is_err());
        assert!(p.set("layouts", "us,de,fr,es,it").is_err(), "at most four");
        assert!(p.set("layouts", "").is_err());
        let one = Prefs::default();
        assert_eq!(one.xkb_parts(), ("us".into(), String::new(), None), "no variants: an empty variant string");
        assert!(valid_layout("gb(extd)") && valid_layout("latam") && !valid_layout("us;rm"));
    }

    #[test]
    fn auto_appearance() {
        assert!(Appearance::Light.is_light(23));
        assert!(!Appearance::Dark.is_light(12));
        assert!(!Appearance::Auto.is_light(6));
        assert!(Appearance::Auto.is_light(7));
        assert!(Appearance::Auto.is_light(18));
        assert!(!Appearance::Auto.is_light(19));
        assert!(local_hour() < 24);
    }

    #[test]
    fn where_it_lives() {
        assert_eq!(path(Some(Path::new("/c")), Some(Path::new("/h"))), Some(PathBuf::from("/c/kompact/settings")));
        assert_eq!(path(None, Some(Path::new("/h"))), Some(PathBuf::from("/h/.config/kompact/settings")));
        assert_eq!(path(None, None), None);
    }
}
