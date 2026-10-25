//! 20e: the shell's assistant (Siva) types into the app in front and presses its keys
//! ("write hello in Text Editor", "new tab", "save it").
//!
//! Over the shell's private channel only (`ipc.rs`: `type <req> <id> <hex text>` and
//! `press <req> <id> <keys>`), not the public virtual-keyboard protocol, so no other client can
//! type into windows. kompact focuses the window, then feeds the keys to it through the seat's
//! keyboard (`KeyboardHandle::input`, past kompact's own shortcuts), a few each turn of the loop
//! (`STROKES_PER_TICK`), and stops as soon as the window loses the keyboard (you clicked
//! elsewhere), closes, or the screen locks. While you hold keys yourself it waits. Every job is
//! answered with `{"type":"typed","req":N,"ok":…,"missing":"…","why":"…"}`.
//!
//! Characters become keys through the keymap in use (its active layout, plain or with Shift,
//! Caps Lock allowed for); ones it can't type (not on the layout) are left out and named in
//! `missing`. Making the strokes is pure and tested here.

use std::collections::{HashMap, VecDeque};

use smithay::input::keyboard::{xkb, Keysym};

/// Characters one `type` may hold.
pub const MAX_TEXT: usize = 500;
/// Key combinations one `press` may hold ("ctrl+a, delete").
pub const MAX_COMBOS: usize = 8;
/// Key events sent each tick of the typing timer.
pub const STROKES_PER_TICK: usize = 24;
/// The typing timer.
pub const TICK_MS: u64 = 8;
/// How long a job may wait for the window to get the keyboard (a shell panel letting go), or
/// for you to let go of keys, in all.
pub const WAIT_MS: u64 = 2000;

/// A key going down or up (an xkb keycode: evdev + 8).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Stroke {
    pub code: u32,
    pub down: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mod {
    Ctrl,
    Shift,
    Alt,
    Super,
}

/// Where each keysym is on the layout: its key, and whether it needs Shift.
pub struct Keys {
    at: HashMap<u32, (u32, bool)>,
}

/// The keys the modifiers fall back to (evdev + 8) on a layout without them.
const SHIFT: u32 = 42 + 8;
const CTRL: u32 = 29 + 8;
const ALT: u32 = 56 + 8;
const SUPER: u32 = 125 + 8;

impl Keys {
    /// The keymap's `layout` (0-based), its first two levels (plain, Shift).
    pub fn of(keymap: &xkb::Keymap, layout: u32) -> Keys {
        let mut at: HashMap<u32, (u32, bool)> = HashMap::new();
        keymap.key_for_each(|map, code| {
            let levels = map.num_levels_for_key(code, layout).min(2);
            for level in 0..levels {
                if let [sym] = map.key_get_syms_by_level(code, layout, level) {
                    // The first key for a symbol wins (the main block before the keypad).
                    at.entry(sym.raw()).or_insert((code.raw(), level == 1));
                }
            }
        });
        Keys { at }
    }

    fn find(&self, sym: Keysym) -> Option<(u32, bool)> {
        self.at.get(&sym.raw()).copied()
    }

    fn modifier(&self, m: Mod) -> u32 {
        let (name, fallback) = match m {
            Mod::Shift => (xkb::keysyms::KEY_Shift_L, SHIFT),
            Mod::Ctrl => (xkb::keysyms::KEY_Control_L, CTRL),
            Mod::Alt => (xkb::keysyms::KEY_Alt_L, ALT),
            Mod::Super => (xkb::keysyms::KEY_Super_L, SUPER),
        };
        self.find(Keysym::new(name)).map(|(c, _)| c).unwrap_or(fallback)
    }

    /// The strokes typing `text` (with Caps Lock on or not), and the characters left out.
    pub fn text(&self, text: &str, caps_lock: bool) -> (Vec<Stroke>, String) {
        let shift = self.modifier(Mod::Shift);
        let mut out = Vec::new();
        let mut missing = String::new();
        for c in text.chars() {
            let sym = match c {
                '\n' => Keysym::new(xkb::keysyms::KEY_Return),
                '\t' => Keysym::new(xkb::keysyms::KEY_Tab),
                c => xkb::utf32_to_keysym(c as u32),
            };
            let Some((code, mut shifted)) = self.find(sym) else {
                if !missing.contains(c) {
                    missing.push(c);
                }
                continue;
            };
            // Caps Lock turns a letter's case round; Shift turns it back.
            if caps_lock && c.is_alphabetic() && c.to_lowercase().ne(c.to_uppercase()) {
                shifted = !shifted;
            }
            if shifted {
                out.push(Stroke { code: shift, down: true });
            }
            out.push(Stroke { code, down: true });
            out.push(Stroke { code, down: false });
            if shifted {
                out.push(Stroke { code: shift, down: false });
            }
        }
        (out, missing)
    }

    /// The strokes for key combinations, "ctrl+t" or "ctrl+a, delete" (`parse_combos`).
    pub fn combos(&self, keys: &str) -> Result<Vec<Stroke>, String> {
        let mut out = Vec::new();
        for (mods, sym) in parse_combos(keys)? {
            let (code, shifted) = self.find(sym).ok_or_else(|| format!("{} isn't on this keyboard layout", xkb::keysym_get_name(sym)))?;
            let mut held: Vec<u32> = mods.iter().map(|m| self.modifier(*m)).collect();
            let shift = self.modifier(Mod::Shift);
            if shifted && !held.contains(&shift) {
                held.push(shift);
            }
            out.extend(held.iter().map(|&code| Stroke { code, down: true }));
            out.push(Stroke { code, down: true });
            out.push(Stroke { code, down: false });
            out.extend(held.iter().rev().map(|&code| Stroke { code, down: false }));
        }
        Ok(out)
    }
}

/// A key's name as said or written ("enter", "page down", "f5", "t", "+") as a keysym.
fn key_sym(name: &str) -> Option<Keysym> {
    let n = name.trim().to_lowercase();
    let xkb_name = match n.as_str() {
        "enter" | "return" => "Return",
        "esc" | "escape" => "Escape",
        "tab" => "Tab",
        "space" | "spacebar" => "space",
        "backspace" => "BackSpace",
        "delete" | "del" => "Delete",
        "insert" | "ins" => "Insert",
        "home" => "Home",
        "end" => "End",
        "pageup" | "page up" | "pgup" => "Prior",
        "pagedown" | "page down" | "pgdn" => "Next",
        "up" => "Up",
        "down" => "Down",
        "left" => "Left",
        "right" => "Right",
        "plus" => "plus",
        "minus" => "minus",
        "equal" | "equals" => "equal",
        "comma" => "comma",
        "period" | "dot" => "period",
        "slash" => "slash",
        "print" | "printscreen" | "print screen" => "Print",
        _ => "",
    };
    if !xkb_name.is_empty() {
        return Some(xkb::keysym_from_name(xkb_name, xkb::KEYSYM_NO_FLAGS));
    }
    let mut chars = n.chars();
    if let (Some(c), None) = (chars.next(), chars.next()) {
        return (!c.is_control() && !c.is_whitespace()).then(|| xkb::utf32_to_keysym(c as u32));
    }
    // F1 to F24.
    if let Some(f) = n.strip_prefix('f').and_then(|d| d.parse::<u32>().ok()).filter(|f| (1..=24).contains(f)) {
        return Some(xkb::keysym_from_name(&format!("F{f}"), xkb::KEYSYM_NO_FLAGS));
    }
    None
}

/// "ctrl+shift+t, enter" → the modifiers and the key of each. A lone "+" is the plus key
/// ("ctrl++").
fn parse_combos(keys: &str) -> Result<Vec<(Vec<Mod>, Keysym)>, String> {
    let combos: Vec<&str> = keys.split(',').map(str::trim).filter(|c| !c.is_empty()).collect();
    if combos.is_empty() {
        return Err("no keys".into());
    }
    if combos.len() > MAX_COMBOS {
        return Err(format!("more than {MAX_COMBOS} key combinations"));
    }
    combos
        .iter()
        .map(|combo| {
            let (head, key) = match combo.strip_suffix("++") {
                Some(head) => (head, "+"),
                None => combo.rsplit_once('+').map(|(h, k)| (h, k)).unwrap_or(("", combo)),
            };
            let mut mods = Vec::new();
            for m in head.split('+').map(|m| m.trim().to_lowercase()).filter(|m| !m.is_empty()) {
                let m = match m.as_str() {
                    "ctrl" | "control" | "ctl" => Mod::Ctrl,
                    "shift" => Mod::Shift,
                    "alt" | "option" => Mod::Alt,
                    "super" | "win" | "windows" | "meta" | "logo" => Mod::Super,
                    // Apps on Linux take Ctrl where macOS takes Command.
                    "cmd" | "command" => Mod::Ctrl,
                    other => return Err(format!("{other:?} isn't a modifier")),
                };
                if !mods.contains(&m) {
                    mods.push(m);
                }
            }
            let sym = key_sym(key).ok_or_else(|| format!("{key:?} isn't a key I know"))?;
            Ok((mods, sym))
        })
        .collect()
}

/// `type`'s text, sent as hex so it can hold spaces and any character on one line.
pub fn from_hex(hex: &str) -> Option<String> {
    if hex.len() % 2 != 0 || hex.len() > MAX_TEXT * 8 {
        return None;
    }
    let bytes: Option<Vec<u8>> = (0..hex.len()).step_by(2).map(|i| u8::from_str_radix(hex.get(i..i + 2)?, 16).ok()).collect();
    let text = String::from_utf8(bytes?).ok()?;
    (!text.is_empty() && text.chars().count() <= MAX_TEXT).then_some(text)
}

/// One job: what's left to send to which window.
pub struct Job {
    pub req: u64,
    pub window: u64,
    pub strokes: VecDeque<Stroke>,
    pub missing: String,
    pub started: std::time::Instant,
    /// Some keys went to the window already.
    pub began: bool,
    /// Keys sent down and not yet up (let go if the job stops).
    pub held: Vec<u32>,
}

pub fn encode_typed(req: u64, ok: bool, missing: &str, why: &str) -> String {
    format!(
        "{{\"type\":\"typed\",\"req\":{req},\"ok\":{ok},\"missing\":{},\"why\":{}}}\n",
        crate::ipc::json_string(missing),
        crate::ipc::json_string(why)
    )
}

/// Milliseconds on the monotonic clock, as libinput stamps key events.
pub fn now_ms() -> u32 {
    // SAFETY: clock_gettime only writes the timespec we hand it.
    let mut ts: libc::timespec = unsafe { std::mem::zeroed() };
    unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut ts) };
    (ts.tv_sec as u64 * 1000 + ts.tv_nsec as u64 / 1_000_000) as u32
}

// ---- in kompact ----

/// What a job is to send.
pub enum What {
    Text(String),
    Keys(String),
}

impl crate::state::Kompact {
    fn typed(&mut self, req: u64, ok: bool, missing: &str, why: &str) {
        if ok {
            tracing::info!("typing: job {req} done{}", if missing.is_empty() { String::new() } else { format!(", left out {missing:?}") });
        } else {
            tracing::info!("typing: job {req} not done: {why}");
        }
        self.ipc.broadcast(&encode_typed(req, ok, missing, why));
    }

    /// 20e: start typing `what` into window `id` (the shell's `type` / `press`).
    pub fn start_typing(&mut self, req: u64, id: u64, what: What) {
        if self.typing.is_some() {
            return self.typed(req, false, "", "I'm still typing something else");
        }
        let Some(window) = self.window_by_id(id) else {
            return self.typed(req, false, "", "that window isn't there any more");
        };
        let Some(keyboard) = self.seat.get_keyboard() else {
            return self.typed(req, false, "", "there's no keyboard");
        };
        let keys = keyboard.with_xkb_state(self, |ctx| {
            let xkb = ctx.xkb().lock().unwrap_or_else(|e| e.into_inner());
            let layout = xkb.active_layout().0;
            // SAFETY: the keymap is used only inside this lock and not kept.
            Keys::of(unsafe { xkb.keymap() }, layout)
        });
        let (strokes, missing) = match what {
            What::Text(text) => keys.text(&text, keyboard.modifier_state().caps_lock),
            What::Keys(combos) => match keys.combos(&combos) {
                Ok(s) => (s, String::new()),
                Err(e) => return self.typed(req, false, "", &e),
            },
        };
        if strokes.is_empty() {
            return self.typed(req, false, &missing, "none of it is on this keyboard layout");
        }
        tracing::info!("typing: job {req}: {} key events for {:?}", strokes.len(), crate::shell::app_id(&window));
        self.activate_window(&window);
        self.typing = Some(Job { req, window: id, strokes: strokes.into(), missing, started: std::time::Instant::now(), began: false, held: Vec::new() });
        use smithay::reexports::calloop::timer::Timer;
        let res = self.loop_handle.insert_source(Timer::immediate(), |_, _, state: &mut Self| state.typing_tick());
        if let Err(e) = res {
            tracing::warn!("typing: no timer: {e}");
            let job = self.typing.take();
            if let Some(j) = job {
                self.typed(j.req, false, "", "kompact couldn't start typing");
            }
        }
    }

    /// Stop the job: let go of its keys, and say why.
    fn stop_typing(&mut self, why: &str) {
        let Some(job) = self.typing.take() else { return };
        if let Some(keyboard) = self.seat.get_keyboard() {
            for code in job.held.iter().rev() {
                let serial = smithay::utils::SERIAL_COUNTER.next_serial();
                keyboard.input::<(), _>(self, (*code).into(), smithay::backend::input::KeyState::Released, serial, now_ms(), |_, _, _| smithay::input::keyboard::FilterResult::Forward);
            }
        }
        self.typed(job.req, false, &job.missing, why);
    }

    /// One tick of the typing timer: a few keys to the window, if it still has the keyboard.
    fn typing_tick(&mut self) -> smithay::reexports::calloop::timer::TimeoutAction {
        use smithay::reexports::calloop::timer::TimeoutAction;
        use smithay::backend::input::KeyState;
        let wait = TimeoutAction::ToDuration(std::time::Duration::from_millis(30));
        let Some(job) = self.typing.as_ref() else { return TimeoutAction::Drop };
        let (window_id, began, waited) = (job.window, job.began, job.started.elapsed().as_millis() as u64);
        if self.lock.is_locked() {
            self.stop_typing("the screen locked");
            return TimeoutAction::Drop;
        }
        let Some(window) = self.window_by_id(window_id) else {
            self.stop_typing("the window closed");
            return TimeoutAction::Drop;
        };
        let Some(keyboard) = self.seat.get_keyboard() else {
            self.stop_typing("there's no keyboard");
            return TimeoutAction::Drop;
        };
        let target = window.toplevel().map(|t| t.wl_surface().clone());
        if keyboard.current_focus() != target {
            if began {
                // You went to another window: never type into the wrong one.
                self.stop_typing("you moved to another window, so I stopped");
                return TimeoutAction::Drop;
            }
            if waited > WAIT_MS {
                self.stop_typing("the window didn't get the keyboard");
                return TimeoutAction::Drop;
            }
            // A shell panel (the HUD you typed into) may still hold it for a moment.
            self.activate_window(&window);
            return wait;
        }
        // Your own keys first: none of ours mixed with yours.
        if !self.pressed_keys.is_empty() {
            if waited > WAIT_MS * 5 {
                self.stop_typing("keys were held down");
                return TimeoutAction::Drop;
            }
            return wait;
        }
        // A tick never ends with our keys down (your next key would come with our Shift).
        let mut sent = 0;
        while sent < STROKES_PER_TICK || self.typing.as_ref().is_some_and(|j| !j.held.is_empty()) {
            let Some(stroke) = self.typing.as_mut().and_then(|j| j.strokes.pop_front()) else { break };
            sent += 1;
            let serial = smithay::utils::SERIAL_COUNTER.next_serial();
            let state = if stroke.down { KeyState::Pressed } else { KeyState::Released };
            keyboard.input::<(), _>(self, stroke.code.into(), state, serial, now_ms(), |_, _, _| smithay::input::keyboard::FilterResult::Forward);
            if let Some(j) = self.typing.as_mut() {
                j.began = true;
                if stroke.down {
                    j.held.push(stroke.code);
                } else {
                    j.held.retain(|c| *c != stroke.code);
                }
            }
        }
        if self.typing.as_ref().is_some_and(|j| j.strokes.is_empty()) {
            let job = self.typing.take().expect("checked");
            self.typed(job.req, true, &job.missing, "");
            return TimeoutAction::Drop;
        }
        TimeoutAction::ToDuration(std::time::Duration::from_millis(TICK_MS))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn us() -> Keys {
        let ctx = xkb::Context::new(xkb::CONTEXT_NO_FLAGS);
        let keymap = xkb::Keymap::new_from_names(&ctx, "", "pc105", "us", "", None, xkb::KEYMAP_COMPILE_NO_FLAGS).expect("the us layout (xkeyboard-config)");
        Keys::of(&keymap, 0)
    }

    const A: u32 = 30 + 8;
    const T: u32 = 20 + 8;
    const ONE: u32 = 2 + 8;
    const ENTER: u32 = 28 + 8;

    fn d(code: u32) -> Stroke {
        Stroke { code, down: true }
    }
    fn u(code: u32) -> Stroke {
        Stroke { code, down: false }
    }

    #[test]
    fn typing_text() {
        let k = us();
        assert_eq!(k.text("a", false), (vec![d(A), u(A)], String::new()));
        assert_eq!(k.text("A", false), (vec![d(SHIFT), d(A), u(A), u(SHIFT)], String::new()));
        assert_eq!(k.text("!", false).0, vec![d(SHIFT), d(ONE), u(ONE), u(SHIFT)], "! is Shift+1");
        assert_eq!(k.text("A", true), (vec![d(A), u(A)], String::new()), "Caps Lock gives the capital");
        assert_eq!(k.text("a", true).0, vec![d(SHIFT), d(A), u(A), u(SHIFT)]);
        assert_eq!(k.text("1", true).0, vec![d(ONE), u(ONE)], "Caps Lock leaves digits alone");
        assert_eq!(k.text("\n", false).0, vec![d(ENTER), u(ENTER)]);
        let (strokes, missing) = k.text("café ☕", false);
        assert_eq!(missing, "é☕", "not on the us layout");
        assert_eq!(strokes.len(), 4 * 2, "c, a, f and the space, down and up");
        assert_eq!(k.text("Hello, world", false).0.iter().filter(|s| s.down).count(), 12 + 1, "one Shift for the H");
    }

    #[test]
    fn pressing_keys() {
        let k = us();
        assert_eq!(k.combos("ctrl+t"), Ok(vec![d(CTRL), d(T), u(T), u(CTRL)]));
        assert_eq!(k.combos("Ctrl + Shift + T"), Ok(vec![d(CTRL), d(SHIFT), d(T), u(T), u(SHIFT), u(CTRL)]));
        assert_eq!(k.combos("enter"), Ok(vec![d(ENTER), u(ENTER)]));
        assert_eq!(k.combos("ctrl+a, delete").map(|s| s.len()), Ok(4 + 2));
        assert_eq!(k.combos("cmd+s").map(|s| s[0]), Ok(d(CTRL)), "Command is Ctrl here");
        // The plus key is Shift+= on a us keyboard.
        assert_eq!(k.combos("ctrl++").map(|s| s.len()), Ok(6));
        assert_eq!(k.combos("ctrl+plus").map(|s| s.len()), Ok(6));
        assert!(k.combos("f5").is_ok() && k.combos("page down").is_ok() && k.combos("alt+left").is_ok());
        assert!(k.combos("hyper+t").is_err());
        assert!(k.combos("ctrl+banana").is_err());
        assert!(k.combos("").is_err());
        assert!(k.combos(&["a"; 9].join(",")).is_err(), "at most 8");
    }

    #[test]
    fn text_on_the_channel() {
        assert_eq!(from_hex("68692021"), Some("hi !".into()));
        assert_eq!(from_hex("c3a9"), Some("é".into()));
        assert_eq!(from_hex(""), None);
        assert_eq!(from_hex("6"), None);
        assert_eq!(from_hex("zz"), None);
        assert_eq!(from_hex("ff"), None, "not UTF-8");
        assert_eq!(from_hex(&"61".repeat(MAX_TEXT + 1)), None);
        assert_eq!(encode_typed(3, false, "é", "it \"closed\""), "{\"type\":\"typed\",\"req\":3,\"ok\":false,\"missing\":\"é\",\"why\":\"it \\\"closed\\\"\"}\n", "one line, ended");
    }
}
