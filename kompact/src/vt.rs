//! VT switches without a hang (found 2026-10-10).
//!
//! `session.change_vt` returns at once, but seatd takes DRM master from kompact right away and
//! the seat's Disable reaches us only when libseat's event is dispatched (smithay 0.4.4
//! `backend/session/libseat.rs`), a loop turn or more later. A page flip in between got EACCES,
//! which `backend_udev.rs` took as a fatal DRM error: kompact quit mid-switch, seatd 0.9.2 then
//! aborted (`get_tty_path: Assertion 'tty >= 0' failed`) and the VT stayed blank.
//!
//! Two guards: `SwitchGate` holds drawing back from our own switch request until the seat
//! answers, and `flip_failure` tells "the seat was taken" (EACCES, or the device already
//! paused) from a real DRM failure, which stays fatal.

use std::time::{Duration, Instant};

/// How long to hold drawing back if the seat never answers (a switch to the VT we're on).
pub const SWITCH_WAIT: Duration = Duration::from_millis(500);

/// Closed from our own `change_vt` until the seat pauses or reactivates us, or `SWITCH_WAIT`.
#[derive(Debug, Default)]
pub struct SwitchGate {
    requested: Option<Instant>,
}

impl SwitchGate {
    pub fn requested(&mut self, now: Instant) {
        self.requested = Some(now);
    }

    /// The seat paused or activated us: the switch is the session code's business now.
    pub fn answered(&mut self) {
        self.requested = None;
    }

    /// Whether our own switch request is still unanswered. Like `may_render`, but changes and
    /// logs nothing: for work that can simply be skipped (Phase 5 connector scans).
    pub fn switching(&self, now: Instant) -> bool {
        matches!(self.requested, Some(t) if now.duration_since(t) < SWITCH_WAIT)
    }

    /// Whether frames may be drawn. Opens again by itself after `SWITCH_WAIT`.
    pub fn may_render(&mut self, now: Instant) -> bool {
        match self.requested {
            None => true,
            Some(t) if now.duration_since(t) >= SWITCH_WAIT => {
                tracing::info!("vt: no answer from the seat in {} ms; drawing again", SWITCH_WAIT.as_millis());
                self.requested = None;
                true
            }
            Some(_) => false,
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum FlipFailure {
    /// Another session has the screen (VT switch): wait for the seat, don't quit.
    SeatTaken,
    Fatal,
}

/// A failed page flip: `inactive` if Smithay said the device is paused, else the OS error of
/// the DRM access, if there was one.
pub fn flip_failure(inactive: bool, io: Option<&std::io::Error>) -> FlipFailure {
    const EACCES: i32 = 13;
    let taken = inactive || io.is_some_and(|e| e.kind() == std::io::ErrorKind::PermissionDenied || e.raw_os_error() == Some(EACCES));
    if taken {
        FlipFailure::SeatTaken
    } else {
        FlipFailure::Fatal
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gate_closed_until_answer_or_timeout() {
        let t0 = Instant::now();
        let mut gate = SwitchGate::default();
        assert!(gate.may_render(t0));
        assert!(!gate.switching(t0));
        gate.requested(t0);
        assert!(gate.switching(t0) && gate.switching(t0 + SWITCH_WAIT - Duration::from_millis(1)));
        assert!(!gate.switching(t0 + SWITCH_WAIT));
        assert!(!gate.may_render(t0));
        assert!(!gate.may_render(t0 + Duration::from_millis(100)));
        gate.answered();
        assert!(gate.may_render(t0 + Duration::from_millis(100)));
        // No answer at all: open again after the wait, and stay open.
        gate.requested(t0);
        assert!(!gate.may_render(t0 + SWITCH_WAIT - Duration::from_millis(1)));
        assert!(gate.may_render(t0 + SWITCH_WAIT));
        assert!(gate.may_render(t0 + SWITCH_WAIT + Duration::from_millis(1)));
    }

    #[test]
    fn seat_taken_or_fatal() {
        use std::io::{Error, ErrorKind};
        // The 2026-10-10 log: "Failed to page flip", Os { code: 13, PermissionDenied }.
        assert_eq!(flip_failure(false, Some(&Error::from_raw_os_error(13))), FlipFailure::SeatTaken);
        assert_eq!(flip_failure(false, Some(&Error::from(ErrorKind::PermissionDenied))), FlipFailure::SeatTaken);
        assert_eq!(flip_failure(true, None), FlipFailure::SeatTaken);
        // Anything else is still a real DRM failure.
        assert_eq!(flip_failure(false, Some(&Error::from_raw_os_error(22))), FlipFailure::Fatal); // EINVAL
        assert_eq!(flip_failure(false, Some(&Error::from_raw_os_error(16))), FlipFailure::Fatal); // EBUSY
        assert_eq!(flip_failure(false, None), FlipFailure::Fatal);
    }
}
