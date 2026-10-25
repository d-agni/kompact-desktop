//! Phase 19: Siva, the desktop's assistant, local only (ROADMAP Phase 19). The tools Siva may use
//! (`tools`) and the rules for each step (`policy`, 19a); what typed or spoken English means
//! (`rules`) and doing it, with Undo and confirm cards (`run`, 19c); what was done (`activity`,
//! 19c-2); hearing you (`voice`, 19d); the HUD under the menu bar (`hud`, 19h, which replaced the
//! character of 19b); speaking the replies (`speech`, 19h-2); routines (`routines`, 19e); a local model for what the rules don't know (`model`, 19f); answers about the desktop's own state (`diagnose`, 19g); learning your voice (`learn`, 19j); which window you mean (`refer`, 19k); files and commands (`work`, 21a: commands run in Siva's terminal, drastic ones ask first); the work loop on the cloud AI, which sees each step's result (`agent`, 21b), or on a model on this PC (`local`, 21c: llama-server on a UNIX socket).
//!
//! Speech, text and window titles stay on this PC (decided 2026-10-22). Since 20d (2026-10-25)
//! one module goes online, for what you ask to look up (`web`: the weather, the news, facts,
//! search results), only to its own list of hosts and as Settings → Assistant allows;
//! `no_network` holds everything else to no connection at all. 20d-2: an optional cloud AI
//! (`cloud`, off until you give it a key) in the local model's place, its requests carried by
//! `web`.

// Wired to the page in 19c; until then only the tests use it.
#![cfg_attr(not(test), allow(dead_code))]

pub mod activity;
pub mod agent;
pub mod browser;
pub mod cloud;
pub mod diagnose;
pub mod policy;
pub mod refer;
pub mod routines;
pub mod rules;
pub mod speech;
pub mod run;
pub mod hud;
pub mod learn;
pub mod local;
pub mod model;
pub mod tools;
pub mod voice;
pub mod web;
pub mod work;

#[cfg(test)]
mod tests {
    /// The assistant's code has no way to reach the network but `web.rs` (20d): no HTTP client,
    /// socket or curl anywhere else, and web addresses only in `browser.rs` (for your browser to
    /// open, 20b). `web.rs` itself goes only to its `HOSTS` (its test `only_where_it_may_go`).
    /// 21a: a command you (or, from 21b, a model) give `run_command` may go online itself, in
    /// Siva's terminal where you see it; Siva's own code still opens nothing.
    #[test]
    fn no_network() {
        let sources = [("mod.rs", include_str!("mod.rs")), ("activity.rs", include_str!("activity.rs")), ("tools.rs", include_str!("tools.rs")), ("policy.rs", include_str!("policy.rs")), ("hud.rs", include_str!("hud.rs")), ("speech.rs", include_str!("speech.rs")), ("routines.rs", include_str!("routines.rs")), ("model.rs", include_str!("model.rs")), ("diagnose.rs", include_str!("diagnose.rs")), ("learn.rs", include_str!("learn.rs")), ("refer.rs", include_str!("refer.rs")), ("rules.rs", include_str!("rules.rs")), ("run.rs", include_str!("run.rs")), ("voice.rs", include_str!("voice.rs")), ("browser.rs", include_str!("browser.rs")), ("web.rs", include_str!("web.rs")), ("cloud.rs", include_str!("cloud.rs")), ("work.rs", include_str!("work.rs")), ("agent.rs", include_str!("agent.rs")), ("local.rs", include_str!("local.rs"))];
        // Spelled in pieces so this list doesn't match itself.
        let clients = [["req", "west"], ["ure", "q::"], ["hy", "per"], ["Tcp", "Stream"], ["Udp", "Socket"], ["Unix", "Stream"], ["\"cu", "rl\""], ["\"wg", "et\""]];
        let addresses = [["http", "://"], ["https", "://"]];
        for (file, text) in sources {
            // web.rs runs curl; it may name it and its hosts, and nothing else. work.rs (21a) names
            // curl and wget only to ask before a download is piped into a shell.
            let clients: &[[&str; 2]] = if file == "web.rs" || file == "work.rs" { &clients[..6] } else { &clients };
            for b in clients {
                let word = b.concat();
                assert!(!text.contains(&word), "{file} mentions {word}: the assistant opens no connection");
            }
            for b in addresses {
                let word = b.concat();
                assert!(file == "browser.rs" || file == "web.rs" || !text.contains(&word), "{file} mentions {word}: web addresses only in browser.rs and web.rs");
            }
        }
    }
}
