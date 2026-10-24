//! 19c-2: what Siva was asked and what it did, kept on this PC for Settings → Assistant →
//! Activity: `~/.local/state/kompact/assistant.log` (`$XDG_STATE_HOME`), one line per entry,
//! `<unix seconds>\t<kind>\t<tool>\t<text>`. It stays small: past `MAX_BYTES` only the newest
//! `KEEP` entries are kept. Clear empties it.
//!
//! The lines and their reading are pure and tested; `record` and the commands touch the file.

use std::path::PathBuf;
use std::sync::Mutex;

use serde::Serialize;
use tauri::{AppHandle, Emitter};
use tracing::warn;

/// Past this the file is cut back to the newest `KEEP` entries.
pub const MAX_BYTES: u64 = 256 * 1024;
pub const KEEP: usize = 500;
/// How many Settings shows (newest first).
pub const SHOWN: usize = 200;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// The request as typed.
    Asked,
    /// 19d: as heard by voice.
    Heard,
    Ran,
    Failed,
    Refused,
    /// Waits for a click on Confirm.
    AsksFirst,
    Confirmed,
    Cancelled,
    Undone,
}

impl Kind {
    pub const ALL: [Kind; 9] = [Kind::Asked, Kind::Heard, Kind::Ran, Kind::Failed, Kind::Refused, Kind::AsksFirst, Kind::Confirmed, Kind::Cancelled, Kind::Undone];

    pub fn word(self) -> &'static str {
        match self {
            Kind::Asked => "asked",
            Kind::Heard => "heard",
            Kind::Ran => "ran",
            Kind::Failed => "failed",
            Kind::Refused => "refused",
            Kind::AsksFirst => "asks-first",
            Kind::Confirmed => "confirmed",
            Kind::Cancelled => "cancelled",
            Kind::Undone => "undone",
        }
    }

    pub fn parse(s: &str) -> Option<Kind> {
        Kind::ALL.into_iter().find(|k| k.word() == s)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Entry {
    /// Unix seconds; the page shows it in local time.
    pub at: u64,
    pub kind: &'static str,
    /// The tool's name, empty for what was asked.
    pub tool: String,
    pub text: String,
}

/// Tabs and line breaks would break the line apart.
fn flat(s: &str) -> String {
    s.chars().map(|c| if c.is_control() { ' ' } else { c }).take(400).collect()
}

pub fn line(at: u64, kind: Kind, tool: &str, text: &str) -> String {
    format!("{at}\t{}\t{}\t{}\n", kind.word(), flat(tool), flat(text))
}

/// The entries in the file's order (oldest first); lines that don't read are skipped.
pub fn parse(text: &str) -> Vec<Entry> {
    text.lines()
        .filter_map(|l| {
            let mut f = l.splitn(4, '\t');
            let at = f.next()?.parse().ok()?;
            let kind = Kind::parse(f.next()?)?.word();
            Some(Entry { at, kind, tool: f.next()?.into(), text: f.next()?.into() })
        })
        .collect()
}

/// The newest `keep` lines of `text`, for cutting the file back.
pub fn newest(text: &str, keep: usize) -> String {
    let lines: Vec<&str> = text.lines().filter(|l| !l.is_empty()).collect();
    let from = lines.len().saturating_sub(keep);
    lines[from..].iter().map(|l| format!("{l}\n")).collect()
}

// ---- the file ----

fn file() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local").join("state")))?;
    Some(base.join("kompact").join("assistant.log"))
}

/// One writer at a time (requests run on Tauri's async runtime).
static LOCK: Mutex<()> = Mutex::new(());

fn now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

fn append(entry: &str) -> std::io::Result<()> {
    use std::io::Write;
    let Some(path) = file() else { return Ok(()) };
    let _held = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    if std::fs::metadata(&path).map(|m| m.len() > MAX_BYTES).unwrap_or(false) {
        let text = std::fs::read_to_string(&path).unwrap_or_default();
        std::fs::write(&path, newest(&text, KEEP))?;
    }
    std::fs::OpenOptions::new().create(true).append(true).open(&path)?.write_all(entry.as_bytes())
}

/// Note one entry; Settings hears `kompact-assistant-activity` and reloads if it shows.
pub fn record(app: &AppHandle, kind: Kind, tool: &str, text: &str) {
    if let Err(e) = append(&line(now(), kind, tool, text)) {
        warn!("assistant: can't write the activity log: {e}");
    }
    let _ = app.emit("kompact-assistant-activity", ());
}

/// The newest `SHOWN` entries, newest first.
#[tauri::command]
pub fn assistant_activity() -> Vec<Entry> {
    let text = file().and_then(|p| std::fs::read_to_string(p).ok()).unwrap_or_default();
    let mut list = parse(&text);
    list.reverse();
    list.truncate(SHOWN);
    list
}

#[tauri::command]
pub fn assistant_activity_clear(app: AppHandle) -> Result<(), String> {
    let Some(path) = file() else { return Ok(()) };
    let _held = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    match std::fs::remove_file(&path) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(format!("Can't clear {}: {e}", path.display())),
    }
    tracing::info!("assistant: activity cleared");
    let _ = app.emit("kompact-assistant-activity", ());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lines_read_back() {
        let text = [
            line(100, Kind::Asked, "", "open files then put it on the left"),
            line(101, Kind::Ran, "open_app", "Opening Files."),
            line(102, Kind::AsksFirst, "power", "Restart the computer?"),
            line(103, Kind::Cancelled, "power", ""),
        ]
        .concat();
        let list = parse(&text);
        assert_eq!(list.len(), 4);
        assert_eq!(list[0], Entry { at: 100, kind: "asked", tool: "".into(), text: "open files then put it on the left".into() });
        assert_eq!(list[2].kind, "asks-first");
        assert_eq!(list[3].text, "");
    }

    #[test]
    fn nothing_breaks_a_line() {
        let l = line(5, Kind::Asked, "", "volume\t30\nand mute");
        assert_eq!(l.matches('\n').count(), 1);
        assert_eq!(parse(&l)[0].text, "volume 30 and mute");
        assert!(line(5, Kind::Asked, "", &"x".repeat(1000)).len() < 420, "long text is cut");
    }

    #[test]
    fn bad_lines_are_skipped() {
        let text = format!("junk\n7\tnope\t\tx\n{}\nabc\tasked\t\tx\n", line(8, Kind::Undone, "", "Undone.").trim_end());
        let list = parse(&text);
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].kind, "undone");
    }

    #[test]
    fn kinds_round_trip() {
        for k in Kind::ALL {
            assert_eq!(Kind::parse(k.word()), Some(k));
        }
    }

    #[test]
    fn cut_back_to_the_newest() {
        let text: String = (0..10).map(|i| line(i, Kind::Asked, "", &i.to_string())).collect();
        let kept = parse(&newest(&text, 3));
        assert_eq!(kept.iter().map(|e| e.at).collect::<Vec<_>>(), vec![7, 8, 9]);
        assert_eq!(newest(&text, 50), text);
    }
}
