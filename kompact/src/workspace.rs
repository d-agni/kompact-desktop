//! Workspaces / virtual desktops (Phase 2e).
//!
//! Nine workspaces. Windows on the active workspace live in the `Space` as usual; windows on the
//! others are taken out of the `Space` (so they're neither drawn nor sent frame callbacks) and
//! parked here with their position and stacking order. Switching parks the current set and maps
//! the target set back in the same order. Layer-shell surfaces (panels) belong to the output,
//! not to a workspace, so they stay put.
//!
//! Minimized windows (6m-0) are parked the same way, in their own list that remembers which
//! workspace they were minimized on; they stay hidden across switches until restored.
//!
//! The bookkeeping is generic over the window type so it's covered by `cargo test` with plain
//! integers; `shell.rs` drives it with real `Window`s.

use smithay::utils::{Logical, Point};

pub const COUNT: usize = 9;

/// Keys 1..9 on the number row, as xkb keycodes (evdev KEY_1 = 2, KEY_9 = 10, plus 8).
/// Using the keycode keeps `Super+Shift+N` working: with Shift held the keysym becomes `!`, `@`,
/// ... (and differs per layout), but the physical key stays the same.
pub fn workspace_for_keycode(xkb_keycode: u32) -> Option<usize> {
    (10..=18).contains(&xkb_keycode).then(|| (xkb_keycode - 10) as usize)
}

type Loc = Point<i32, Logical>;

/// 10c: what "next window" (Super+Tab, Alt+Tab) picks: after the focused one, the windows on
/// screen in stacking order, then this workspace's minimized ones, most recently minimized
/// first (picking one brings it back, as Alt+Tab does on Windows), wrapping round. With nothing
/// focused: the first on screen, or (all minimized, e.g. after Show desktop) the last minimized.
pub fn next_in_cycle<W: PartialEq + Clone>(visible: &[W], minimized_oldest_first: &[W], current: Option<&W>) -> Option<W> {
    let cycle: Vec<&W> = visible.iter().chain(minimized_oldest_first.iter().rev()).collect();
    if cycle.is_empty() {
        return None;
    }
    let next = match current.and_then(|c| cycle.iter().position(|w| *w == c)) {
        Some(i) => cycle[(i + 1) % cycle.len()],
        None => cycle[0],
    };
    Some(next.clone())
}

/// What the caller has to do to carry out a switch.
#[derive(Debug, PartialEq)]
pub struct Switch<W> {
    /// Map these into the space, in this order (bottom-to-top), at these locations.
    pub show: Vec<(W, Loc)>,
    /// Then focus this (the window last focused there, or its top-most), or clear focus.
    pub focus: Option<W>,
}

pub struct Workspaces<W> {
    active: usize,
    /// Parked windows per workspace, bottom-to-top. The active workspace's entry is empty: its
    /// windows are in the `Space`.
    parked: Vec<Vec<(W, Loc)>>,
    /// Window that had focus when each workspace was last left.
    last_focus: Vec<Option<W>>,
    /// Minimized windows, oldest first, with where they were and their workspace.
    minimized: Vec<(W, Loc, usize)>,
}

impl<W: Clone + PartialEq> Default for Workspaces<W> {
    fn default() -> Self {
        Self {
            active: 0,
            parked: (0..COUNT).map(|_| Vec::new()).collect(),
            last_focus: vec![None; COUNT],
            minimized: Vec::new(),
        }
    }
}

impl<W: Clone + PartialEq> Workspaces<W> {
    /// Active workspace, 0-based (shown to the user as 1..9).
    pub fn active(&self) -> usize {
        self.active
    }

    /// Switch to workspace `to`. `visible` is what's on screen now (bottom-to-top, with
    /// locations) and `focused` the window with keyboard focus; the caller unmaps all of
    /// `visible` and then applies the returned `Switch`. `None` = nothing to do (same workspace
    /// or out of range), and nothing was changed.
    pub fn switch(&mut self, to: usize, visible: Vec<(W, Loc)>, focused: Option<W>) -> Option<Switch<W>> {
        if to >= COUNT || to == self.active {
            return None;
        }
        self.last_focus[self.active] = focused;
        self.parked[self.active] = visible;
        self.active = to;
        let show = std::mem::take(&mut self.parked[to]);
        let remembered = self.last_focus[to].take();
        let focus = remembered
            .filter(|w| show.iter().any(|(s, _)| s == w))
            .or_else(|| show.last().map(|(w, _)| w.clone()));
        Some(Switch { show, focus })
    }

    /// Park `window` (currently on the active workspace, at `loc`) on workspace `to`, on top of
    /// that workspace's stack. Returns false (nothing changed) for the active workspace or an
    /// out-of-range one; the caller unmaps it from the space only on true.
    pub fn send(&mut self, window: W, loc: Loc, to: usize) -> bool {
        if to >= COUNT || to == self.active {
            return false;
        }
        self.parked[to].push((window.clone(), loc));
        // Arriving there, it's what you'd expect to have focus when you follow it.
        self.last_focus[to] = Some(window);
        true
    }

    /// Minimize `window` (on the active workspace, at `loc`). False if it already is.
    pub fn minimize(&mut self, window: W, loc: Loc) -> bool {
        if self.is_minimized(&window) {
            return false;
        }
        let ws = self.active;
        if self.last_focus[ws].as_ref() == Some(&window) {
            self.last_focus[ws] = None;
        }
        self.minimized.push((window, loc, ws));
        true
    }

    pub fn is_minimized(&self, window: &W) -> bool {
        self.minimized.iter().any(|(w, _, _)| w == window)
    }

    /// Take the most recently minimized window of the active workspace back, with where it was.
    pub fn restore_last(&mut self) -> Option<(W, Loc)> {
        let pos = self.minimized.iter().rposition(|(_, _, ws)| *ws == self.active)?;
        let (w, loc, _) = self.minimized.remove(pos);
        Some((w, loc))
    }

    /// Take a specific minimized window back (6m-c `activate`), with where it was.
    pub fn restore(&mut self, window: &W) -> Option<Loc> {
        let pos = self.minimized.iter().position(|(w, _, _)| w == window)?;
        Some(self.minimized.remove(pos).1)
    }

    /// 10c: the active workspace's minimized windows, oldest first.
    pub fn minimized_here(&self) -> Vec<W> {
        self.minimized.iter().filter(|(_, _, ws)| *ws == self.active).map(|(w, _, _)| w.clone()).collect()
    }

    /// Minimized windows on the active workspace, and on all of them.
    pub fn minimized_counts(&self) -> (usize, usize) {
        (self.minimized.iter().filter(|(_, _, ws)| *ws == self.active).count(), self.minimized.len())
    }

    /// Workspace and location of a parked or minimized window (None if it's in the space or
    /// unknown).
    pub fn find(&self, window: &W) -> Option<(usize, Loc)> {
        self.parked
            .iter()
            .enumerate()
            .find_map(|(i, stack)| stack.iter().find(|(w, _)| w == window).map(|(_, loc)| (i, *loc)))
            .or_else(|| self.minimized.iter().find(|(w, _, _)| w == window).map(|(_, loc, ws)| (*ws, *loc)))
    }

    /// Move a parked or minimized window without showing it (e.g. it asked to be maximized
    /// while hidden).
    pub fn set_location(&mut self, window: &W, loc: Loc) -> bool {
        for stack in &mut self.parked {
            if let Some(entry) = stack.iter_mut().find(|(w, _)| w == window) {
                entry.1 = loc;
                return true;
            }
        }
        if let Some(entry) = self.minimized.iter_mut().find(|(w, _, _)| w == window) {
            entry.1 = loc;
            return true;
        }
        false
    }

    /// Forget a parked or minimized window (it closed). Returns where it was.
    pub fn remove(&mut self, window: &W) -> Option<(usize, Loc)> {
        if let Some(pos) = self.minimized.iter().position(|(w, _, _)| w == window) {
            let (_, loc, ws) = self.minimized.remove(pos);
            return Some((ws, loc));
        }
        for (i, stack) in self.parked.iter_mut().enumerate() {
            if let Some(pos) = stack.iter().position(|(w, _)| w == window) {
                let (_, loc) = stack.remove(pos);
                if self.last_focus[i].as_ref() == Some(window) {
                    self.last_focus[i] = None;
                }
                return Some((i, loc));
            }
        }
        None
    }

    /// Every window not in the space: parked on any workspace, or minimized.
    pub fn parked(&self) -> impl Iterator<Item = &W> {
        self.parked.iter().flatten().map(|(w, _)| w).chain(self.minimized.iter().map(|(w, _, _)| w))
    }

    /// Number of windows parked on each workspace (the active one shows 0 here).
    pub fn parked_counts(&self) -> Vec<usize> {
        self.parked.iter().map(Vec::len).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(x: i32, y: i32) -> Loc {
        Point::from((x, y))
    }

    #[test]
    fn next_window_includes_minimized() {
        // On screen a, b; minimized c then d (d most recently).
        let (vis, min) = (["a", "b"], ["c", "d"]);
        assert_eq!(next_in_cycle(&vis, &min, Some(&"a")), Some("b"));
        assert_eq!(next_in_cycle(&vis, &min, Some(&"b")), Some("d"));
        assert_eq!(next_in_cycle(&vis, &min, Some(&"d")), Some("c"));
        assert_eq!(next_in_cycle(&vis, &min, Some(&"c")), Some("a"));
        assert_eq!(next_in_cycle(&vis, &min, None), Some("a"));
        // Everything minimized (Show desktop): the last minimized comes back first.
        assert_eq!(next_in_cycle::<&str>(&[], &min, None), Some("d"));
        // A single window minimized, nothing else: Alt+Tab brings it back.
        assert_eq!(next_in_cycle::<&str>(&[], &["c"], None), Some("c"));
        assert_eq!(next_in_cycle::<&str>(&[], &[], None), None);
        // As before without minimized windows.
        assert_eq!(next_in_cycle(&vis, &[], Some(&"b")), Some("a"));
    }

    #[test]
    fn number_row_keycodes() {
        assert_eq!(workspace_for_keycode(10), Some(0)); // key 1
        assert_eq!(workspace_for_keycode(18), Some(8)); // key 9
        assert_eq!(workspace_for_keycode(19), None); // key 0
        assert_eq!(workspace_for_keycode(9), None); // Escape
    }

    #[test]
    fn switching_parks_and_restores_in_order() {
        let mut ws = Workspaces::default();
        let s = ws.switch(1, vec![(1, p(0, 0)), (2, p(10, 10))], Some(2)).unwrap();
        assert_eq!(s, Switch { show: vec![], focus: None });
        assert_eq!(ws.active(), 1);

        // Workspace 2 gets window 3, then we go back to 1.
        let s = ws.switch(0, vec![(3, p(50, 50))], Some(3)).unwrap();
        assert_eq!(s.show, vec![(1, p(0, 0)), (2, p(10, 10))], "same order and spots as before");
        assert_eq!(s.focus, Some(2), "focus comes back to what had it");

        let s = ws.switch(1, s.show, Some(1)).unwrap();
        assert_eq!(s.show, vec![(3, p(50, 50))]);
        assert_eq!(s.focus, Some(3));
    }

    #[test]
    fn switching_to_current_or_invalid_changes_nothing() {
        let mut ws: Workspaces<u32> = Workspaces::default();
        assert_eq!(ws.switch(0, vec![(1, p(0, 0))], Some(1)), None);
        assert_eq!(ws.switch(COUNT, vec![(1, p(0, 0))], Some(1)), None);
        assert_eq!(ws.active(), 0);
        assert_eq!(ws.find(&1), None, "nothing was parked by a refused switch");
    }

    #[test]
    fn focus_falls_back_to_top_most_when_last_focused_is_gone() {
        let mut ws = Workspaces::default();
        ws.switch(1, vec![(1, p(0, 0)), (2, p(0, 0)), (3, p(0, 0))], Some(2));
        ws.remove(&2); // closed while hidden
        let s = ws.switch(0, vec![], None).unwrap();
        assert_eq!(s.focus, Some(3));
    }

    #[test]
    fn empty_workspace_clears_focus() {
        let mut ws = Workspaces::default();
        let s = ws.switch(4, vec![(1, p(0, 0))], Some(1)).unwrap();
        assert_eq!(s.focus, None);
    }

    #[test]
    fn send_window_to_other_workspace() {
        let mut ws = Workspaces::default();
        assert!(ws.send(7, p(100, 200), 3));
        assert_eq!(ws.find(&7), Some((3, p(100, 200))));
        assert!(!ws.send(8, p(0, 0), 0), "can't send to the active workspace");
        assert!(!ws.send(8, p(0, 0), COUNT));
        // Following it there shows it, focused, on top.
        let s = ws.switch(3, vec![(1, p(0, 0))], Some(1)).unwrap();
        assert_eq!(s.show, vec![(7, p(100, 200))]);
        assert_eq!(s.focus, Some(7));
    }

    #[test]
    fn sent_window_goes_on_top_of_existing_ones() {
        let mut ws = Workspaces::default();
        ws.switch(2, vec![], None); // go to workspace 3...
        ws.switch(0, vec![(1, p(0, 0))], Some(1)); // ...leave window 1 there, back to 1
        ws.send(5, p(9, 9), 2);
        let s = ws.switch(2, vec![], None).unwrap();
        assert_eq!(s.show, vec![(1, p(0, 0)), (5, p(9, 9))]);
    }

    #[test]
    fn closing_a_parked_window() {
        let mut ws = Workspaces::default();
        ws.switch(1, vec![(1, p(1, 1)), (2, p(2, 2))], Some(1));
        assert_eq!(ws.remove(&1), Some((0, p(1, 1))));
        assert_eq!(ws.remove(&1), None);
        assert_eq!(ws.parked().copied().collect::<Vec<_>>(), vec![2]);
        let s = ws.switch(0, vec![], None).unwrap();
        assert_eq!(s.focus, Some(2), "closed window's focus memory was dropped");
    }

    #[test]
    fn parked_window_can_move_while_hidden() {
        let mut ws = Workspaces::default();
        ws.switch(1, vec![(1, p(1, 1))], None);
        assert!(ws.set_location(&1, p(0, 32)));
        assert!(!ws.set_location(&9, p(0, 0)));
        assert_eq!(ws.find(&1), Some((0, p(0, 32))));
    }

    #[test]
    fn counts_per_workspace() {
        let mut ws = Workspaces::default();
        ws.send(1, p(0, 0), 2);
        ws.send(2, p(0, 0), 2);
        ws.send(3, p(0, 0), 8);
        assert_eq!(ws.parked_counts(), vec![0, 0, 2, 0, 0, 0, 0, 0, 1]);
    }

    #[test]
    fn minimize_and_restore() {
        let mut ws: Workspaces<u32> = Workspaces::default();
        assert!(ws.minimize(1, p(10, 10)));
        assert!(ws.minimize(2, p(20, 20)));
        assert!(!ws.minimize(2, p(0, 0)), "already minimized");
        assert_eq!(ws.find(&1), Some((0, p(10, 10))));
        assert_eq!(ws.parked().copied().collect::<Vec<_>>(), vec![1, 2]);
        assert_eq!(ws.minimized_counts(), (2, 2));
        // Most recent first.
        assert_eq!(ws.restore_last(), Some((2, p(20, 20))));
        assert!(!ws.is_minimized(&2));
        assert_eq!(ws.restore_last(), Some((1, p(10, 10))));
        assert_eq!(ws.restore_last(), None);
    }

    #[test]
    fn restore_a_given_window() {
        let mut ws: Workspaces<u32> = Workspaces::default();
        ws.minimize(1, p(1, 1));
        ws.minimize(2, p(2, 2));
        assert_eq!(ws.restore(&1), Some(p(1, 1)));
        assert_eq!(ws.restore(&1), None);
        assert_eq!(ws.restore_last(), Some((2, p(2, 2))));
    }

    #[test]
    fn minimized_windows_belong_to_their_workspace() {
        let mut ws: Workspaces<u32> = Workspaces::default();
        ws.minimize(1, p(1, 1));
        let sw = ws.switch(1, vec![(5, p(0, 0))], Some(5)).unwrap();
        assert!(sw.show.is_empty(), "a minimized window doesn't come back on a switch");
        assert_eq!(ws.minimized_counts(), (0, 1));
        assert_eq!(ws.restore_last(), None, "nothing minimized on this workspace");
        let sw = ws.switch(0, vec![], None).unwrap();
        assert_eq!(sw.show, vec![(5, p(0, 0))]);
        assert_eq!(ws.restore_last(), Some((1, p(1, 1))));
    }

    #[test]
    fn minimized_window_moves_and_closes() {
        let mut ws: Workspaces<u32> = Workspaces::default();
        ws.minimize(1, p(1, 1));
        assert!(ws.set_location(&1, p(0, 28)));
        assert_eq!(ws.find(&1), Some((0, p(0, 28))));
        assert_eq!(ws.remove(&1), Some((0, p(0, 28))));
        assert_eq!(ws.parked().count(), 0);
        assert_eq!(ws.restore_last(), None);
    }
}
