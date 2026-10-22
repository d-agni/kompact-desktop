//! 16a/16b: the window switcher (Alt+Tab / Super+Tab), in most-recently-used order.
//!
//! The first press lists this workspace's windows: the focused one, the others on screen top to
//! bottom (focusing raises, so the stack is the use order), then the minimized ones, most
//! recently minimized first. The selection starts on the second (the window used before). While
//! the shortcut's modifiers stay down, each further press moves it (Shift: back), as do the
//! arrow keys; letting go of the modifiers switches to the selected window, raising only it, so
//! A, B, C with A in front becomes B, A, C. Esc cancels.
//!
//! 16b: the shell shows the list (thumbnails) once the switch has been held for `SHOW_AFTER`,
//! so a quick tap doesn't flash it. 16c: the dock's switcher button opens a session with no keys
//! held (`Hold::Sticky`), which ends on a click, Enter or Esc.
//!
//! Generic over the window type, so `cargo test` covers it with plain values; `shell.rs` drives
//! it with real `Window`s.

use std::time::{Duration, Instant};

use crate::keymap::Mods;

/// How long the switch is held before the shell shows the list.
pub const SHOW_AFTER: Duration = Duration::from_millis(150);

/// Keysyms the session handles.
const TAB: u32 = 0xff09;
const ISO_LEFT_TAB: u32 = 0xfe20;
const ESCAPE: u32 = 0xff1b;
const RETURN: u32 = 0xff0d;
const KP_ENTER: u32 = 0xff8d;
const SPACE: u32 = 0x20;
const LEFT: u32 = 0xff51;
const UP: u32 = 0xff52;
const RIGHT: u32 = 0xff53;
const DOWN: u32 = 0xff54;

/// The windows in the order the switcher shows them.
pub fn order<W: PartialEq + Clone>(top_to_bottom: &[W], minimized_oldest_first: &[W], current: Option<&W>) -> Vec<W> {
    let mut out: Vec<W> = Vec::new();
    if let Some(c) = current.filter(|c| top_to_bottom.contains(c)) {
        out.push(c.clone());
    }
    out.extend(top_to_bottom.iter().filter(|w| Some(*w) != current).cloned());
    out.extend(minimized_oldest_first.iter().rev().cloned());
    out
}

/// What keeps the session open.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hold {
    /// The shortcut's modifiers (without Shift): letting go of all of them switches.
    Keys { ctrl: bool, alt: bool, logo: bool },
    /// 16c: opened by a click; only a choice or Esc ends it.
    Sticky,
}

impl Hold {
    /// The modifiers of a shortcut press; `None` if it has none to hold (it then switches at once).
    pub fn of(mods: Mods) -> Option<Hold> {
        (mods.ctrl || mods.alt || mods.logo).then_some(Hold::Keys { ctrl: mods.ctrl, alt: mods.alt, logo: mods.logo })
    }

    /// All the held modifiers are up.
    fn released(self, mods: Mods) -> bool {
        match self {
            Hold::Keys { ctrl, alt, logo } => !(ctrl && mods.ctrl || alt && mods.alt || logo && mods.logo),
            Hold::Sticky => false,
        }
    }
}

/// What a key press does to an open session.
#[derive(Debug, PartialEq, Eq)]
pub enum KeyOutcome {
    /// The selection moved.
    Moved,
    /// Switch to the selected window now.
    Commit,
    Cancel,
    /// Not the session's: kept from the app while the session is open (modifiers aren't).
    Ignored,
}

pub struct Session<W> {
    list: Vec<W>,
    selected: usize,
    hold: Hold,
    /// The shortcut's key (its unshifted keysym), which steps forward; none when sticky.
    key: Option<u32>,
    started: Instant,
    /// The shell has been told (and must be told when it ends).
    pub shown: bool,
}

impl<W: PartialEq + Clone> Session<W> {
    /// A session over `list` (from `order`); `has_current`: its first entry is the window in use,
    /// so the selection starts on the next one. `None` if there's nothing to switch to.
    pub fn new(list: Vec<W>, has_current: bool, hold: Hold, key: Option<u32>, now: Instant) -> Option<Session<W>> {
        if list.is_empty() {
            return None;
        }
        let selected = if has_current && list.len() > 1 { 1 } else { 0 };
        Some(Session { list, selected, hold, key, started: now, shown: false })
    }

    pub fn list(&self) -> &[W] {
        &self.list
    }

    pub fn selected_index(&self) -> usize {
        self.selected
    }

    pub fn selected(&self) -> &W {
        &self.list[self.selected]
    }

    pub fn is_sticky(&self) -> bool {
        self.hold == Hold::Sticky
    }

    /// Move the selection by `by`, wrapping round.
    pub fn step(&mut self, by: i32) {
        let n = self.list.len() as i32;
        self.selected = (self.selected as i32 + by).rem_euclid(n) as usize;
    }

    /// Select `w` (the pointer is on its card). False if it isn't listed or already selected.
    pub fn select(&mut self, w: &W) -> bool {
        match self.list.iter().position(|x| x == w) {
            Some(i) if i != self.selected => {
                self.selected = i;
                true
            }
            _ => false,
        }
    }

    /// Whether the shell should be shown the list now (held long enough, not yet told).
    pub fn due(&self, now: Instant) -> bool {
        !self.shown && (self.is_sticky() || now.duration_since(self.started) >= SHOW_AFTER)
    }

    /// The modifiers are now `mods` (after a key went up or down): true if the switch is done.
    pub fn released(&self, mods: Mods) -> bool {
        self.hold.released(mods)
    }

    /// A key went down while the session is open (`sym` its unshifted keysym).
    pub fn key(&mut self, sym: u32, mods: Mods) -> KeyOutcome {
        match sym {
            ESCAPE => KeyOutcome::Cancel,
            RETURN | KP_ENTER | SPACE => KeyOutcome::Commit,
            LEFT | UP | ISO_LEFT_TAB => {
                self.step(-1);
                KeyOutcome::Moved
            }
            RIGHT | DOWN => {
                self.step(1);
                KeyOutcome::Moved
            }
            s if Some(s) == self.key || s == TAB => {
                self.step(if mods.shift { -1 } else { 1 });
                KeyOutcome::Moved
            }
            _ => KeyOutcome::Ignored,
        }
    }

    /// Drop windows that are gone (`keep` false). True if the list changed; the selection stays
    /// on the same window, or the one after it if that one went.
    pub fn retain(&mut self, mut keep: impl FnMut(&W) -> bool) -> bool {
        let before = self.list.len();
        let mut i = 0;
        let mut selected = self.selected;
        self.list.retain(|w| {
            let k = keep(w);
            if !k && i < self.selected {
                selected -= 1;
            }
            i += 1;
            k
        });
        self.selected = if self.list.is_empty() { 0 } else { selected.min(self.list.len() - 1) };
        self.list.len() != before
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALT: Mods = Mods { ctrl: false, alt: true, shift: false, logo: false };
    const SHIFT_ALT: Mods = Mods { ctrl: false, alt: true, shift: true, logo: false };
    const NONE: Mods = Mods { ctrl: false, alt: false, shift: false, logo: false };

    fn session(list: &[&'static str], hold: Hold) -> Session<&'static str> {
        Session::new(list.to_vec(), true, hold, Some(TAB), Instant::now()).unwrap()
    }

    #[test]
    fn most_recently_used_order() {
        // A in front, then B, then C; D minimized before E.
        assert_eq!(order(&["a", "b", "c"], &["d", "e"], Some(&"a")), ["a", "b", "c", "e", "d"]);
        // Focus without raising (focus follows mouse): the focused one still comes first.
        assert_eq!(order(&["a", "b", "c"], &[], Some(&"b")), ["b", "a", "c"]);
        // Nothing focused, or the focus elsewhere (a minimized window can't have it).
        assert_eq!(order(&["a", "b"], &["d"], None), ["a", "b", "d"]);
        assert_eq!(order(&["a"], &["d"], Some(&"d")), ["a", "d"]);
        assert!(order::<&str>(&[], &[], None).is_empty());
    }

    #[test]
    fn tap_picks_the_previous_window() {
        // The user's example: A, B, C with A active: one Alt+Tab selects B, and B, A, C follows
        // from raising only B.
        let s = session(&["a", "b", "c"], Hold::of(ALT).unwrap());
        assert_eq!(*s.selected(), "b");
        // Alone, or with nothing in use, it's the first.
        assert_eq!(*session(&["a"], Hold::Sticky).selected(), "a");
        let none = Session::new(vec!["a", "b"], false, Hold::Sticky, None, Instant::now()).unwrap();
        assert_eq!(*none.selected(), "a");
        assert!(Session::<&str>::new(vec![], true, Hold::Sticky, None, Instant::now()).is_none());
    }

    #[test]
    fn minimized_windows_come_back() {
        // 10c's cases: on screen a, b; minimized c then d. From b the next is d (the last
        // minimized), then c.
        let mut s = Session::new(order(&["b", "a"], &["c", "d"], Some(&"b")), true, Hold::Sticky, Some(TAB), Instant::now()).unwrap();
        assert_eq!(*s.selected(), "a");
        s.step(1);
        assert_eq!(*s.selected(), "d");
        s.step(1);
        assert_eq!(*s.selected(), "c");
        // Everything minimized (Show desktop): the last minimized comes back first.
        let all = Session::new(order::<&str>(&[], &["c", "d"], None), false, Hold::Sticky, None, Instant::now()).unwrap();
        assert_eq!(*all.selected(), "d");
        // A single window minimized, nothing else: Alt+Tab brings it back.
        let one = Session::new(order::<&str>(&[], &["c"], None), false, Hold::Sticky, None, Instant::now()).unwrap();
        assert_eq!(*one.selected(), "c");
    }

    #[test]
    fn keys_move_and_end_it() {
        let mut s = session(&["a", "b", "c"], Hold::of(ALT).unwrap());
        assert_eq!(s.key(TAB, ALT), KeyOutcome::Moved);
        assert_eq!(*s.selected(), "c");
        assert_eq!(s.key(TAB, ALT), KeyOutcome::Moved);
        assert_eq!(*s.selected(), "a", "wraps round");
        assert_eq!(s.key(TAB, SHIFT_ALT), KeyOutcome::Moved);
        assert_eq!(*s.selected(), "c", "Shift goes back");
        assert_eq!(s.key(ISO_LEFT_TAB, SHIFT_ALT), KeyOutcome::Moved);
        assert_eq!(s.key(LEFT, ALT), KeyOutcome::Moved);
        assert_eq!(*s.selected(), "a");
        assert_eq!(s.key(RIGHT, ALT), KeyOutcome::Moved);
        assert_eq!(*s.selected(), "b");
        assert_eq!(s.key(0x71, ALT), KeyOutcome::Ignored);
        assert_eq!(s.key(RETURN, ALT), KeyOutcome::Commit);
        assert_eq!(s.key(ESCAPE, ALT), KeyOutcome::Cancel);
        // Another key as the shortcut (Super+grave, say) steps too.
        let mut g = Session::new(vec![1, 2, 3], true, Hold::Sticky, Some(0x60), Instant::now()).unwrap();
        assert_eq!(g.key(0x60, NONE), KeyOutcome::Moved);
        assert_eq!(*g.selected(), 3);
    }

    #[test]
    fn letting_go_switches() {
        let alt = session(&["a", "b"], Hold::of(ALT).unwrap());
        assert!(!alt.released(ALT));
        assert!(!alt.released(SHIFT_ALT));
        assert!(alt.released(Mods { shift: true, ..NONE }), "Shift doesn't hold it");
        assert!(alt.released(NONE));
        let ctrl_super = Hold::of(Mods { ctrl: true, logo: true, ..NONE }).unwrap();
        let s = session(&["a", "b"], ctrl_super);
        assert!(!s.released(Mods { logo: true, ..NONE }), "one of them still down");
        assert!(s.released(NONE));
        assert!(!session(&["a", "b"], Hold::Sticky).released(NONE));
        assert_eq!(Hold::of(Mods { shift: true, ..NONE }), None, "nothing to hold");
    }

    #[test]
    fn shown_after_a_moment() {
        let t0 = Instant::now();
        let mut s = Session::new(vec!["a", "b"], true, Hold::of(ALT).unwrap(), Some(TAB), t0).unwrap();
        assert!(!s.due(t0));
        assert!(!s.due(t0 + Duration::from_millis(100)));
        assert!(s.due(t0 + SHOW_AFTER));
        s.shown = true;
        assert!(!s.due(t0 + SHOW_AFTER));
        let sticky = Session::new(vec!["a"], false, Hold::Sticky, None, t0).unwrap();
        assert!(sticky.due(t0), "a click shows it at once");
    }

    #[test]
    fn pointer_selects() {
        let mut s = session(&["a", "b", "c"], Hold::Sticky);
        assert!(s.select(&"c"));
        assert!(!s.select(&"c"), "already");
        assert!(!s.select(&"x"));
        assert_eq!(s.selected_index(), 2);
    }

    #[test]
    fn closed_windows_leave() {
        let mut s = session(&["a", "b", "c", "d"], Hold::Sticky);
        s.step(1); // c
        assert!(s.retain(|w| *w != "a"));
        assert_eq!((s.list(), *s.selected()), (&["b", "c", "d"][..], "c"));
        assert!(s.retain(|w| *w != "c"));
        assert_eq!(*s.selected(), "d", "the next one");
        assert!(s.retain(|w| *w != "d"));
        assert_eq!(*s.selected(), "b", "the last went: the one before");
        assert!(!s.retain(|_| true));
        assert!(s.retain(|_| false));
        assert!(s.list().is_empty());
    }
}
