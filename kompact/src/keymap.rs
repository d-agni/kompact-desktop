//! 10a: every kompact shortcut as data, with presets the user can change.
//!
//! An `Action` is something kompact does on a key (close the window, show workspace 3, take a
//! screenshot…). A `Keymap` gives each action up to two key combinations (`Combo`). It starts
//! from a preset: `macos` (the default; Super plays the Command key), `windows` (Windows 11) or
//! `classic` (the keys kompact had before Phase 10). The user's own changes go on top, in
//! `~/.config/kompact/keys`:
//!
//! ```text
//! preset = macos
//! window.close = Super+W
//! screenshot.region = Super+Shift+4, Shift+Print
//! desktop.show = none
//! ```
//!
//! Only the changes from the preset are in the file; bad lines are reported and skipped.
//! `KOMPACT_KEYS=classic|macos|windows` picks a preset and ignores the file (for testing).
//!
//! Keys are matched by the key's own symbol without Shift or the layout's other levels applied
//! (Super+Shift+3 is "3", not "#") and with exactly the modifiers given. The workspace actions
//! use the physical 1–9 keys (`1-9`); only their modifiers can change. A lone tap of Super
//! (pressed and released with nothing else) can be bound too (`Super`). Ctrl+Alt+F1–F12 always
//! switch consoles and can't be bound; Quit and Lock can't be left without a key.

use std::collections::HashMap;
use std::fmt;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use smithay::input::keyboard::xkb;

/// Everything a shortcut can do.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Action {
    Spotlight,
    SwitchWindows,
    CloseWindow,
    Minimize,
    RestoreMinimized,
    ToggleFill,
    SnapLeft,
    SnapRight,
    SnapUp,
    SnapDown,
    ShowWorkspace,
    SendToWorkspace,
    PrevWorkspace,
    NextWorkspace,
    Overview,
    ShowDesktop,
    Terminal,
    Files,
    Lock,
    Quit,
    ShotAll,
    ShotPointerOutput,
    ShotRegion,
    FocusFollowsMouse,
    DebugOverlay,
    /// 12c: sound (the shell runs them: `wpctl`, with an on-screen level).
    VolumeUp,
    VolumeDown,
    VolumeMute,
    MicMute,
    /// 12f: the next keyboard layout (does nothing, and the key goes to the app, with one).
    NextLayout,
}

/// Every action, in the order the settings app lists them.
pub const ACTIONS: &[Action] = &[
    Action::SwitchWindows,
    Action::CloseWindow,
    Action::Minimize,
    Action::RestoreMinimized,
    Action::ToggleFill,
    Action::ShowDesktop,
    Action::SnapLeft,
    Action::SnapRight,
    Action::SnapUp,
    Action::SnapDown,
    Action::ShowWorkspace,
    Action::SendToWorkspace,
    Action::PrevWorkspace,
    Action::NextWorkspace,
    Action::Overview,
    Action::ShotAll,
    Action::ShotPointerOutput,
    Action::ShotRegion,
    Action::VolumeUp,
    Action::VolumeDown,
    Action::VolumeMute,
    Action::MicMute,
    Action::Spotlight,
    Action::NextLayout,
    Action::Terminal,
    Action::Files,
    Action::Lock,
    Action::Quit,
    Action::FocusFollowsMouse,
    Action::DebugOverlay,
];

/// The settings app's groups.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Group {
    // (The order the settings app shows them in; `GROUPS`.)
    Windows,
    Snapping,
    Workspaces,
    Screenshots,
    Sound,
    System,
}

impl Action {
    /// The name in `~/.config/kompact/keys` and on the shell channel.
    pub fn id(self) -> &'static str {
        match self {
            Action::Spotlight => "spotlight",
            Action::SwitchWindows => "window.switch",
            Action::CloseWindow => "window.close",
            Action::Minimize => "window.minimize",
            Action::RestoreMinimized => "window.restore",
            Action::ToggleFill => "window.maximize",
            Action::SnapLeft => "snap.left",
            Action::SnapRight => "snap.right",
            Action::SnapUp => "snap.up",
            Action::SnapDown => "snap.down",
            Action::ShowWorkspace => "workspace.show",
            Action::SendToWorkspace => "workspace.send",
            Action::PrevWorkspace => "workspace.previous",
            Action::NextWorkspace => "workspace.next",
            Action::Overview => "workspace.overview",
            Action::ShowDesktop => "desktop.show",
            Action::Terminal => "app.terminal",
            Action::Files => "app.files",
            Action::Lock => "session.lock",
            Action::Quit => "session.quit",
            Action::ShotAll => "screenshot.all",
            Action::ShotPointerOutput => "screenshot.monitor",
            Action::ShotRegion => "screenshot.region",
            Action::FocusFollowsMouse => "focus.follows-mouse",
            Action::DebugOverlay => "debug.overlay",
            Action::VolumeUp => "sound.volume-up",
            Action::VolumeDown => "sound.volume-down",
            Action::VolumeMute => "sound.mute",
            Action::MicMute => "sound.mic-mute",
            Action::NextLayout => "input.next-layout",
        }
    }

    pub fn from_id(id: &str) -> Option<Action> {
        ACTIONS.iter().copied().find(|a| a.id() == id)
    }

    pub fn label(self) -> &'static str {
        match self {
            Action::Spotlight => "Spotlight search",
            Action::SwitchWindows => "Switch to the next window",
            Action::CloseWindow => "Close the window",
            Action::Minimize => "Minimize the window",
            Action::RestoreMinimized => "Bring back the last minimized window",
            Action::ToggleFill => "Maximize or restore the window",
            Action::SnapLeft => "Snap left",
            Action::SnapRight => "Snap right",
            Action::SnapUp => "Snap up (maximize)",
            Action::SnapDown => "Snap down",
            Action::ShowWorkspace => "Show workspace 1–9",
            Action::SendToWorkspace => "Send the window to workspace 1–9",
            Action::PrevWorkspace => "Previous workspace",
            Action::NextWorkspace => "Next workspace",
            Action::Overview => "Workspace overview",
            Action::ShowDesktop => "Show the desktop",
            Action::Terminal => "Open a terminal",
            Action::Files => "Open Files",
            Action::Lock => "Lock the screen",
            Action::Quit => "Quit kompact",
            Action::ShotAll => "Screenshot of every monitor",
            Action::ShotPointerOutput => "Screenshot of the monitor under the pointer",
            Action::ShotRegion => "Screenshot of a region",
            Action::FocusFollowsMouse => "Focus follows the mouse on/off",
            Action::DebugOverlay => "Debug overlay on/off",
            Action::VolumeUp => "Volume up",
            Action::VolumeDown => "Volume down",
            Action::VolumeMute => "Mute or unmute the sound",
            Action::MicMute => "Mute or unmute the microphone",
            Action::NextLayout => "Next keyboard layout",
        }
    }

    pub fn group(self) -> Group {
        use Action::*;
        match self {
            SwitchWindows | CloseWindow | Minimize | RestoreMinimized | ToggleFill | ShowDesktop => Group::Windows,
            SnapLeft | SnapRight | SnapUp | SnapDown => Group::Snapping,
            ShowWorkspace | SendToWorkspace | PrevWorkspace | NextWorkspace | Overview => Group::Workspaces,
            ShotAll | ShotPointerOutput | ShotRegion => Group::Screenshots,
            VolumeUp | VolumeDown | VolumeMute | MicMute => Group::Sound,
            Spotlight | NextLayout | Terminal | Files | Lock | Quit | FocusFollowsMouse | DebugOverlay => Group::System,
        }
    }

    /// Bound to the 1–9 keys as a family (`1-9`), only the modifiers change.
    pub fn is_family(self) -> bool {
        matches!(self, Action::ShowWorkspace | Action::SendToWorkspace)
    }

    /// Must keep a key: the way out (Quit) and Lock.
    pub fn required(self) -> bool {
        matches!(self, Action::Quit | Action::Lock)
    }
}

pub const GROUPS: &[Group] = &[Group::Windows, Group::Snapping, Group::Workspaces, Group::Screenshots, Group::Sound, Group::System];

impl Group {
    pub fn label(self) -> &'static str {
        match self {
            Group::Windows => "Windows",
            Group::Snapping => "Snapping",
            Group::Workspaces => "Workspaces",
            Group::Screenshots => "Screenshots",
            Group::Sound => "Sound",
            Group::System => "System",
        }
    }
}

/// The modifiers held with a key (Caps Lock and Num Lock don't count).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Mods {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    pub logo: bool,
}

/// The key of a combination.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Key {
    /// An xkb keysym, lower case for letters (as the unshifted key gives it).
    Sym(u32),
    /// The physical 1–9 keys (workspaces).
    Digits,
    /// The modifiers alone, pressed and released (only a lone Super is supported).
    Tap,
}

/// A key with exactly these modifiers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Combo {
    pub mods: Mods,
    pub key: Key,
}

const SUPER_L: u32 = 0xffeb;
const SUPER_R: u32 = 0xffec;
const F1: u32 = 0xffbe;
const F12: u32 = 0xffc9;

pub fn is_super_key(sym: u32) -> bool {
    matches!(sym, SUPER_L | SUPER_R)
}

/// A modifier's own key (pressing it alone isn't a combination).
fn is_modifier_sym(sym: u32) -> bool {
    // Shift_L … Hyper_R, ISO_Level3_Shift and friends.
    (0xffe1..=0xffee).contains(&sym) || (0xfe01..=0xfe0f).contains(&sym)
}

impl Combo {
    /// Ctrl+Alt+F1–F12 switch consoles, always.
    pub fn is_reserved(&self) -> bool {
        let vt_mods = Mods { ctrl: true, alt: true, ..Mods::default() };
        matches!(self.key, Key::Sym(s) if self.mods == vt_mods && (F1..=F12).contains(&s))
    }

    /// Whether these two can't both be bound (the same key, or a digit and the 1–9 family).
    pub fn clashes(&self, other: &Combo) -> bool {
        if self.mods != other.mods {
            return false;
        }
        let digit = |k: Key| matches!(k, Key::Sym(s) if (0x31..=0x39).contains(&s));
        self.key == other.key
            || (self.key == Key::Digits && digit(other.key))
            || (other.key == Key::Digits && digit(self.key))
    }
}

/// Parse `Ctrl+Super+Left`, `Super+Shift+1-9`, `Super` (a tap), `Print`. Modifier names are
/// case-insensitive (Super = Win = Cmd = Logo, Ctrl = Control, Alt = Option); key names are xkb
/// keysym names, also case-insensitive (`q`, `Q`, `Return`, `space`, `F3`).
pub fn parse_combo(text: &str) -> Result<Combo, String> {
    let text = text.trim();
    if text.is_empty() {
        return Err("empty".into());
    }
    let parts: Vec<&str> = text.split('+').map(str::trim).collect();
    let mut mods = Mods::default();
    let mut key = None;
    for (i, part) in parts.iter().enumerate() {
        let last = i == parts.len() - 1;
        let m = match part.to_ascii_lowercase().as_str() {
            "ctrl" | "control" => Some(&mut mods.ctrl),
            "alt" | "option" | "opt" => Some(&mut mods.alt),
            "shift" => Some(&mut mods.shift),
            "super" | "win" | "cmd" | "command" | "logo" | "meta" => Some(&mut mods.logo),
            _ => None,
        };
        match m {
            Some(flag) if !*flag => *flag = true,
            Some(_) => return Err(format!("{part} twice in {text:?}")),
            None if !last => return Err(format!("{part:?} isn't a modifier (Ctrl, Alt, Shift, Super)")),
            None if part.is_empty() => return Err(format!("no key after the last + in {text:?}")),
            None if *part == "1-9" => key = Some(Key::Digits),
            None => {
                let sym = xkb::keysym_from_name(part, xkb::KEYSYM_CASE_INSENSITIVE).raw();
                if sym == 0 {
                    return Err(format!("no key called {part:?}"));
                }
                if is_modifier_sym(sym) {
                    return Err(format!("{part:?} is a modifier; a lone Super tap is written \"Super\""));
                }
                key = Some(Key::Sym(sym));
            }
        }
    }
    let key = match key {
        Some(k) => k,
        // Only modifiers: a tap, and only a lone Super.
        None if mods == (Mods { logo: true, ..Mods::default() }) => Key::Tap,
        None => return Err(format!("{text:?} has no key (only a lone \"Super\" tap can be bound)")),
    };
    Ok(Combo { mods, key })
}

impl fmt::Display for Combo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut parts: Vec<String> = Vec::new();
        for (on, name) in [(self.mods.ctrl, "Ctrl"), (self.mods.alt, "Alt"), (self.mods.shift, "Shift"), (self.mods.logo, "Super")] {
            if on {
                parts.push(name.into());
            }
        }
        match self.key {
            Key::Tap => {}
            Key::Digits => parts.push("1-9".into()),
            Key::Sym(s) => {
                let name = xkb::keysym_get_name(xkb::Keysym::new(s));
                // Letters as on the keycap.
                parts.push(if name.len() == 1 { name.to_ascii_uppercase() } else { name });
            }
        }
        write!(f, "{}", parts.join("+"))
    }
}

/// Comma-separated combinations, or `none`.
pub fn parse_combos(text: &str) -> Result<Vec<Combo>, String> {
    if text.trim().eq_ignore_ascii_case("none") {
        return Ok(Vec::new());
    }
    let combos = text.split(',').map(parse_combo).collect::<Result<Vec<_>, _>>()?;
    if combos.len() > 2 {
        return Err("at most two combinations per action".into());
    }
    Ok(combos)
}

pub fn format_combos(combos: &[Combo]) -> String {
    if combos.is_empty() {
        return "none".into();
    }
    combos.iter().map(Combo::to_string).collect::<Vec<_>>().join(", ")
}

/// A preset shortcut set.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Preset {
    MacOs,
    Windows,
    Classic,
}

pub const PRESETS: &[Preset] = &[Preset::MacOs, Preset::Windows, Preset::Classic];

impl Preset {
    pub fn id(self) -> &'static str {
        match self {
            Preset::MacOs => "macos",
            Preset::Windows => "windows",
            Preset::Classic => "classic",
        }
    }

    pub fn from_id(id: &str) -> Option<Preset> {
        PRESETS.iter().copied().find(|p| p.id().eq_ignore_ascii_case(id.trim()))
    }

    pub fn label(self) -> &'static str {
        match self {
            Preset::MacOs => "macOS",
            Preset::Windows => "Windows 11",
            Preset::Classic => "kompact classic",
        }
    }

    /// Windows: Super+Down minimizes a window that isn't snapped or maximized.
    pub fn floating_down_minimizes(self) -> bool {
        self == Preset::Windows
    }

    /// The preset's keys, as written in the file.
    fn table(self) -> &'static [(Action, &'static str)] {
        use Action::*;
        match self {
            Preset::Classic => &[
                (Spotlight, "Super+space"),
                (SwitchWindows, "Super+Tab"),
                (CloseWindow, "Super+Q"),
                (RestoreMinimized, "Super+U"),
                (ToggleFill, "Super+F"),
                (SnapLeft, "Super+Left"),
                (SnapRight, "Super+Right"),
                (SnapUp, "Super+Up"),
                (SnapDown, "Super+Down"),
                (ShowWorkspace, "Super+1-9"),
                (SendToWorkspace, "Shift+Super+1-9"),
                (Terminal, "Super+Return"),
                (Lock, "Super+L"),
                (Quit, "Super+Escape"),
                (ShotAll, "Print"),
                (ShotPointerOutput, "Ctrl+Print"),
                (ShotRegion, "Shift+Print"),
                (VolumeUp, "XF86AudioRaiseVolume"),
                (VolumeDown, "XF86AudioLowerVolume"),
                (VolumeMute, "XF86AudioMute"),
                (MicMute, "XF86AudioMicMute"),
                (NextLayout, "Ctrl+Alt+space"),
                (FocusFollowsMouse, "Super+M"),
                (DebugOverlay, "Super+D"),
            ],
            Preset::MacOs => &[
                (Spotlight, "Super+space"),
                (SwitchWindows, "Super+Tab"),
                (CloseWindow, "Super+Q"),
                (Minimize, "Super+M"),
                (RestoreMinimized, "Super+U"),
                (ToggleFill, "Ctrl+Super+F"),
                (SnapLeft, "Ctrl+Super+Left"),
                (SnapRight, "Ctrl+Super+Right"),
                (SnapUp, "Ctrl+Super+Up"),
                (SnapDown, "Ctrl+Super+Down"),
                (ShowWorkspace, "Ctrl+1-9"),
                (SendToWorkspace, "Ctrl+Shift+1-9"),
                (PrevWorkspace, "Ctrl+Left"),
                (NextWorkspace, "Ctrl+Right"),
                (Overview, "Ctrl+Up"),
                (ShowDesktop, "Super+F3"),
                (Terminal, "Super+Return"),
                (Files, "Shift+Super+F"),
                (Lock, "Ctrl+Super+Q"),
                (Quit, "Shift+Super+Escape"),
                (ShotAll, "Shift+Super+3, Print"),
                (ShotPointerOutput, "Ctrl+Print"),
                (ShotRegion, "Shift+Super+4, Shift+Print"),
                (VolumeUp, "XF86AudioRaiseVolume"),
                (VolumeDown, "XF86AudioLowerVolume"),
                (VolumeMute, "XF86AudioMute"),
                (MicMute, "XF86AudioMicMute"),
                (NextLayout, "Ctrl+space"),
                (DebugOverlay, "Ctrl+Alt+Super+D"),
            ],
            Preset::Windows => &[
                (Spotlight, "Super+S, Super"),
                (SwitchWindows, "Alt+Tab"),
                (CloseWindow, "Alt+F4"),
                (RestoreMinimized, "Super+U"),
                (ToggleFill, "Super+F"),
                (SnapLeft, "Super+Left"),
                (SnapRight, "Super+Right"),
                (SnapUp, "Super+Up"),
                (SnapDown, "Super+Down"),
                (ShowWorkspace, "Ctrl+Super+1-9"),
                (SendToWorkspace, "Shift+Super+1-9"),
                (PrevWorkspace, "Ctrl+Super+Left"),
                (NextWorkspace, "Ctrl+Super+Right"),
                (Overview, "Super+Tab"),
                (ShowDesktop, "Super+D"),
                (Terminal, "Super+Return"),
                (Files, "Super+E"),
                (Lock, "Super+L"),
                (Quit, "Shift+Super+Escape"),
                (ShotAll, "Print"),
                (ShotPointerOutput, "Alt+Print"),
                (ShotRegion, "Shift+Super+S, Shift+Print"),
                (VolumeUp, "XF86AudioRaiseVolume"),
                (VolumeDown, "XF86AudioLowerVolume"),
                (VolumeMute, "XF86AudioMute"),
                (MicMute, "XF86AudioMicMute"),
                (NextLayout, "Super+space"),
                (DebugOverlay, "Ctrl+Alt+Super+D"),
            ],
        }
    }

    /// The preset's combinations for `action` (none if it has no key there).
    pub fn combos(self, action: Action) -> Vec<Combo> {
        self.table()
            .iter()
            .find(|(a, _)| *a == action)
            .map(|(_, text)| parse_combos(text).expect("preset tables are valid (tested)"))
            .unwrap_or_default()
    }
}

/// A key press as the keyboard filter sees it.
#[derive(Debug, Clone, Copy)]
pub struct Press {
    /// The key's unshifted symbol (level 1 of its layout).
    pub sym: u32,
    /// The xkb keycode (for the physical 1–9 keys).
    pub keycode: u32,
    pub mods: Mods,
}

/// The shortcuts in use: a preset plus the user's changes.
#[derive(Debug, Clone)]
pub struct Keymap {
    preset: Preset,
    keys: HashMap<Action, Vec<Combo>>,
}

impl Default for Keymap {
    fn default() -> Self {
        Keymap::new(Preset::MacOs)
    }
}

impl Keymap {
    pub fn new(preset: Preset) -> Keymap {
        let keys = ACTIONS.iter().map(|&a| (a, preset.combos(a))).collect();
        Keymap { preset, keys }
    }

    pub fn preset(&self) -> Preset {
        self.preset
    }

    pub fn combos(&self, action: Action) -> &[Combo] {
        self.keys.get(&action).map(Vec::as_slice).unwrap_or(&[])
    }

    /// Whether the user changed this one from the preset.
    pub fn is_changed(&self, action: Action) -> bool {
        self.combos(action) != self.preset.combos(action).as_slice()
    }

    /// Give `action` these combinations, if they're allowed: right kind of key, not reserved,
    /// not taken by another action, and Quit/Lock not left without one.
    pub fn set(&mut self, action: Action, combos: Vec<Combo>) -> Result<(), String> {
        if combos.is_empty() && action.required() {
            return Err(format!("{} must keep a key", action.id()));
        }
        if combos.len() > 2 {
            return Err("at most two combinations per action".into());
        }
        for c in &combos {
            if c.is_reserved() {
                return Err(format!("{c} switches consoles and can't be changed"));
            }
            match (action.is_family(), c.key) {
                (true, Key::Digits) | (false, Key::Sym(_)) => {}
                (true, _) => return Err(format!("{} takes the 1–9 keys, e.g. \"Ctrl+1-9\"", action.id())),
                (false, Key::Digits) => return Err(format!("only the workspace actions take \"1-9\"; {} needs one key", action.id())),
                (false, Key::Tap) if action != Action::Spotlight && action.group() != Group::System => {
                    return Err("a lone Super tap can open Spotlight or the System actions only".into());
                }
                (false, Key::Tap) => {}
            }
            for (&other, others) in &self.keys {
                if other != action && others.iter().any(|o| o.clashes(c)) {
                    return Err(format!("{c} is already {} ({})", other.id(), other.label()));
                }
            }
        }
        if combos.len() == 2 && combos[0].clashes(&combos[1]) {
            return Err("the same combination twice".into());
        }
        self.keys.insert(action, combos);
        Ok(())
    }

    /// What `press` does, with the workspace (0-based) for the 1–9 actions.
    pub fn lookup(&self, press: &Press) -> Option<(Action, Option<usize>)> {
        // Single keys before the 1–9 family, so a changed Super+Shift+3 wins over it.
        for &a in ACTIONS {
            if self.combos(a).iter().any(|c| c.mods == press.mods && c.key == Key::Sym(press.sym)) {
                return Some((a, None));
            }
        }
        let n = crate::workspace::workspace_for_keycode(press.keycode)?;
        ACTIONS
            .iter()
            .copied()
            .find(|&a| self.combos(a).iter().any(|c| c.mods == press.mods && c.key == Key::Digits))
            .map(|a| (a, Some(n)))
    }

    /// The action a lone Super tap does, if any.
    pub fn tap_action(&self) -> Option<Action> {
        ACTIONS.iter().copied().find(|&a| self.combos(a).iter().any(|c| c.key == Key::Tap))
    }

    pub fn floating_down_minimizes(&self) -> bool {
        self.preset.floating_down_minimizes()
    }

    /// One line per group for the log: `Windows: window.switch=Super+Tab, …` (changed ones
    /// marked `*`).
    pub fn summary_lines(&self) -> Vec<String> {
        GROUPS
            .iter()
            .map(|&g| {
                let items: Vec<String> = ACTIONS
                    .iter()
                    .filter(|a| a.group() == g)
                    .map(|&a| format!("{}{}={}", a.id(), if self.is_changed(a) { "*" } else { "" }, format_combos(self.combos(a))))
                    .collect();
                format!("{}: {}", g.label(), items.join(", "))
            })
            .collect()
    }

    /// Read a keys file: the preset line, then changes. Returns the keymap and what was wrong.
    pub fn parse(text: &str) -> (Keymap, Vec<String>) {
        let mut errors = Vec::new();
        let mut lines = Vec::new();
        let mut preset = Preset::MacOs;
        for (n, line) in text.lines().enumerate() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let Some((k, v)) = line.split_once('=') else {
                errors.push(format!("line {}: no '=' in {line:?}", n + 1));
                continue;
            };
            let (k, v) = (k.trim(), v.trim());
            if k == "preset" {
                match Preset::from_id(v) {
                    Some(p) => preset = p,
                    None => errors.push(format!("line {}: no preset {v:?} (macos, windows, classic)", n + 1)),
                }
            } else {
                lines.push((n + 1, k.to_string(), v.to_string()));
            }
        }
        let mut map = Keymap::new(preset);
        for (n, k, v) in lines {
            let Some(action) = Action::from_id(&k) else {
                errors.push(format!("line {n}: no action {k:?}"));
                continue;
            };
            match parse_combos(&v).and_then(|c| map.set(action, c)) {
                Ok(()) => {}
                Err(e) => errors.push(format!("line {n} ({k}): {e}; keeping {}", format_combos(map.combos(action)))),
            }
        }
        (map, errors)
    }

    /// 10b: this action back to the preset's keys (fails if another action took them since).
    pub fn reset(&mut self, action: Action) -> Result<(), String> {
        self.set(action, self.preset.combos(action))
    }

    /// 10b: the same keymap on another preset, with the user's changes carried over (`keep`) as
    /// far as they still fit; returns the new keymap and the changes that had to be dropped.
    pub fn with_preset(&self, preset: Preset, keep: bool) -> (Keymap, Vec<String>) {
        let mut map = Keymap::new(preset);
        let mut dropped = Vec::new();
        if keep {
            for &a in ACTIONS {
                if self.is_changed(a) {
                    if let Err(e) = map.set(a, self.combos(a).to_vec()) {
                        dropped.push(format!("{} = {} ({e})", a.id(), format_combos(self.combos(a))));
                    }
                }
            }
        }
        (map, dropped)
    }

    /// The file for this keymap: the preset, then only what differs from it.
    pub fn to_text(&self) -> String {
        let mut out = format!(
            "# kompact keyboard shortcuts (10a): a preset, then your changes from it.\n\
             # Edit in System Settings, or here: action = keys[, keys] or none.\n\
             preset = {}\n",
            self.preset.id()
        );
        for &a in ACTIONS {
            if self.is_changed(a) {
                out.push_str(&format!("{} = {}\n", a.id(), format_combos(self.combos(a))));
            }
        }
        out
    }
}

/// 10b: combinations for a workspace action given with one digit (as recorded: "Ctrl+3") mean
/// the 1–9 family ("Ctrl+1-9").
pub fn normalize_for(action: Action, combos: Vec<Combo>) -> Vec<Combo> {
    if !action.is_family() {
        return combos;
    }
    combos
        .into_iter()
        .map(|c| match c.key {
            Key::Sym(s) if (0x31..=0x39).contains(&s) => Combo { key: Key::Digits, ..c },
            _ => c,
        })
        .collect()
}

/// How long `keys record` waits for a combination.
pub const RECORD_TIMEOUT: Duration = Duration::from_secs(10);
const ESCAPE: u32 = 0xff1b;

/// 10b: what a recording ended with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Recorded {
    Combo(Combo),
    Cancelled(&'static str),
}

/// 10b: the settings app asked for the next combination: kompact reports it instead of acting
/// on it. Modifier keys alone wait; Esc (alone) cancels; a lone Super tap counts.
#[derive(Debug, Default)]
pub struct Recorder {
    started: Option<Instant>,
}

impl Recorder {
    pub fn start(&mut self, now: Instant) {
        self.started = Some(now);
    }

    pub fn is_active(&self) -> bool {
        self.started.is_some()
    }

    /// A key went down while recording: `None` keeps waiting (a modifier on its own).
    pub fn press(&mut self, press: &Press) -> Option<Recorded> {
        self.started?;
        if is_modifier_sym(press.sym) {
            return None;
        }
        self.started = None;
        if press.sym == ESCAPE && press.mods == Mods::default() {
            return Some(Recorded::Cancelled("Esc"));
        }
        Some(Recorded::Combo(Combo { mods: press.mods, key: Key::Sym(press.sym) }))
    }

    /// Super was tapped alone while recording.
    pub fn tap(&mut self) -> Option<Recorded> {
        self.started.take()?;
        Some(Recorded::Combo(Combo { mods: Mods { logo: true, ..Mods::default() }, key: Key::Tap }))
    }

    pub fn cancel(&mut self, why: &'static str) -> Option<Recorded> {
        self.started.take().map(|_| Recorded::Cancelled(why))
    }

    /// Gave up after `RECORD_TIMEOUT`.
    pub fn timed_out(&mut self, now: Instant) -> Option<Recorded> {
        let started = self.started?;
        (now.duration_since(started) >= RECORD_TIMEOUT).then(|| self.cancel("timeout")).flatten()
    }
}

/// 10b: write the keys file safely (a `.part` file, then renamed over it).
pub fn save_to(path: &Path, text: &str) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    let part = path.with_extension("part");
    std::fs::write(&part, text).map_err(|e| format!("{}: {e}", part.display()))?;
    std::fs::rename(&part, path).map_err(|e| format!("{}: {e}", path.display()))
}

/// 10b: save `map` to `~/.config/kompact/keys`; returns where.
pub fn save(map: &Keymap) -> Result<PathBuf, String> {
    let config = std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from);
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let path = keys_path(config.as_deref(), home.as_deref()).ok_or("no home directory")?;
    save_to(&path, &map.to_text())?;
    Ok(path)
}

/// 10a: a lone tap of Super (pressed and released with no other key or button in between).
#[derive(Debug, Default)]
pub struct Tap {
    armed: bool,
}

impl Tap {
    /// A key went down: arms on a Super key pressed alone, disarms on anything else.
    pub fn press(&mut self, sym: u32, mods_before: Mods) {
        self.armed = is_super_key(sym) && !mods_before.ctrl && !mods_before.alt && !mods_before.shift;
    }

    /// A key came up: true if it was the tap.
    pub fn release(&mut self, sym: u32) -> bool {
        let fired = self.armed && is_super_key(sym);
        if fired || !is_super_key(sym) {
            self.armed = false;
        }
        fired
    }

    /// A pointer button or scroll while Super is held (Super+drag moves windows): no tap.
    pub fn cancel(&mut self) {
        self.armed = false;
    }
}

/// `~/.config/kompact/keys`.
pub fn keys_path(config_home: Option<&Path>, home: Option<&Path>) -> Option<PathBuf> {
    config_home
        .map(Path::to_path_buf)
        .or_else(|| home.map(|h| h.join(".config")))
        .map(|c| c.join("kompact").join("keys"))
}

/// The keymap to start with: `KOMPACT_KEYS` (a preset, the file ignored), else the keys file,
/// else the macOS preset. Logs what it did, and the keys.
pub fn load() -> Keymap {
    let map = choose();
    if map.floating_down_minimizes() {
        tracing::info!("keys: Super+Down minimizes a window that isn't snapped or maximized (as on Windows)");
    }
    for line in map.summary_lines() {
        tracing::info!("keys: {line}");
    }
    map
}

fn choose() -> Keymap {
    if let Some(p) = std::env::var("KOMPACT_KEYS").ok().filter(|v| !v.trim().is_empty()) {
        match Preset::from_id(&p) {
            Some(preset) => {
                tracing::info!("keys: preset {} ({}) from KOMPACT_KEYS (the keys file is ignored)", preset.id(), preset.label());
                return Keymap::new(preset);
            }
            None => tracing::warn!("keys: KOMPACT_KEYS={p:?} isn't a preset (macos, windows, classic); using the keys file"),
        }
    }
    let config = std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from);
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let Some(path) = keys_path(config.as_deref(), home.as_deref()) else {
        tracing::info!("keys: preset macos (no home directory)");
        return Keymap::default();
    };
    match std::fs::read_to_string(&path) {
        Ok(text) => {
            let (map, errors) = Keymap::parse(&text);
            for e in &errors {
                tracing::warn!("keys: {}: {e}", path.display());
            }
            let changed: Vec<&str> = ACTIONS.iter().filter(|&&a| map.is_changed(a)).map(|a| a.id()).collect();
            tracing::info!(
                "keys: preset {} ({}) from {}, {} changed{}",
                map.preset().id(),
                map.preset().label(),
                path.display(),
                changed.len(),
                if changed.is_empty() { String::new() } else { format!(" ({})", changed.join(", ")) }
            );
            map
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            tracing::info!("keys: preset macos (no {}; KOMPACT_KEYS=classic for the old keys)", path.display());
            Keymap::default()
        }
        Err(e) => {
            tracing::warn!("keys: {}: {e}; preset macos", path.display());
            Keymap::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const Q: u32 = 0x71;
    const PRINT: u32 = 0xff61;
    const LEFT: u32 = 0xff51;
    const TAB: u32 = 0xff09;

    fn mods(spec: &str) -> Mods {
        Mods { ctrl: spec.contains('c'), alt: spec.contains('a'), shift: spec.contains('s'), logo: spec.contains('w') }
    }

    fn press(sym: u32, m: &str) -> Press {
        Press { sym, keycode: 0, mods: mods(m) }
    }

    #[test]
    fn combos_parse_and_print() {
        let c = parse_combo("Ctrl+Super+Left").unwrap();
        assert_eq!(c, Combo { mods: mods("cw"), key: Key::Sym(LEFT) });
        assert_eq!(c.to_string(), "Ctrl+Super+Left");
        // Any order and case; Cmd/Win are Super; letters as on the keycap.
        assert_eq!(parse_combo("q+cmd").unwrap_err(), "\"q\" isn't a modifier (Ctrl, Alt, Shift, Super)");
        assert_eq!(parse_combo("cmd+Q").unwrap(), Combo { mods: mods("w"), key: Key::Sym(Q) });
        assert_eq!(parse_combo("win+shift+q").unwrap().to_string(), "Shift+Super+Q");
        assert_eq!(parse_combo("Super+Shift+3").unwrap().key, Key::Sym(0x33));
        assert_eq!(parse_combo("Ctrl+Shift+1-9").unwrap(), Combo { mods: mods("cs"), key: Key::Digits });
        assert_eq!(parse_combo("Super").unwrap(), Combo { mods: mods("w"), key: Key::Tap });
        assert_eq!(parse_combo("print").unwrap().to_string(), "Print");
        assert_eq!(parse_combo("Super+space").unwrap().to_string(), "Super+space");
        assert!(parse_combo("Ctrl").is_err());
        assert!(parse_combo("Super+Shift").is_err());
        assert!(parse_combo("Super+Nonsense").is_err());
        assert!(parse_combo("Super+Super_L").is_err());
        assert!(parse_combo("Super+Super+Q").is_err());
        assert!(parse_combo("Super+").is_err());
        assert_eq!(parse_combos("none").unwrap(), vec![]);
        assert_eq!(parse_combos("Super+3, Print").unwrap().len(), 2);
        assert!(parse_combos("A, B, C").is_err());
        for text in ["Ctrl+Super+Left", "Shift+Super+3, Print", "Ctrl+1-9", "Super", "Alt+F4", "none"] {
            let p = parse_combos(text).unwrap();
            assert_eq!(parse_combos(&format_combos(&p)).unwrap(), p, "{text}");
        }
    }

    #[test]
    fn media_keys() {
        // 12c: the volume keys by their XF86 names, in every preset, no modifiers.
        const RAISE: u32 = 0x1008ff13;
        const MUTE: u32 = 0x1008ff12;
        assert_eq!(parse_combo("XF86AudioRaiseVolume").unwrap(), Combo { mods: mods(""), key: Key::Sym(RAISE) });
        assert_eq!(parse_combo("XF86AudioMute").unwrap().to_string(), "XF86AudioMute");
        for &p in PRESETS {
            let map = Keymap::new(p);
            assert_eq!(map.lookup(&press(RAISE, "")), Some((Action::VolumeUp, None)), "{p:?}");
            assert_eq!(map.lookup(&press(MUTE, "")), Some((Action::VolumeMute, None)), "{p:?}");
        }
    }

    #[test]
    fn presets_are_valid_and_free_of_clashes() {
        for &p in PRESETS {
            let map = Keymap::new(p);
            let all: Vec<(Action, Combo)> = ACTIONS.iter().flat_map(|&a| map.combos(a).iter().map(move |&c| (a, c))).collect();
            for (i, (a, c)) in all.iter().enumerate() {
                assert!(!c.is_reserved(), "{p:?} {a:?}");
                assert_eq!(a.is_family(), c.key == Key::Digits, "{p:?} {a:?} {c}");
                for (b, d) in &all[i + 1..] {
                    assert!(!c.clashes(d), "{p:?}: {a:?} and {b:?} both {c}");
                }
            }
            assert!(!map.combos(Action::Quit).is_empty() && !map.combos(Action::Lock).is_empty(), "{p:?}");
            // Rebuilding each action through `set` accepts the preset as it is.
            let mut copy = map.clone();
            for &a in ACTIONS {
                copy.set(a, map.combos(a).to_vec()).unwrap_or_else(|e| panic!("{p:?} {a:?}: {e}"));
            }
            assert!(ACTIONS.iter().all(|&a| !map.is_changed(a)));
        }
    }

    #[test]
    fn classic_is_the_old_keys() {
        let map = Keymap::new(Preset::Classic);
        assert_eq!(map.lookup(&press(Q, "w")), Some((Action::CloseWindow, None)));
        assert_eq!(map.lookup(&press(TAB, "w")), Some((Action::SwitchWindows, None)));
        assert_eq!(map.lookup(&press(0x6d, "w")), Some((Action::FocusFollowsMouse, None)));
        assert_eq!(map.lookup(&press(0x64, "w")), Some((Action::DebugOverlay, None)));
        assert_eq!(map.lookup(&press(0xff1b, "w")), Some((Action::Quit, None)));
        assert_eq!(map.lookup(&press(PRINT, "")), Some((Action::ShotAll, None)));
        assert_eq!(map.lookup(&press(PRINT, "c")), Some((Action::ShotPointerOutput, None)));
        assert_eq!(map.lookup(&press(PRINT, "s")), Some((Action::ShotRegion, None)));
        // Exactly these modifiers: Super+Print and Alt+Print stay free, as before.
        assert_eq!(map.lookup(&press(PRINT, "w")), None);
        assert_eq!(map.lookup(&press(PRINT, "a")), None);
        // Workspaces by physical key (keycode 12 is "3").
        assert_eq!(map.lookup(&Press { sym: 0x33, keycode: 12, mods: mods("w") }), Some((Action::ShowWorkspace, Some(2))));
        assert_eq!(map.lookup(&Press { sym: 0x23, keycode: 12, mods: mods("ws") }), Some((Action::SendToWorkspace, Some(2))));
        assert_eq!(map.tap_action(), None);
        assert!(!map.floating_down_minimizes());
    }

    #[test]
    fn mac_and_windows_presets() {
        let mac = Keymap::new(Preset::MacOs);
        assert_eq!(mac.lookup(&press(0x6d, "w")), Some((Action::Minimize, None)));
        assert_eq!(mac.lookup(&press(LEFT, "cw")), Some((Action::SnapLeft, None)));
        assert_eq!(mac.lookup(&press(LEFT, "c")), Some((Action::PrevWorkspace, None)));
        assert_eq!(mac.lookup(&press(0x33, "sw")), Some((Action::ShotAll, None)));
        assert_eq!(mac.lookup(&press(PRINT, "")), Some((Action::ShotAll, None)));
        assert_eq!(mac.lookup(&Press { sym: 0x31, keycode: 10, mods: mods("c") }), Some((Action::ShowWorkspace, Some(0))));
        assert_eq!(mac.lookup(&press(Q, "cw")), Some((Action::Lock, None)));
        assert_eq!(mac.lookup(&press(0xff1b, "w")), None); // quit needs Shift now
        assert_eq!(mac.lookup(&press(0xff1b, "sw")), Some((Action::Quit, None)));
        let win = Keymap::new(Preset::Windows);
        assert_eq!(win.lookup(&press(TAB, "a")), Some((Action::SwitchWindows, None)));
        assert_eq!(win.lookup(&press(TAB, "w")), Some((Action::Overview, None)));
        assert_eq!(win.lookup(&press(0xffc1, "a")), Some((Action::CloseWindow, None))); // Alt+F4
        assert_eq!(win.lookup(&press(0x64, "w")), Some((Action::ShowDesktop, None)));
        assert_eq!(win.lookup(&press(PRINT, "a")), Some((Action::ShotPointerOutput, None)));
        assert_eq!(win.lookup(&press(0x73, "w")), Some((Action::Spotlight, None))); // Super+S
        assert_eq!(win.tap_action(), Some(Action::Spotlight));
        assert!(win.floating_down_minimizes());
    }

    #[test]
    fn changes_are_checked() {
        let mut map = Keymap::new(Preset::MacOs);
        let c = |t: &str| parse_combos(t).unwrap();
        assert!(map.set(Action::CloseWindow, c("Super+W")).is_ok());
        assert!(map.is_changed(Action::CloseWindow));
        assert_eq!(map.lookup(&press(0x77, "w")), Some((Action::CloseWindow, None)));
        assert_eq!(map.lookup(&press(Q, "w")), None);
        // Taken, reserved, required, wrong kind.
        assert!(map.set(Action::Files, c("Super+M")).unwrap_err().contains("window.minimize"));
        assert!(map.set(Action::Files, c("Ctrl+Alt+F2")).unwrap_err().contains("consoles"));
        assert!(map.set(Action::Quit, c("none")).unwrap_err().contains("must keep"));
        assert!(map.set(Action::ShowWorkspace, c("Super+3")).is_err());
        assert!(map.set(Action::Files, c("Super+1-9")).is_err());
        assert!(map.set(Action::SnapLeft, c("Super")).is_err());
        // A digit clashes with the 1–9 family under the same modifiers.
        assert!(map.set(Action::Files, c("Ctrl+5")).unwrap_err().contains("workspace.show"));
        // Unbinding frees the key for another action.
        assert!(map.set(Action::Minimize, c("none")).is_ok());
        assert!(map.set(Action::Files, c("Super+M")).is_ok());
    }

    #[test]
    fn keys_file_round_trip() {
        let (map, errors) = Keymap::parse("");
        assert!(errors.is_empty());
        assert_eq!(map.preset(), Preset::MacOs);
        let text = "# mine\npreset = windows\nwindow.close = Super+W\ndesktop.show = none\nscreenshot.region = Alt+Super+R, Shift+Print\n";
        let (map, errors) = Keymap::parse(text);
        assert!(errors.is_empty(), "{errors:?}");
        assert_eq!(map.preset(), Preset::Windows);
        assert_eq!(map.combos(Action::ShowDesktop), &[]);
        let (again, _) = Keymap::parse(&map.to_text());
        for &a in ACTIONS {
            assert_eq!(again.combos(a), map.combos(a), "{a:?}");
        }
        assert!(map.to_text().contains("window.close = Super+W\n"));
        assert!(!map.to_text().contains("snap.left")); // unchanged: not written
        // Bad lines are reported and skipped; the rest still applies.
        let (map, errors) = Keymap::parse("preset = amiga\nwindow.close = Super+W\nnope = Super+X\nsession.quit = none\nspotlight Super+K\n");
        assert_eq!(errors.len(), 4, "{errors:?}");
        assert_eq!(map.preset(), Preset::MacOs);
        assert_eq!(map.lookup(&press(0x77, "w")), Some((Action::CloseWindow, None)));
        assert!(!map.combos(Action::Quit).is_empty());
    }

    #[test]
    fn summary() {
        let mut map = Keymap::new(Preset::Windows);
        map.set(Action::CloseWindow, parse_combos("Super+W").unwrap()).unwrap();
        let lines = map.summary_lines();
        assert_eq!(lines.len(), GROUPS.len());
        assert!(lines[0].starts_with("Windows: window.switch=Alt+Tab, window.close*=Super+W, window.minimize=none"), "{}", lines[0]);
        assert!(lines[4].starts_with("Sound: sound.volume-up=XF86AudioRaiseVolume"), "{}", lines[4]);
        assert!(lines[5].contains("spotlight=Super+S, Super"), "{}", lines[5]);
    }

    #[test]
    fn super_taps() {
        let mut t = Tap::default();
        t.press(SUPER_L, Mods::default());
        assert!(t.release(SUPER_L));
        // Super+Q: no tap.
        t.press(SUPER_L, Mods::default());
        t.press(Q, mods("w"));
        assert!(!t.release(Q));
        assert!(!t.release(SUPER_L));
        // Super with Ctrl held, or a click in between: no tap.
        t.press(SUPER_L, mods("c"));
        assert!(!t.release(SUPER_L));
        t.press(SUPER_R, Mods::default());
        t.cancel();
        assert!(!t.release(SUPER_R));
    }

    #[test]
    fn presets_switch_with_or_without_changes() {
        let mut mac = Keymap::new(Preset::MacOs);
        mac.set(Action::CloseWindow, parse_combos("Super+W").unwrap()).unwrap();
        mac.set(Action::Files, parse_combos("Alt+Super+F").unwrap()).unwrap();
        mac.set(Action::Terminal, parse_combos("Super+D").unwrap()).unwrap(); // free on macOS
        let (win, dropped) = mac.with_preset(Preset::Windows, true);
        assert_eq!(win.preset(), Preset::Windows);
        assert_eq!(win.combos(Action::CloseWindow), parse_combos("Super+W").unwrap().as_slice());
        assert_eq!(win.combos(Action::Files), parse_combos("Alt+Super+F").unwrap().as_slice());
        // Super+D is Show desktop on Windows: that change can't come along.
        assert_eq!(dropped.len(), 1, "{dropped:?}");
        assert!(dropped[0].starts_with("app.terminal = Super+Return") || dropped[0].starts_with("app.terminal = Super+D"), "{dropped:?}");
        let (clean, dropped) = mac.with_preset(Preset::Windows, false);
        assert!(dropped.is_empty());
        assert!(ACTIONS.iter().all(|&a| !clean.is_changed(a)));
        // Reset one action; and it fails if its key was given away meanwhile.
        let mut m = Keymap::new(Preset::MacOs);
        m.set(Action::Minimize, vec![]).unwrap();
        m.set(Action::Files, parse_combos("Super+M").unwrap()).unwrap();
        assert!(m.reset(Action::Minimize).unwrap_err().contains("app.files"));
        m.reset(Action::Files).unwrap();
        m.reset(Action::Minimize).unwrap();
        assert!(ACTIONS.iter().all(|&a| !m.is_changed(a)));
    }

    #[test]
    fn workspace_keys_from_one_digit() {
        let c = parse_combos("Ctrl+3, Alt+Super+5").unwrap();
        let fam = normalize_for(Action::ShowWorkspace, c.clone());
        assert_eq!(fam.iter().map(Combo::to_string).collect::<Vec<_>>(), ["Ctrl+1-9", "Alt+Super+1-9"]);
        assert_eq!(normalize_for(Action::CloseWindow, c.clone()), c);
    }

    #[test]
    fn recording() {
        let t0 = Instant::now();
        let mut r = Recorder::default();
        assert_eq!(r.press(&press(Q, "w")), None); // not recording: nothing
        r.start(t0);
        assert!(r.is_active());
        assert_eq!(r.press(&press(SUPER_L, "")), None); // a modifier alone: keep waiting
        assert_eq!(r.press(&press(Q, "cw")), Some(Recorded::Combo(parse_combo("Ctrl+Super+Q").unwrap())));
        assert!(!r.is_active());
        r.start(t0);
        assert_eq!(r.press(&press(ESCAPE, "")), Some(Recorded::Cancelled("Esc")));
        r.start(t0);
        assert_eq!(r.press(&press(ESCAPE, "s")), Some(Recorded::Combo(parse_combo("Shift+Escape").unwrap())));
        r.start(t0);
        assert_eq!(r.tap(), Some(Recorded::Combo(parse_combo("Super").unwrap())));
        r.start(t0);
        assert_eq!(r.timed_out(t0 + Duration::from_secs(9)), None);
        assert_eq!(r.timed_out(t0 + RECORD_TIMEOUT), Some(Recorded::Cancelled("timeout")));
        assert_eq!(r.cancel("locked"), None);
        r.start(t0);
        assert_eq!(r.cancel("locked"), Some(Recorded::Cancelled("locked")));
    }

    #[test]
    fn saving() {
        let dir = std::env::temp_dir().join(format!("kompact-keys-test-{}", std::process::id()));
        let path = dir.join("kompact").join("keys");
        let mut map = Keymap::new(Preset::Windows);
        map.set(Action::CloseWindow, parse_combos("Super+W").unwrap()).unwrap();
        save_to(&path, &map.to_text()).unwrap();
        let (back, errors) = Keymap::parse(&std::fs::read_to_string(&path).unwrap());
        assert!(errors.is_empty());
        assert_eq!(back.preset(), Preset::Windows);
        assert_eq!(back.combos(Action::CloseWindow), map.combos(Action::CloseWindow));
        assert!(!path.with_extension("part").exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn keys_file_place() {
        assert_eq!(keys_path(None, Some(Path::new("/home/u"))), Some(PathBuf::from("/home/u/.config/kompact/keys")));
        assert_eq!(keys_path(Some(Path::new("/c")), Some(Path::new("/home/u"))), Some(PathBuf::from("/c/kompact/keys")));
        assert_eq!(keys_path(None, None), None);
    }
}
