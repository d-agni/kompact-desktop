//! 11a: window animations, the pure part. A window being minimized shrinks and fades into its
//! dock icon; one being restored grows back out of it (macOS's "Scale" effect). 11b: one being
//! maximized, restored from maximized or snapped grows from where it was to where it goes,
//! once the app has drawn at its new size (until then it's *held* where it was). `render.rs`
//! draws a window at `Animations::frame(now)` instead of where it is; nothing here touches a
//! window or the renderer.
//!
//! `KOMPACT_ANIMATIONS=off` turns them all off.

use std::time::{Duration, Instant};

pub const MINIMIZE: Duration = Duration::from_millis(280);
pub const RESTORE: Duration = Duration::from_millis(240);
/// 11b: maximize / snap / restore.
pub const MOVE: Duration = Duration::from_millis(220);
/// 11b: how long a window waits where it was for the app to draw at the new size; after that
/// it's drawn where it is, as before.
pub const HOLD_MAX: Duration = Duration::from_millis(400);
/// How see-through a window is at the dock end of the animation.
pub const ALPHA_AT_DOCK: f32 = 0.25;

/// A rectangle in global logical coordinates, in fractions so slow frames stay smooth.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RectF {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

impl RectF {
    pub fn new(x: f64, y: f64, w: f64, h: f64) -> Self {
        RectF { x, y, w, h }
    }

    pub fn lerp(self, to: RectF, t: f64) -> RectF {
        let l = |a: f64, b: f64| a + (b - a) * t;
        RectF { x: l(self.x, to.x), y: l(self.y, to.y), w: l(self.w, to.w), h: l(self.h, to.h) }
    }
}

/// Slow start, slow end.
pub fn ease_in_out(t: f64) -> f64 {
    let t = t.clamp(0.0, 1.0);
    if t < 0.5 {
        4.0 * t * t * t
    } else {
        1.0 - (-2.0 * t + 2.0).powi(3) / 2.0
    }
}

/// Fast start, slow end.
pub fn ease_out(t: f64) -> f64 {
    1.0 - (1.0 - t.clamp(0.0, 1.0)).powi(3)
}

/// Where `frame` ends up at a dock icon: its shape kept, its longer side `size`, centred on
/// `centre`.
pub fn icon_rect(frame: RectF, centre: (f64, f64), size: f64) -> RectF {
    let s = if frame.w.max(frame.h) > 0.0 { size / frame.w.max(frame.h) } else { 1.0 };
    let (w, h) = (frame.w * s, frame.h * s);
    RectF { x: centre.0 - w / 2.0, y: centre.1 - h / 2.0, w, h }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// Into the dock; the window is already unmapped and drawn as a ghost.
    Minimize,
    /// Out of the dock; the window is mapped and drawn here instead of where it is.
    Restore,
    /// 11b: from its old place and size to the new ones (maximize, snap, restore).
    Move,
}

/// One frame of an animation: where the window's frame (title bar included) is drawn, and how
/// opaque.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Frame {
    pub rect: RectF,
    pub alpha: f32,
}

#[derive(Debug, Clone, Copy)]
pub struct Anim {
    pub kind: Kind,
    start: Instant,
    duration: Duration,
    from: RectF,
    to: RectF,
    alpha: (f32, f32),
}

impl Anim {
    /// Minimize (window → icon) or restore (icon → window).
    pub fn new(kind: Kind, start: Instant, window: RectF, icon: RectF) -> Self {
        match kind {
            Kind::Minimize => Anim { kind, start, duration: MINIMIZE, from: window, to: icon, alpha: (1.0, ALPHA_AT_DOCK) },
            Kind::Restore => Anim { kind, start, duration: RESTORE, from: icon, to: window, alpha: (ALPHA_AT_DOCK, 1.0) },
            Kind::Move => Self::moving(start, window, icon),
        }
    }

    /// 11b: from one frame to another, opaque all the way.
    pub fn moving(start: Instant, from: RectF, to: RectF) -> Self {
        Anim { kind: Kind::Move, start, duration: MOVE, from, to, alpha: (1.0, 1.0) }
    }

    /// The frame at `now`, or `None` once it's over.
    pub fn at(&self, now: Instant) -> Option<Frame> {
        let t = now.saturating_duration_since(self.start).as_secs_f64() / self.duration.as_secs_f64();
        if t >= 1.0 {
            return None;
        }
        let e = match self.kind {
            Kind::Minimize => ease_in_out(t),
            Kind::Restore | Kind::Move => ease_out(t),
        };
        let alpha = self.alpha.0 + (self.alpha.1 - self.alpha.0) * e as f32;
        Some(Frame { rect: self.from.lerp(self.to, e), alpha })
    }
}

/// 11b: a window asked to change size, drawn where it was until the app has drawn at the new
/// size (its geometry no longer `size`).
#[derive(Debug, Clone, Copy)]
struct Held {
    from: RectF,
    size: (i32, i32),
    since: Instant,
}

/// The running animations, at most one per window (a new one replaces it, so a window
/// restored mid-minimize turns round).
#[derive(Debug)]
pub struct Animations<W> {
    list: Vec<(W, Anim)>,
    held: Vec<(W, Held)>,
}

impl<W> Default for Animations<W> {
    fn default() -> Self {
        Animations { list: Vec::new(), held: Vec::new() }
    }
}

impl<W: PartialEq + Clone> Animations<W> {
    pub fn start(&mut self, window: W, anim: Anim) {
        self.list.retain(|(w, _)| *w != window);
        self.held.retain(|(w, _)| *w != window);
        self.list.push((window, anim));
    }

    /// 11b: hold `window` at `from` (its frame now, its geometry `size`) until `committed`
    /// sees another size, or `HOLD_MAX`. A running animation's current frame is the start.
    pub fn hold(&mut self, window: W, from: RectF, size: (i32, i32), now: Instant) {
        let from = self.frame(&window, now).map(|f| f.rect).unwrap_or(from);
        self.list.retain(|(w, _)| *w != window);
        self.held.retain(|(w, _)| *w != window);
        self.held.push((window, Held { from, size, since: now }));
    }

    /// 11b: where a held window started, if it's held (and not for too long).
    pub fn held_from(&self, window: &W, now: Instant) -> Option<RectF> {
        self.held
            .iter()
            .find(|(w, h)| w == window && now.saturating_duration_since(h.since) < HOLD_MAX)
            .map(|(_, h)| h.from)
    }

    /// 11b: the app committed with geometry `size`, its frame now `to`. A held window whose size
    /// changed starts moving from where it was held; returns whether it did.
    pub fn committed(&mut self, window: &W, size: (i32, i32), to: RectF, now: Instant) -> bool {
        let Some(i) = self.held.iter().position(|(w, _)| w == window) else { return false };
        let h = self.held[i].1;
        if size == h.size && now.saturating_duration_since(h.since) < HOLD_MAX {
            return false; // still the old size: keep holding
        }
        self.held.remove(i);
        if now.saturating_duration_since(h.since) >= HOLD_MAX || h.from == to {
            return false;
        }
        self.list.retain(|(w, _)| w != window);
        self.list.push((window.clone(), Anim::moving(now, h.from, to)));
        true
    }

    /// Drop this window's animation or hold (it's being dragged: it follows the pointer).
    pub fn stop(&mut self, window: &W) {
        self.list.retain(|(w, _)| w != window);
        self.held.retain(|(w, _)| w != window);
    }

    /// This window's frame now, if it's animating (or held).
    pub fn frame(&self, window: &W, now: Instant) -> Option<Frame> {
        self.list
            .iter()
            .find(|(w, _)| w == window)
            .and_then(|(_, a)| a.at(now))
            .or_else(|| self.held_from(window, now).map(|rect| Frame { rect, alpha: 1.0 }))
    }

    /// The windows being minimized (drawn as ghosts), oldest first, with their frames now.
    pub fn ghosts(&self, now: Instant) -> Vec<(W, Frame)> {
        self.list
            .iter()
            .filter(|(_, a)| a.kind == Kind::Minimize)
            .filter_map(|(w, a)| Some((w.clone(), a.at(now)?)))
            .collect()
    }

    /// Drop finished ones (and those `keep` says no to: closed windows). Returns how many are
    /// still running.
    pub fn prune(&mut self, now: Instant, keep: impl Fn(&W) -> bool) -> usize {
        self.list.retain(|(w, a)| a.at(now).is_some() && keep(w));
        self.held.retain(|(w, h)| now.saturating_duration_since(h.since) < HOLD_MAX && keep(w));
        self.list.len() + self.held.len()
    }

    pub fn is_empty(&self) -> bool {
        self.list.is_empty() && self.held.is_empty()
    }
}

/// On unless the variable says off.
pub fn enabled_from_env(value: Option<&str>) -> bool {
    !matches!(value.map(|v| v.trim().to_ascii_lowercase()).as_deref(), Some("off" | "0" | "no" | "false"))
}

#[cfg(test)]
mod tests {
    use super::*;

    const WIN: RectF = RectF { x: 100.0, y: 100.0, w: 800.0, h: 400.0 };

    fn close(a: RectF, b: RectF) -> bool {
        (a.x - b.x).abs() < 1e-6 && (a.y - b.y).abs() < 1e-6 && (a.w - b.w).abs() < 1e-6 && (a.h - b.h).abs() < 1e-6
    }

    #[test]
    fn easing_ends() {
        for f in [ease_in_out, ease_out] {
            assert_eq!(f(0.0), 0.0);
            assert!((f(1.0) - 1.0).abs() < 1e-9);
            assert_eq!(f(-1.0), 0.0);
            assert!((f(2.0) - 1.0).abs() < 1e-9);
        }
        assert!((ease_in_out(0.5) - 0.5).abs() < 1e-9);
        assert!(ease_out(0.5) > 0.5, "fast start");
        // Never goes backwards.
        let mut last = 0.0;
        for i in 0..=100 {
            let v = ease_in_out(i as f64 / 100.0);
            assert!(v >= last);
            last = v;
        }
    }

    #[test]
    fn icon_rect_keeps_the_shape() {
        let r = icon_rect(WIN, (500.0, 860.0), 50.0);
        assert!(close(r, RectF::new(475.0, 847.5, 50.0, 25.0)));
        let tall = icon_rect(RectF::new(0.0, 0.0, 200.0, 400.0), (10.0, 10.0), 40.0);
        assert!(close(tall, RectF::new(0.0, -10.0, 20.0, 40.0)));
        // Nothing to scale: no division by zero.
        let empty = icon_rect(RectF::new(0.0, 0.0, 0.0, 0.0), (5.0, 5.0), 40.0);
        assert_eq!((empty.x, empty.y), (5.0, 5.0));
    }

    #[test]
    fn minimize_goes_from_the_window_to_the_icon() {
        let t0 = Instant::now();
        let icon = icon_rect(WIN, (500.0, 860.0), 50.0);
        let a = Anim::new(Kind::Minimize, t0, WIN, icon);
        let first = a.at(t0).unwrap();
        assert!(close(first.rect, WIN));
        assert_eq!(first.alpha, 1.0);
        let mid = a.at(t0 + MINIMIZE / 2).unwrap();
        assert!(mid.rect.w < WIN.w && mid.rect.w > icon.w);
        assert!(mid.alpha < 1.0 && mid.alpha > ALPHA_AT_DOCK);
        let late = a.at(t0 + MINIMIZE - Duration::from_millis(1)).unwrap();
        assert!((late.rect.w - icon.w).abs() < 1.0);
        assert!(a.at(t0 + MINIMIZE).is_none());
    }

    #[test]
    fn restore_goes_from_the_icon_to_the_window() {
        let t0 = Instant::now();
        let icon = icon_rect(WIN, (500.0, 860.0), 50.0);
        let a = Anim::new(Kind::Restore, t0, WIN, icon);
        let first = a.at(t0).unwrap();
        assert!(close(first.rect, icon));
        assert!((first.alpha - ALPHA_AT_DOCK).abs() < 1e-6);
        let late = a.at(t0 + RESTORE - Duration::from_millis(1)).unwrap();
        assert!((late.rect.w - WIN.w).abs() < 1.0);
        assert!(a.at(t0 + RESTORE).is_none());
    }

    #[test]
    fn one_animation_per_window() {
        let t0 = Instant::now();
        let icon = icon_rect(WIN, (500.0, 860.0), 50.0);
        let mut list = Animations::default();
        list.start(1u32, Anim::new(Kind::Minimize, t0, WIN, icon));
        list.start(2u32, Anim::new(Kind::Minimize, t0, WIN, icon));
        assert_eq!(list.ghosts(t0).len(), 2);
        // Restored mid-minimize: it turns round and is no longer a ghost.
        list.start(1u32, Anim::new(Kind::Restore, t0, WIN, icon));
        assert_eq!(list.ghosts(t0).iter().map(|(w, _)| *w).collect::<Vec<_>>(), [2]);
        assert!(list.frame(&1, t0).is_some());
        assert!(list.frame(&3, t0).is_none());
        // Window 2 closed: dropped; then everything ends.
        assert_eq!(list.prune(t0, |w| *w != 2), 1);
        assert_eq!(list.prune(t0 + MINIMIZE, |_| true), 0);
        assert!(list.is_empty());
    }

    #[test]
    fn a_held_window_moves_once_the_app_has_resized() {
        let t0 = Instant::now();
        let to = RectF::new(0.0, 28.0, 1440.0, 802.0);
        let mut list = Animations::default();
        list.hold(1u32, WIN, (800, 370), t0);
        // Held where it was until then.
        assert!(close(list.frame(&1, t0 + Duration::from_millis(30)).unwrap().rect, WIN));
        // A commit at the old size (say, focus drawn) changes nothing.
        assert!(!list.committed(&1, (800, 370), to, t0 + Duration::from_millis(40)));
        assert!(list.frame(&1, t0 + Duration::from_millis(40)).is_some());
        // The new size: it grows from where it was to where it is.
        let t1 = t0 + Duration::from_millis(60);
        assert!(list.committed(&1, (1440, 774), to, t1));
        assert!(close(list.frame(&1, t1).unwrap().rect, WIN));
        let late = list.frame(&1, t1 + MOVE - Duration::from_millis(1)).unwrap();
        assert!((late.rect.w - to.w).abs() < 1.0);
        assert_eq!(late.alpha, 1.0);
        assert!(list.frame(&1, t1 + MOVE).is_none());
        // Not a ghost: it's mapped.
        assert!(list.ghosts(t1).is_empty());
    }

    #[test]
    fn a_hold_gives_up() {
        let t0 = Instant::now();
        let mut list = Animations::default();
        list.hold(1u32, WIN, (800, 370), t0);
        assert!(list.frame(&1, t0 + HOLD_MAX).is_none(), "drawn where it is again");
        assert!(!list.committed(&1, (1440, 774), WIN, t0 + HOLD_MAX));
        assert_eq!(list.prune(t0 + HOLD_MAX, |_| true), 0);
        // Committed at once at the new size but nowhere else to go: nothing to animate.
        list.hold(2u32, WIN, (800, 370), t0);
        assert!(!list.committed(&2, (900, 370), WIN, t0));
        assert!(list.is_empty());
    }

    #[test]
    fn a_hold_mid_animation_starts_from_there() {
        let t0 = Instant::now();
        let to = RectF::new(0.0, 0.0, 1000.0, 600.0);
        let mut list = Animations::default();
        list.start(1u32, Anim::moving(t0, WIN, to));
        let t1 = t0 + MOVE / 2;
        let mid = list.frame(&1, t1).unwrap().rect;
        list.hold(1u32, to, (1000, 572), t1);
        assert!(close(list.frame(&1, t1).unwrap().rect, mid));
        list.stop(&1);
        assert!(list.is_empty());
    }

    #[test]
    fn switch() {
        assert!(enabled_from_env(None));
        assert!(enabled_from_env(Some("on")));
        assert!(!enabled_from_env(Some("off")));
        assert!(!enabled_from_env(Some(" No ")));
    }
}
