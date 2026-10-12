//! 6p-b: SIGTERM, SIGINT and SIGHUP shut kompact down the way Super+Escape does (window
//! geometry remembered, portals stopped while the display is still up, the shell stopped),
//! instead of killing it on the spot.
//!
//! The handler only records the signal (an atomic store is all that's safe in a signal
//! handler); the shell-channel timer in `state.rs` picks it up within 50 ms and calls
//! `Kompact::shut_down`. Installed with `SA_RESETHAND`: a second signal, if shutting down hangs,
//! gets the default action and ends kompact at once.

use std::sync::atomic::{AtomicI32, Ordering};

static RECEIVED: AtomicI32 = AtomicI32::new(0);

const SIGNALS: [libc::c_int; 3] = [libc::SIGTERM, libc::SIGINT, libc::SIGHUP];

extern "C" fn on_signal(sig: libc::c_int) {
    RECEIVED.store(sig, Ordering::Relaxed);
}

/// Called once from `main`, before the backend starts.
pub fn install() {
    for sig in SIGNALS {
        // SAFETY: a zeroed sigaction is valid; we set the handler, an empty mask and plain
        // flags, and the handler itself only does an atomic store.
        let ok = unsafe {
            let mut action: libc::sigaction = std::mem::zeroed();
            action.sa_sigaction = on_signal as extern "C" fn(libc::c_int) as libc::sighandler_t;
            action.sa_flags = libc::SA_RESTART | libc::SA_RESETHAND;
            libc::sigemptyset(&mut action.sa_mask);
            libc::sigaction(sig, &action, std::ptr::null_mut()) == 0
        };
        if !ok {
            tracing::warn!("can't handle {}: {}", name(sig), std::io::Error::last_os_error());
        }
    }
}

/// The signal received since the last call, if any.
pub fn take() -> Option<libc::c_int> {
    match RECEIVED.swap(0, Ordering::Relaxed) {
        0 => None,
        sig => Some(sig),
    }
}

pub fn name(sig: libc::c_int) -> &'static str {
    match sig {
        libc::SIGTERM => "SIGTERM",
        libc::SIGINT => "SIGINT",
        libc::SIGHUP => "SIGHUP",
        _ => "a signal",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names() {
        assert_eq!(name(libc::SIGTERM), "SIGTERM");
        assert_eq!(name(libc::SIGHUP), "SIGHUP");
        assert_eq!(name(libc::SIGUSR1), "a signal");
    }

    #[test]
    fn recorded_once() {
        assert_eq!(take(), None);
        on_signal(libc::SIGTERM);
        assert_eq!(take(), Some(libc::SIGTERM));
        assert_eq!(take(), None);
    }
}
