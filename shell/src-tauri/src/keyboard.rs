//! 12f: keyboard layouts for System Settings → Mouse & Keyboard and the menu bar's input
//! switcher. kompact owns the keyboard (its `layouts=` setting and the `keyboard` message with
//! the active layout); this lists what can be chosen, from xkb's own catalogue
//! (`/usr/share/X11/xkb/rules/evdev.lst`: layouts, and variants as `layout(variant)`), and passes
//! the menu bar's choice on (`keyboard-layout <n>`).

use serde::Serialize;
use tauri::State;

use crate::compositor::Compositor;

const CATALOGUE: &str = "/usr/share/X11/xkb/rules/evdev.lst";

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LayoutChoice {
    /// What kompact's `layouts=` takes: `us`, `de(nodeadkeys)`.
    pub code: String,
    pub name: String,
}

/// The `! layout` and `! variant` sections of an `evdev.lst`, layouts and their variants
/// together, by name.
pub fn catalogue(text: &str) -> Vec<LayoutChoice> {
    let mut out = Vec::new();
    let mut section = "";
    for line in text.lines() {
        if let Some(s) = line.strip_prefix("! ") {
            section = s.trim();
            continue;
        }
        let line = line.trim();
        let Some((code, rest)) = line.split_once(char::is_whitespace) else { continue };
        let rest = rest.trim();
        match section {
            "layout" => out.push(LayoutChoice { code: code.to_string(), name: rest.to_string() }),
            "variant" => {
                // "  nodeadkeys      de: German (no dead keys)"
                if let Some((layout, name)) = rest.split_once(": ") {
                    out.push(LayoutChoice { code: format!("{layout}({code})"), name: name.trim().to_string() });
                }
            }
            _ => {}
        }
    }
    // Only what kompact's `layouts=` takes (lowercase codes; a few variants have capitals), and
    // not the placeholder for a user's own file.
    out.retain(|l| l.code != "custom" && l.code.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"_-()".contains(&b)));
    out.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    out
}

/// Every layout and variant xkb knows (about 600), for the search in Settings.
#[tauri::command]
pub fn keyboard_layouts() -> Vec<LayoutChoice> {
    std::fs::read_to_string(CATALOGUE).map(|t| catalogue(&t)).unwrap_or_default()
}

/// The menu bar's switcher: use the n-th configured layout.
#[tauri::command]
pub fn keyboard_layout(c: State<'_, Compositor>, n: u32) -> Result<(), String> {
    c.send_line_pub("keyboard-layout", &format!("keyboard-layout {n}\n"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_catalogue() {
        let text = "! model\n  pc105           Generic 105-key PC\n\n! layout\n  us              English (US)\n  de              German\n  custom          A user-defined custom Layout\n\n! variant\n  nodeadkeys      de: German (no dead keys)\n  intl            us: English (US, intl., with dead keys)\n  OADG109A        jp: Japanese (OADG 109A)\n\n! option\n  grp             Switching to another layout\n";
        let list = catalogue(text);
        assert_eq!(
            list,
            vec![
                LayoutChoice { code: "us".into(), name: "English (US)".into() },
                LayoutChoice { code: "us(intl)".into(), name: "English (US, intl., with dead keys)".into() },
                LayoutChoice { code: "de".into(), name: "German".into() },
                LayoutChoice { code: "de(nodeadkeys)".into(), name: "German (no dead keys)".into() },
            ]
        );
    }

    /// This machine's catalogue: `cargo test real_catalogue -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn real_catalogue() {
        let list = keyboard_layouts();
        println!("{} layouts; e.g. {:?}", list.len(), list.iter().take(3).collect::<Vec<_>>());
        assert!(list.iter().any(|l| l.code == "us"));
    }
}
