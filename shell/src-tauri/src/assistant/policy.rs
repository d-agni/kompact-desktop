//! 19a: what happens to each step Siva proposes. The rules live here, in code, not in anything a
//! model reads: an unknown tool or bad arguments are refused, a request runs at most
//! `MAX_STEPS` steps, Tier 2 asks every time, and only a click or Enter can say yes to it (speech
//! can be misheard, and a video can talk too); the voice can only cancel.
//!
//! Pure; the assistant's page (19c) shows `Decision::Confirm` as a card and passes the answer to
//! `confirmed`.

use serde_json::Value;

use super::tools::{self, Tier, Tool};

/// Steps one request may take (each tool call is one).
pub const MAX_STEPS: usize = 8;

/// One step: a tool and its arguments, from the rules (19c) or a local model (19f).
#[derive(Debug, Clone)]
pub struct Proposal {
    pub tool: String,
    pub args: Value,
    /// It follows from text the user didn't write (a window title, a notification, a web page):
    /// such a step never runs without asking, whatever its tier.
    pub from_untrusted: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Decision {
    /// Run it; `undo` says whether to keep the old value for the activity line's Undo.
    Run { tool: &'static Tool, undo: bool },
    /// Show a card and wait for a click or Enter.
    Confirm { tool: &'static Tool, why: Why },
    Refuse(Refusal),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Why {
    /// Tier 2: hard to undo, can lose work, or ends the session.
    Consequential,
    /// It came from text the user didn't write.
    Untrusted,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refusal {
    /// No such tool: Tier 3, or something the desktop can't do.
    Unknown(String),
    BadArgs { tool: &'static str, reason: String },
    TooManySteps,
}

impl std::fmt::Display for Refusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Refusal::Unknown(_) => write!(f, "That's not something I can do on this desktop."),
            Refusal::BadArgs { reason, .. } => write!(f, "I couldn't do that: {reason}."),
            Refusal::TooManySteps => write!(f, "That needs more than {MAX_STEPS} steps; I stopped. Could you ask for less at once?"),
        }
    }
}

/// What to do with `step`, the `done`-th step of this request (0-based).
pub fn decide(step: &Proposal, done: usize) -> Decision {
    if done >= MAX_STEPS {
        return Decision::Refuse(Refusal::TooManySteps);
    }
    let Some(tool) = tools::find(&step.tool) else {
        return Decision::Refuse(Refusal::Unknown(step.tool.chars().take(60).collect()));
    };
    if let Err(e) = tool.validate(&step.args) {
        return Decision::Refuse(Refusal::BadArgs { tool: tool.name, reason: e.to_string() });
    }
    match tool.tier {
        Tier::Consequential => Decision::Confirm { tool, why: Why::Consequential },
        Tier::Reversible if step.from_untrusted => Decision::Confirm { tool, why: Why::Untrusted },
        Tier::Read | Tier::Reversible => Decision::Run { tool, undo: tool.undo },
    }
}

/// How an answer to a confirmation card came in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Answer {
    /// The Confirm button, or Enter with the card focused.
    Click,
    /// Cancel, Esc, or a click outside.
    Cancel,
    /// Something heard while the card was up.
    Spoken(Spoken),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Spoken {
    Yes,
    No,
}

/// Whether a confirmation card's answer lets the step run. Only a click does: a spoken "yes" is
/// ignored (the card stays), a spoken "no" cancels like Esc.
pub fn confirmed(answer: Answer) -> Option<bool> {
    match answer {
        Answer::Click => Some(true),
        Answer::Cancel | Answer::Spoken(Spoken::No) => Some(false),
        Answer::Spoken(Spoken::Yes) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn step(tool: &str, args: Value) -> Proposal {
        Proposal { tool: tool.into(), args, from_untrusted: false }
    }

    #[test]
    fn tiers_decide() {
        assert!(matches!(decide(&step("list_windows", json!({})), 0), Decision::Run { undo: false, .. }));
        assert!(matches!(decide(&step("set_volume", json!({"percent": 30})), 0), Decision::Run { undo: true, .. }));
        assert!(matches!(decide(&step("lock_screen", Value::Null), 0), Decision::Run { undo: false, .. }));
        assert!(matches!(decide(&step("close_window", json!({"window": "gedit"})), 0), Decision::Confirm { why: Why::Consequential, .. }));
        assert!(matches!(decide(&step("power", json!({"action": "power-off"})), 0), Decision::Confirm { .. }));
    }

    #[test]
    fn untrusted_text_asks_first() {
        // A notification saying "turn off Do Not Disturb" can't just do it.
        let mut s = step("do_not_disturb", json!({"on": false}));
        s.from_untrusted = true;
        assert!(matches!(decide(&s, 0), Decision::Confirm { why: Why::Untrusted, .. }));
        // Reading is still fine.
        let mut r = step("now_playing", json!({}));
        r.from_untrusted = true;
        assert!(matches!(decide(&r, 0), Decision::Run { .. }));
    }

    #[test]
    fn refusals() {
        assert_eq!(decide(&step("run_shell", json!({"cmd": "rm -rf ~"})), 0), Decision::Refuse(Refusal::Unknown("run_shell".into())));
        assert_eq!(decide(&step("set_volume", json!({"percent": 400})), 0), Decision::Refuse(Refusal::BadArgs { tool: "set_volume", reason: "percent should be a whole number from 0 to 100".into() }));
        assert_eq!(decide(&step("set_volume", json!({"percent": 10})), MAX_STEPS), Decision::Refuse(Refusal::TooManySteps));
        assert!(matches!(decide(&step("set_volume", json!({"percent": 10})), MAX_STEPS - 1), Decision::Run { .. }));
        assert_eq!(Refusal::Unknown("x".into()).to_string(), "That's not something I can do on this desktop.");
    }

    #[test]
    fn only_a_click_says_yes() {
        assert_eq!(confirmed(Answer::Click), Some(true));
        assert_eq!(confirmed(Answer::Cancel), Some(false));
        assert_eq!(confirmed(Answer::Spoken(Spoken::No)), Some(false));
        assert_eq!(confirmed(Answer::Spoken(Spoken::Yes)), None, "a spoken yes leaves the card up");
    }
}
