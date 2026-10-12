//! 12i-1: idle time. Any key, pointer move, click or scroll is activity; after `lock-after`
//! minutes without any (Settings → Lock Screen & Energy; 0 = never) kompact starts the lock
//! screen (8d), once per idle stretch. An app that asks the screen to stay awake
//! (`zwp_idle_inhibit`, say a video playing) holds the count at zero while its window is shown,
//! so it starts again from when the video stops. Tools like swayidle get the same activity
//! through ext-idle-notify (Smithay's `IdleNotifierState`). 12i-3: the same for turning the
//! screens off (`screen-off-after`), once per idle stretch; any input turns them on again.

use std::time::{Duration, Instant};

/// The choices Settings offers, in minutes (0: never).
pub const LOCK_AFTER_CHOICES: [u32; 9] = [0, 1, 2, 5, 10, 15, 20, 30, 60];

#[derive(Debug, Clone, Copy)]
pub struct Idle {
    last: Instant,
    /// The lock for this idle stretch was started already.
    fired: bool,
    /// 12i-3: the screens were turned off in this idle stretch.
    off_fired: bool,
}

impl Idle {
    pub fn new(now: Instant) -> Idle {
        Idle { last: now, fired: false, off_fired: false }
    }

    pub fn activity(&mut self, now: Instant) {
        self.last = now;
        self.fired = false;
        self.off_fired = false;
    }

    pub fn idle_for(&self, now: Instant) -> Duration {
        now.saturating_duration_since(self.last)
    }

    /// Whether to lock now: `after_min` minutes (0: never) without activity, not while
    /// something keeps the screen awake, once per idle stretch.
    pub fn lock_due(&mut self, now: Instant, after_min: u32, inhibited: bool) -> bool {
        if inhibited {
            self.last = now;
            return false;
        }
        if after_min == 0 || self.fired || self.idle_for(now) < Duration::from_secs(after_min as u64 * 60) {
            return false;
        }
        self.fired = true;
        true
    }

    /// 12i-3: whether to turn the screens off now; like `lock_due`, with its own once-per-stretch.
    pub fn screen_off_due(&mut self, now: Instant, after_min: u32, inhibited: bool) -> bool {
        if inhibited {
            self.last = now;
            return false;
        }
        if after_min == 0 || self.off_fired || self.idle_for(now) < Duration::from_secs(after_min as u64 * 60) {
            return false;
        }
        self.off_fired = true;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn locks_once_after_the_idle_time() {
        let t0 = Instant::now();
        let min = |m: u64| t0 + Duration::from_secs(m * 60);
        let mut idle = Idle::new(t0);
        assert!(!idle.lock_due(min(4), 5, false));
        assert!(idle.lock_due(min(5), 5, false));
        // Once per stretch, until there's activity.
        assert!(!idle.lock_due(min(9), 5, false));
        idle.activity(min(10));
        assert!(!idle.lock_due(min(14), 5, false));
        assert!(idle.lock_due(min(15), 5, false));
        // Never.
        let mut idle = Idle::new(t0);
        assert!(!idle.lock_due(min(600), 0, false));
    }

    #[test]
    fn screens_off_and_lock_each_once() {
        let t0 = Instant::now();
        let min = |m: u64| t0 + Duration::from_secs(m * 60);
        let mut idle = Idle::new(t0);
        // Off after 2 minutes, locked after 5: both happen, each once.
        assert!(!idle.screen_off_due(min(1), 2, false));
        assert!(idle.screen_off_due(min(2), 2, false));
        assert!(!idle.screen_off_due(min(3), 2, false));
        assert!(!idle.lock_due(min(3), 5, false));
        assert!(idle.lock_due(min(5), 5, false));
        assert!(!idle.screen_off_due(min(6), 2, false));
        // Activity (which turns them on) starts it all again.
        idle.activity(min(7));
        assert!(idle.screen_off_due(min(9), 2, false));
        // Kept awake: never off.
        let mut idle = Idle::new(t0);
        assert!(!idle.screen_off_due(min(60), 2, true));
        assert!(!idle.screen_off_due(min(61), 0, false));
    }

    #[test]
    fn an_app_keeping_the_screen_awake() {
        let t0 = Instant::now();
        let min = |m: u64| t0 + Duration::from_secs(m * 60);
        let mut idle = Idle::new(t0);
        // A video plays from minute 1 to 30: no lock meanwhile, and the count starts at 30.
        assert!(!idle.lock_due(min(1), 5, true));
        assert!(!idle.lock_due(min(30), 5, true));
        assert!(!idle.lock_due(min(34), 5, false));
        assert!(idle.lock_due(min(35), 5, false));
        assert_eq!(idle.idle_for(min(35)), Duration::from_secs(5 * 60));
    }
}
