//! Phase 19: Siva, the desktop's assistant, local only (ROADMAP Phase 19). 19a is the part that
//! decides: the tools Siva may use (`tools`) and the rules for each step (`policy`). Nothing
//! here runs anything yet; Siva's page (19c) does, through the shell's existing commands. 19b is
//! Siva himself on the desktop (`siva`).
//!
//! Nothing in the assistant opens a network connection: speech, text and window titles stay on
//! this PC (decided 2026-10-22). `no_network` keeps it so.

// Wired to the page in 19c; until then only the tests use it.
#![cfg_attr(not(test), allow(dead_code))]

pub mod policy;
pub mod rules;
pub mod run;
pub mod siva;
pub mod tools;

#[cfg(test)]
mod tests {
    /// The assistant's code has no way to reach the network: no HTTP client, socket or URL.
    #[test]
    fn no_network() {
        let sources = [("mod.rs", include_str!("mod.rs")), ("tools.rs", include_str!("tools.rs")), ("policy.rs", include_str!("policy.rs")), ("siva.rs", include_str!("siva.rs")), ("rules.rs", include_str!("rules.rs")), ("run.rs", include_str!("run.rs"))];
        // Spelled in pieces so this list doesn't match itself.
        let banned = [["req", "west"], ["ure", "q::"], ["hy", "per"], ["Tcp", "Stream"], ["Udp", "Socket"], ["http", "://"], ["https", "://"]];
        for (file, text) in sources {
            for b in banned {
                let word = b.concat();
                assert!(!text.contains(&word), "{file} mentions {word}: the assistant stays on this PC");
            }
        }
    }
}
