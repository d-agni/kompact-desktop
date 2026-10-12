//! 8a: apps bring each other forward (xdg-activation-v1), e.g. a link clicked in a terminal
//! raises the browser that opens it.
//!
//! An app asks kompact for a token, passes it on (`XDG_ACTIVATION_TOKEN`, or over D-Bus), and
//! the app it reached asks to be activated with it. To keep apps from stealing focus, a token
//! only counts if the app that asked for it had the keyboard at the time (`accept_token`), and
//! only for `MAX_AGE` (`decide`). `KOMPACT_ACTIVATION=off` refuses every token.
//!
//! Pure rules here; `state.rs` has the Smithay handler.

use std::time::Duration;

/// A token older than this is refused (the Smithay example uses 10 s too).
pub const MAX_AGE: Duration = Duration::from_secs(10);

/// Whether to keep a newly asked-for token.
pub fn accept_token(enabled: bool, asker_has_keyboard: bool) -> bool {
    enabled && asker_has_keyboard
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    Activate,
    TooOld,
    Disabled,
}

/// What to do with an activation request carrying a token issued `age` ago.
pub fn decide(enabled: bool, age: Duration) -> Verdict {
    if !enabled {
        Verdict::Disabled
    } else if age >= MAX_AGE {
        Verdict::TooOld
    } else {
        Verdict::Activate
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_app_with_the_keyboard_gets_a_token() {
        assert!(accept_token(true, true));
        assert!(!accept_token(true, false), "a background app can't steal focus");
        assert!(!accept_token(false, true), "KOMPACT_ACTIVATION=off");
    }

    #[test]
    fn fresh_tokens_activate() {
        assert_eq!(decide(true, Duration::from_millis(300)), Verdict::Activate);
        assert_eq!(decide(true, Duration::from_secs(9)), Verdict::Activate);
        assert_eq!(decide(true, MAX_AGE), Verdict::TooOld);
        assert_eq!(decide(false, Duration::ZERO), Verdict::Disabled);
    }
}
