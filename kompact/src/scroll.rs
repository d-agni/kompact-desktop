//! 13h: scrolling. A wheel notch reaches apps as libinput's distance (15 per notch) plus its
//! 120ths of a notch (`v120`); before 13h kompact sent the 120ths as the distance, so every
//! notch scrolled about 8 times too far. `speed` (Settings → Mouse & Keyboard, a percentage)
//! scales both, so apps reading either one scroll alike (Smithay sums `v120` into whole
//! notches for apps older than wl_pointer v8).

/// Scrolling speeds Settings offers, in percent of libinput's own.
pub const MIN_SPEED: u32 = 25;
pub const MAX_SPEED: u32 = 300;
pub const DEFAULT_SPEED: u32 = 100;

/// libinput's distance for one wheel notch, for events that only bring `v120`.
const NOTCH: f64 = 15.0;

/// What to send apps for one axis of a scroll event: the distance and the 120ths of a notch
/// (wheels only), at `speed` percent.
pub fn scaled(amount: Option<f64>, v120: Option<f64>, speed: u32) -> (f64, Option<i32>) {
    let k = speed.clamp(MIN_SPEED, MAX_SPEED) as f64 / 100.0;
    let amount = amount.or_else(|| v120.map(|v| v * NOTCH / 120.0)).unwrap_or(0.0);
    (amount * k, v120.map(|v| (v * k).round() as i32))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wheel_notches() {
        // One notch down at the default speed: libinput's 15 and 120ths of a notch.
        assert_eq!(scaled(Some(15.0), Some(120.0), 100), (15.0, Some(120)));
        // Half speed: half a notch each (apps need two notches for one step).
        assert_eq!(scaled(Some(15.0), Some(120.0), 50), (7.5, Some(60)));
        // Up, faster.
        assert_eq!(scaled(Some(-15.0), Some(-120.0), 200), (-30.0, Some(-240)));
        // No distance given: worked out from the notches.
        assert_eq!(scaled(None, Some(120.0), 100), (15.0, Some(120)));
        // Out-of-range speeds are held to the range.
        assert_eq!(scaled(Some(15.0), Some(120.0), 1), (3.75, Some(30)));
        assert_eq!(scaled(Some(15.0), Some(120.0), 1000), (45.0, Some(360)));
    }

    #[test]
    fn touchpads() {
        // Fingers bring a distance only.
        assert_eq!(scaled(Some(4.0), None, 50), (2.0, None));
        assert_eq!(scaled(None, None, 100), (0.0, None));
    }
}
