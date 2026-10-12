//! 12h-2: how many menu bars there are. One on the main display, or (Settings → Displays →
//! Menu bar on every display) one per display. Every panel window asks for no monitor, and
//! kompact puts each new one on the main display first, then on a display without one, so the
//! shell only keeps the count right (`main.rs` `sync_panels`) and remaps its one menu bar when
//! the main display changes.

use serde_json::Value;

/// The first panel window; the others are `panel-2`, `panel-3`, …
pub const FIRST: &str = "panel";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Plan {
    /// Panel windows wanted.
    pub wanted: usize,
    /// Unmap and map the first panel again, so kompact puts it on the main display.
    pub remap_first: bool,
}

/// What a `displays` message means for the menu bars, given `have` panel windows and the main
/// display before it (`None`: the first message since the shell started).
pub fn plan(msg: &Value, have: usize, main_before: Option<&str>) -> Plan {
    let outputs = msg.get("outputs").and_then(Value::as_array).map_or(0, Vec::len);
    let every = msg.get("menu_bars").and_then(Value::as_str) == Some("every");
    let main = msg.get("primary").and_then(Value::as_str);
    let wanted = if every { outputs.max(1) } else { 1 };
    // With one menu bar it must be on the main display: after the main display changed, or
    // after the others went (the one left may be on another display).
    let main_changed = main_before.is_some() && main.is_some() && main_before != main;
    let remap_first = !every && (main_changed || have > wanted);
    Plan { wanted, remap_first }
}

/// `panel`, `panel-2`, …
pub fn label(i: usize) -> String {
    if i == 0 {
        FIRST.to_string()
    } else {
        format!("{FIRST}-{}", i + 1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn msg(n: usize, every: bool, main: &str) -> Value {
        let outputs: Vec<Value> = (0..n).map(|i| json!({"name": format!("O-{i}")})).collect();
        json!({"type": "displays", "outputs": outputs, "primary": main, "menu_bars": if every { "every" } else { "primary" }})
    }

    #[test]
    fn menu_bar_count() {
        // First message, one menu bar: nothing to do.
        assert_eq!(plan(&msg(2, false, "VGA-1"), 1, None), Plan { wanted: 1, remap_first: false });
        // Every display: one each.
        assert_eq!(plan(&msg(2, true, "VGA-1"), 1, None), Plan { wanted: 2, remap_first: false });
        // A monitor unplugged: one fewer, no remap (kompact already moved it).
        assert_eq!(plan(&msg(1, true, "VGA-1"), 2, Some("VGA-1")), Plan { wanted: 1, remap_first: false });
        // Back to one menu bar: it goes to the main display.
        assert_eq!(plan(&msg(2, false, "VGA-1"), 2, Some("VGA-1")), Plan { wanted: 1, remap_first: true });
        // Another main display.
        assert_eq!(plan(&msg(2, false, "HDMIA-1"), 1, Some("VGA-1")), Plan { wanted: 1, remap_first: true });
        // With a menu bar everywhere the main display doesn't move any.
        assert_eq!(plan(&msg(2, true, "HDMIA-1"), 2, Some("VGA-1")), Plan { wanted: 2, remap_first: false });
        // No outputs listed: still one.
        assert_eq!(plan(&msg(0, true, ""), 1, None).wanted, 1);
        assert_eq!((label(0).as_str(), label(1).as_str()), ("panel", "panel-2"));
    }
}
