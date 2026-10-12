//! Focus policy (Phase 2d): which window gets keyboard focus after a close, and when hovering
//! should move focus (focus-follows-mouse, off by default).
//!
//! Pure decisions over plain values so they're covered by `cargo test`; `shell.rs` feeds them
//! the real window stack and pointer state.

use std::sync::atomic::{AtomicBool, Ordering};

/// Startup value for focus-follows-mouse, set from `--focus-follows-mouse` in `main` before a
/// backend creates the compositor state (so the backends don't need a new parameter).
static FOCUS_FOLLOWS_MOUSE_AT_START: AtomicBool = AtomicBool::new(false);

pub fn set_focus_follows_mouse_at_start(on: bool) {
    FOCUS_FOLLOWS_MOUSE_AT_START.store(on, Ordering::Relaxed);
}

pub fn focus_follows_mouse_at_start() -> bool {
    FOCUS_FOLLOWS_MOUSE_AT_START.load(Ordering::Relaxed)
}

/// After a window closed, what to focus. `remaining` is the stack bottom-to-top without the
/// closed window. `needs_new_focus` is true if the closed window (or one of its popups) had
/// focus, or nothing valid has focus now.
///
/// Returns `None` to leave focus alone (closing a background window doesn't steal focus from
/// the one you're typing in), or `Some(target)` with the top-most remaining window, or
/// `Some(None)` if no window is left.
pub fn focus_after_close<T: Clone>(remaining: &[T], needs_new_focus: bool) -> Option<Option<T>> {
    if !needs_new_focus {
        return None;
    }
    Some(remaining.last().cloned())
}

/// Everything focus-follows-mouse needs to know about one pointer motion.
pub struct Hover<T> {
    pub enabled: bool,
    /// A compositor move/resize drag is in progress.
    pub dragging: bool,
    /// The pointer is grabbed: a button is held (client drag, text selection) or a popup grab is
    /// active. Moving focus then would break what the user is doing.
    pub grabbed: bool,
    /// The pointer is over a layer-shell surface (panel etc.) on the top/overlay layers.
    pub over_layer: bool,
    /// Keyboard focus is on something that isn't a toplevel window (e.g. a layer surface that
    /// asked for keyboard input). Hovering must not steal focus from it.
    pub focus_on_non_window: bool,
    /// Window under the pointer, if any.
    pub under: Option<T>,
    /// Window that currently has keyboard focus, if any.
    pub focused: Option<T>,
}

/// The window to focus because the pointer moved onto it, or `None` to leave focus alone.
/// "Sloppy" focus: moving onto empty desktop or a panel keeps the last window focused, and
/// hovering focuses without raising (clicking still raises).
pub fn hover_focus_target<T: PartialEq>(h: Hover<T>) -> Option<T> {
    if !h.enabled || h.dragging || h.grabbed || h.over_layer || h.focus_on_non_window {
        return None;
    }
    let under = h.under?;
    if h.focused.as_ref() == Some(&under) {
        return None;
    }
    Some(under)
}

/// A layer surface's `keyboard_interactivity` (wlr-layer-shell), as the focus rules see it (6c).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LayerKeys {
    /// Never gets the keyboard (panel, wallpaper).
    None,
    /// On the top/overlay layers: holds the keyboard while mapped (launcher, lock screen). On the
    /// bottom/background layers the protocol treats it like `OnDemand`.
    Exclusive,
    /// Gets the keyboard when clicked, loses it when something else is clicked.
    OnDemand,
}

/// The layer surface that must hold the keyboard right now: the top-most one with exclusive
/// interactivity on the top or overlay layer. `layers` is bottom-to-top as `(surface,
/// above_windows, keys)`, with overlay surfaces after top ones.
pub fn exclusive_layer<L: Clone>(layers: &[(L, bool, LayerKeys)]) -> Option<L> {
    layers
        .iter()
        .rev()
        .find(|(_, above_windows, keys)| *above_windows && *keys == LayerKeys::Exclusive)
        .map(|(l, _, _)| l.clone())
}

/// What a button press does to keyboard focus.
#[derive(Debug, PartialEq, Eq)]
pub enum ClickFocus<W, L> {
    Window(W),
    Layer(L),
    Keep,
    /// 6m-e5b: the desktop itself was clicked; no window keeps the keyboard (macOS).
    Clear,
}

/// Keyboard focus for a press on whatever is top-most under the pointer: a layer surface (with
/// its interactivity, and whether it lies below the windows: wallpaper) or a window, at most one
/// of them. While an exclusive layer surface exists, nothing else can take the keyboard.
pub fn click_focus<W, L>(exclusive_active: bool, layer: Option<(L, LayerKeys, bool)>, window: Option<W>) -> ClickFocus<W, L> {
    if exclusive_active {
        return ClickFocus::Keep;
    }
    match (layer, window) {
        (Some((l, LayerKeys::OnDemand | LayerKeys::Exclusive, _)), _) => ClickFocus::Layer(l),
        // The wallpaper, or bare desktop: as on macOS, the window loses the keyboard (6m-e5b).
        (Some((_, LayerKeys::None, true)), _) | (None, None) => ClickFocus::Clear,
        // A panel or the dock: the click is its own, and must not focus the window behind it,
        // nor take the keyboard from the one in front (the macOS menu bar and Dock don't).
        (Some((_, LayerKeys::None, false)), _) => ClickFocus::Keep,
        (None, Some(w)) => ClickFocus::Window(w),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hover(under: Option<u32>, focused: Option<u32>) -> Hover<u32> {
        Hover {
            enabled: true,
            dragging: false,
            grabbed: false,
            over_layer: false,
            focus_on_non_window: false,
            under,
            focused,
        }
    }

    #[test]
    fn close_focused_goes_to_top_most_remaining() {
        // Stack bottom-to-top: 1, 2, 3 remain after the focused top window closed.
        assert_eq!(focus_after_close(&[1, 2, 3], true), Some(Some(3)));
    }

    #[test]
    fn close_focused_never_picks_the_back_most() {
        // The old behavior focused elements().next(), the back-most window (1).
        assert_ne!(focus_after_close(&[1, 2, 3], true), Some(Some(1)));
    }

    #[test]
    fn close_background_window_keeps_focus() {
        assert_eq!(focus_after_close(&[1, 2, 3], false), None);
    }

    #[test]
    fn close_last_window_clears_focus() {
        assert_eq!(focus_after_close::<u32>(&[], true), Some(None));
    }

    #[test]
    fn hover_focuses_window_under_pointer() {
        assert_eq!(hover_focus_target(hover(Some(2), Some(1))), Some(2));
        assert_eq!(hover_focus_target(hover(Some(2), None)), Some(2));
    }

    #[test]
    fn hover_off_by_default_does_nothing() {
        assert!(!focus_follows_mouse_at_start());
        let mut h = hover(Some(2), Some(1));
        h.enabled = false;
        assert_eq!(hover_focus_target(h), None);
    }

    #[test]
    fn hover_already_focused_is_noop() {
        assert_eq!(hover_focus_target(hover(Some(1), Some(1))), None);
    }

    #[test]
    fn hover_empty_desktop_keeps_focus() {
        assert_eq!(hover_focus_target(hover(None, Some(1))), None);
    }

    #[test]
    fn hover_never_interrupts_drags_grabs_panels_or_layer_focus() {
        let cases: [fn(&mut Hover<u32>); 4] = [
            |h| h.dragging = true,
            |h| h.grabbed = true,
            |h| h.over_layer = true,
            |h| h.focus_on_non_window = true,
        ];
        for set in cases {
            let mut h = hover(Some(2), Some(1));
            set(&mut h);
            assert_eq!(hover_focus_target(h), None);
        }
    }

    #[test]
    fn exclusive_layer_is_top_most_above_windows() {
        use LayerKeys::*;
        assert_eq!(exclusive_layer::<u32>(&[]), Option::None);
        // Panel (none) and wallpaper (below windows, exclusive) don't count.
        assert_eq!(exclusive_layer(&[(1, false, Exclusive), (2, true, None), (3, true, OnDemand)]), Option::<u32>::None);
        // The top-most exclusive one wins.
        assert_eq!(exclusive_layer(&[(1, true, Exclusive), (2, true, None), (3, true, Exclusive)]), Some(3));
    }

    #[test]
    fn click_focus_rules() {
        use LayerKeys::*;
        type C = ClickFocus<u32, u32>;
        // Window clicked: it gets focus.
        assert_eq!(click_focus::<u32, u32>(false, Option::None, Some(7)), C::Window(7));
        // Panel or dock clicked (no keyboard, above the windows): nothing changes, even with a
        // window behind it.
        assert_eq!(click_focus(false, Some((1, None, false)), Some(7)), C::Keep);
        // Launcher-style surfaces take the keyboard when clicked.
        assert_eq!(click_focus(false, Some((2, OnDemand, false)), Some(7)), C::Layer(2));
        assert_eq!(click_focus::<u32, u32>(false, Some((2, Exclusive, false)), Option::None), C::Layer(2));
        // 6m-e5b: the wallpaper or bare desktop takes the keyboard away from the windows.
        assert_eq!(click_focus::<u32, u32>(false, Some((3, None, true)), Option::None), C::Clear);
        assert_eq!(click_focus::<u32, u32>(false, Option::None, Option::None), C::Clear);
        // An exclusive surface is open: clicks elsewhere can't take the keyboard from it.
        assert_eq!(click_focus::<u32, u32>(true, Option::None, Some(7)), C::Keep);
        assert_eq!(click_focus(true, Some((2, OnDemand, false)), Some(7)), C::Keep);
        assert_eq!(click_focus::<u32, u32>(true, Some((3, None, true)), Option::None), C::Keep);
    }
}
