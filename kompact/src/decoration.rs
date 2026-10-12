//! 6m-e1: xdg-decoration. 6m-e2b-1: kompact's own bar (`titlebar.rs`) (on by default since 6m-e4; `KOMPACT_SSD=off` to switch off).
//!
//! Apps such as foot, Qt and SDL ask whether kompact will draw their title bar (server side)
//! or they should (client side). Without the switch the answer is always client side, which is
//! what every app did anyway when the protocol wasn't offered. With it, an app that asks for
//! server side gets it, and `render.rs` draws the bar for windows whose current mode is server
//! side; an app that leaves the choice to us (or asks for client side) still draws its own.
//!
//! GTK apps (Firefox, Nautilus, gedit) don't use this protocol and always draw their own bar.

use std::collections::BTreeSet;

use smithay::reexports::wayland_protocols::xdg::decoration::zv1::server::zxdg_toplevel_decoration_v1::Mode;

/// The mode to tell an app that asked for `requested` (`None`: it left the choice to us);
/// `ssd`: kompact draws title bars (unless `KOMPACT_SSD=off`).
pub fn answer(requested: Option<Mode>, ssd: bool) -> Mode {
    if ssd && requested == Some(Mode::ServerSide) {
        Mode::ServerSide
    } else {
        Mode::ClientSide
    }
}

/// The app-ids that asked for server-side decorations, so each is logged once.
#[derive(Default)]
pub struct Askers(BTreeSet<String>);

impl Askers {
    /// True the first time `app` asks for server side.
    pub fn first_ask(&mut self, app: &str, requested: Option<Mode>) -> bool {
        requested == Some(Mode::ServerSide) && self.0.insert(app.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn client_side_unless_switched_on_and_asked() {
        for ssd in [false, true] {
            assert_eq!(answer(Some(Mode::ClientSide), ssd), Mode::ClientSide);
            assert_eq!(answer(None, ssd), Mode::ClientSide);
        }
        assert_eq!(answer(Some(Mode::ServerSide), false), Mode::ClientSide);
        assert_eq!(answer(Some(Mode::ServerSide), true), Mode::ServerSide);
    }

    #[test]
    fn each_asker_once() {
        let mut a = Askers::default();
        assert!(a.first_ask("foot", Some(Mode::ServerSide)));
        assert!(!a.first_ask("foot", Some(Mode::ServerSide)));
        assert!(!a.first_ask("gedit", Some(Mode::ClientSide)));
        assert!(!a.first_ask("mpv", None));
        assert!(a.first_ask("mpv", Some(Mode::ServerSide)));
    }
}
