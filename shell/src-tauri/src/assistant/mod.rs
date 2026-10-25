//! Phase 19: Siva, the desktop's assistant, local only (ROADMAP Phase 19). The tools Siva may use
//! (`tools`) and the rules for each step (`policy`, 19a); what typed or spoken English means
//! (`rules`) and doing it, with Undo and confirm cards (`run`, 19c); what was done (`activity`,
//! 19c-2); hearing you (`voice`, 19d); the HUD under the menu bar (`hud`, 19h, which replaced the
//! character of 19b); speaking the replies (`speech`, 19h-2); routines (`routines`, 19e); a local model for what the rules don't know (`model`, 19f); answers about the desktop's own state (`diagnose`, 19g); learning your voice (`learn`, 19j); which window you mean (`refer`, 19k).
//!
//! Nothing in the assistant opens a network connection: speech, text and window titles stay on
//! this PC (decided 2026-10-22). `no_network` keeps it so.

// Wired to the page in 19c; until then only the tests use it.
#![cfg_attr(not(test), allow(dead_code))]

pub mod activity;
pub mod browser;
pub mod diagnose;
pub mod policy;
pub mod refer;
pub mod routines;
pub mod rules;
pub mod speech;
pub mod run;
pub mod hud;
pub mod learn;
pub mod model;
pub mod tools;
pub mod voice;

#[cfg(test)]
mod tests {
    /// The assistant's code has no way to reach the network: no HTTP client or socket anywhere,
    /// and web addresses only in `browser.rs` (for your browser to open, 20b).
    #[test]
    fn no_network() {
        let sources = [("mod.rs", include_str!("mod.rs")), ("activity.rs", include_str!("activity.rs")), ("tools.rs", include_str!("tools.rs")), ("policy.rs", include_str!("policy.rs")), ("hud.rs", include_str!("hud.rs")), ("speech.rs", include_str!("speech.rs")), ("routines.rs", include_str!("routines.rs")), ("model.rs", include_str!("model.rs")), ("diagnose.rs", include_str!("diagnose.rs")), ("learn.rs", include_str!("learn.rs")), ("refer.rs", include_str!("refer.rs")), ("rules.rs", include_str!("rules.rs")), ("run.rs", include_str!("run.rs")), ("voice.rs", include_str!("voice.rs")), ("browser.rs", include_str!("browser.rs"))];
        // Spelled in pieces so this list doesn't match itself.
        let clients = [["req", "west"], ["ure", "q::"], ["hy", "per"], ["Tcp", "Stream"], ["Udp", "Socket"], ["Unix", "Stream"], ["\"cu", "rl\""], ["\"wg", "et\""]];
        let addresses = [["http", "://"], ["https", "://"]];
        for (file, text) in sources {
            for b in clients {
                let word = b.concat();
                assert!(!text.contains(&word), "{file} mentions {word}: the assistant opens no connection");
            }
            for b in addresses {
                let word = b.concat();
                assert!(file == "browser.rs" || !text.contains(&word), "{file} mentions {word}: web addresses only in browser.rs");
            }
        }
    }
}
