//! 5j-3: the dock follows the pointer to another monitor, as on macOS: hold the pointer against
//! the bottom edge of a monitor the dock isn't on for `DWELL`, and kompact tells the shell
//! (`{"type":"dock_to","output":…}` on its channel); the shell remaps the dock, which kompact
//! then puts on the output under the pointer. `KOMPACT_DOCK_FOLLOW=off` turns it off.
//!
//! `EdgePush` is the pure rule; `Kompact::dock_follow_tick` feeds it from the 50 ms shell tick.

use std::time::{Duration, Instant};

/// How long the pointer must stay on the bottom edge.
pub const DWELL: Duration = Duration::from_millis(400);

/// The pointer counts as on the bottom edge this close to it (it's clamped to `h - 1`).
pub const EDGE_PX: f64 = 2.0;

#[derive(Debug, Default)]
pub struct EdgePush {
    /// The output whose bottom edge the pointer is on, since when, and whether that push fired.
    at: Option<(String, Instant, bool)>,
}

impl EdgePush {
    /// `edge`: the output whose bottom edge the pointer is on (`None`: not on one); `dock`: the
    /// output the dock is on (`None`: no dock). The output to move the dock to, once per push.
    pub fn update(&mut self, edge: Option<&str>, dock: Option<&str>, now: Instant) -> Option<String> {
        let (Some(edge), Some(dock)) = (edge, dock) else {
            self.at = None;
            return None;
        };
        if edge == dock {
            self.at = None;
            return None;
        }
        match &mut self.at {
            Some((name, since, fired)) if name == edge => {
                if !*fired && now.duration_since(*since) >= DWELL {
                    *fired = true;
                    return Some(edge.to_string());
                }
                None
            }
            _ => {
                self.at = Some((edge.to_string(), now, false));
                None
            }
        }
    }
}

/// Whether `y` is on the bottom edge of an output spanning `top..top + h`.
pub fn on_bottom_edge(y: f64, top: i32, h: i32) -> bool {
    y >= (top + h) as f64 - EDGE_PX && y < (top + h) as f64
}

/// 10d-1c: the screen edge the dock is on (the shell's `dock-side`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Side {
    Left,
    #[default]
    Bottom,
    Right,
}

impl Side {
    pub fn from_name(s: &str) -> Option<Side> {
        match s {
            "left" => Some(Side::Left),
            "bottom" => Some(Side::Bottom),
            "right" => Some(Side::Right),
            _ => None,
        }
    }
}

/// Whether `(x, y)` is on the `side` edge of an output at `(ox, oy)`, `w` × `h`.
pub fn on_edge(side: Side, (x, y): (f64, f64), (ox, oy, w, h): (i32, i32, i32, i32)) -> bool {
    match side {
        Side::Bottom => on_bottom_edge(y, oy, h),
        Side::Left => x >= ox as f64 && x < ox as f64 + EDGE_PX,
        Side::Right => x >= (ox + w) as f64 - EDGE_PX && x < (ox + w) as f64,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn held_on_another_monitors_bottom_edge() {
        let t = Instant::now();
        let ms = |n| t + Duration::from_millis(n);
        let mut p = EdgePush::default();
        assert_eq!(p.update(Some("HDMIA-1"), Some("VGA-1"), t), None);
        assert_eq!(p.update(Some("HDMIA-1"), Some("VGA-1"), ms(350)), None, "not long enough yet");
        assert_eq!(p.update(Some("HDMIA-1"), Some("VGA-1"), ms(400)), Some("HDMIA-1".into()));
        assert_eq!(p.update(Some("HDMIA-1"), Some("VGA-1"), ms(900)), None, "once per push");
        // Leaving the edge and pushing again counts anew.
        assert_eq!(p.update(None, Some("VGA-1"), ms(1000)), None);
        assert_eq!(p.update(Some("HDMIA-1"), Some("VGA-1"), ms(1100)), None);
        assert_eq!(p.update(Some("HDMIA-1"), Some("VGA-1"), ms(1500)), Some("HDMIA-1".into()));
    }

    #[test]
    fn not_for_the_docks_own_monitor_or_without_a_dock() {
        let t = Instant::now();
        let later = t + Duration::from_secs(1);
        let mut p = EdgePush::default();
        p.update(Some("VGA-1"), Some("VGA-1"), t);
        assert_eq!(p.update(Some("VGA-1"), Some("VGA-1"), later), None);
        p.update(Some("HDMIA-1"), None, t);
        assert_eq!(p.update(Some("HDMIA-1"), None, later), None);
        // The dock arrives meanwhile (it moved): the push is over.
        p.update(Some("HDMIA-1"), Some("VGA-1"), t);
        assert_eq!(p.update(Some("HDMIA-1"), Some("HDMIA-1"), later), None);
        assert_eq!(p.update(Some("HDMIA-1"), Some("VGA-1"), later), None, "starts again");
    }

    #[test]
    fn bottom_edge() {
        assert!(on_bottom_edge(767.0, 0, 768));
        assert!(on_bottom_edge(766.5, 0, 768));
        assert!(!on_bottom_edge(760.0, 0, 768));
        assert!(!on_bottom_edge(768.0, 0, 768), "below it is another output's");
        assert!(on_bottom_edge(899.0, 0, 900));
        // 10d-1c: the other edges, on VGA-1 (1366,0 1440x900) right of HDMI-A-1.
        let vga = (1366, 0, 1440, 900);
        assert!(on_edge(Side::Left, (1366.5, 400.0), vga));
        assert!(!on_edge(Side::Left, (1365.5, 400.0), vga), "that's HDMI-A-1's");
        assert!(on_edge(Side::Right, (2805.5, 400.0), vga));
        assert!(!on_edge(Side::Right, (2700.0, 400.0), vga));
        assert!(on_edge(Side::Bottom, (2000.0, 899.0), vga));
        assert_eq!(Side::from_name("left"), Some(Side::Left));
        assert_eq!(Side::from_name("top"), None);
    }
}
