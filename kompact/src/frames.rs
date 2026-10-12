//! 6p-e: frame callbacks paced to the screen, not to the event loop.
//!
//! `send_frames` runs on every turn of the event loop, and a turn happens on every client
//! message, not once a refresh. It used to tell Smithay that every surface was shown on the
//! output (so no throttle applied), which answered each app's frame callback right away: GTK4
//! apps (Calculator, Text Editor, Nautilus) and Firefox redrew and committed up to 370 times a
//! second, each a whole-window `wl_shm` damage that kompact copies to the GPU (6p-d measured
//! 1.5 Gpx in 10 s). Now each surface gets at most one frame callback per `PACE`, a bit under a
//! 60 Hz refresh. `KOMPACT_FRAME_PACING=off` is the old behaviour.

use std::time::Duration;

/// The shortest gap between two frame callbacks to the same surface (a 60 Hz refresh is 16.7 ms;
/// the loop wakes for each page flip, so a little under it keeps 60 fps).
pub const PACE: Duration = Duration::from_millis(15);

/// Paced (default) unless `KOMPACT_FRAME_PACING` is off/0/no/false.
pub fn paced_from_env(value: Option<&str>) -> bool {
    !matches!(value.map(|v| v.trim().to_ascii_lowercase()).as_deref(), Some("off" | "0" | "no" | "false"))
}

pub fn paced() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| {
        let on = paced_from_env(std::env::var("KOMPACT_FRAME_PACING").ok().as_deref());
        if on {
            tracing::info!("frames: callbacks paced, one per {} ms per surface (6p-e; KOMPACT_FRAME_PACING=off for the old way)", PACE.as_millis());
        } else {
            tracing::info!("frames: old way, callbacks on every loop turn (KOMPACT_FRAME_PACING=off)");
        }
        on
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paced_by_default() {
        assert!(paced_from_env(None));
        assert!(paced_from_env(Some("on")));
        assert!(!paced_from_env(Some("off")));
        assert!(!paced_from_env(Some(" False ")));
    }

    #[test]
    fn under_one_refresh() {
        assert!(PACE < Duration::from_micros(16_667));
    }
}
