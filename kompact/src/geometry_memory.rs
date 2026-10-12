//! Remembered per-application geometry (Phase 2c).
//!
//! When an app's last window closes, its floating geometry (the size/position it had or would
//! return to, ignoring snap/maximize) is remembered under its xdg app-id. When the app opens a
//! window again and no other window of it is open, that geometry is used instead of smart
//! placement. Kept small (at most `MAX_APPS` entries, least recently used dropped first) and
//! persisted as a plain text file, one app per line: `x y w h app-id`.
//!
//! Phase 5: `x y` is relative to the monitor the window was on (`moved_between`), so an app
//! comes back at the same spot on whichever monitor it's opened on (the one under the pointer).
//! Geometry saved with one monitor at 0,0 means the same thing.
//!
//! Everything except the two file helpers is pure and covered by `cargo test`.

use std::{
    io::Write,
    path::{Path, PathBuf},
};

use smithay::utils::{Logical, Point, Rectangle};

/// Upper bound on remembered apps, so the file and lookups stay trivially small.
pub const MAX_APPS: usize = 64;

#[derive(Debug, Default, PartialEq)]
pub struct GeometryMemory {
    /// Least recently remembered first.
    entries: Vec<(String, Rectangle<i32, Logical>)>,
}

impl GeometryMemory {
    pub fn recall(&self, app_id: &str) -> Option<Rectangle<i32, Logical>> {
        self.entries.iter().find(|(id, _)| id == app_id).map(|(_, r)| *r)
    }

    /// Remember `rect` for `app_id`. Returns whether anything changed (so callers only write the
    /// file when needed). Empty app-ids and empty rectangles are ignored.
    pub fn remember(&mut self, app_id: &str, rect: Rectangle<i32, Logical>) -> bool {
        if !valid_app_id(app_id) || rect.size.w <= 0 || rect.size.h <= 0 {
            return false;
        }
        let pos = self.entries.iter().position(|(id, _)| id == app_id);
        if let Some(pos) = pos {
            if pos == self.entries.len() - 1 && self.entries[pos].1 == rect {
                return false;
            }
            self.entries.remove(pos);
        }
        self.entries.push((app_id.to_string(), rect));
        if self.entries.len() > MAX_APPS {
            self.entries.remove(0);
        }
        true
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn to_text(&self) -> String {
        let mut out = String::new();
        for (id, r) in &self.entries {
            out.push_str(&format!("{} {} {} {} {}\n", r.loc.x, r.loc.y, r.size.w, r.size.h, id));
        }
        out
    }

    /// Parse the file format. Malformed lines are skipped rather than failing the whole file.
    pub fn from_text(text: &str) -> Self {
        let mut mem = Self::default();
        for line in text.lines() {
            let mut parts = line.splitn(5, ' ');
            let nums: Vec<i32> = parts.by_ref().take(4).filter_map(|p| p.parse().ok()).collect();
            let Some(id) = parts.next() else { continue };
            if let [x, y, w, h] = nums[..] {
                mem.remember(id, Rectangle::new((x, y).into(), (w, h).into()));
            }
        }
        mem
    }
}

/// App-ids are written one per line with spaces as separators, so reject anything that would
/// break the format (newlines) or is clearly not an app-id. Spaces are fine: the id is the last
/// field and taken whole.
fn valid_app_id(app_id: &str) -> bool {
    !app_id.trim().is_empty() && !app_id.contains(['\n', '\r']) && app_id.len() <= 255
}

/// Move (and if needed shrink) `rect` so it lies entirely inside `area`. Used for remembered
/// geometry (the screen or panels may have changed since) and for restoring from snap/maximize
/// (a window can be dropped mostly off-screen).
pub fn fit_inside(rect: Rectangle<i32, Logical>, area: Rectangle<i32, Logical>) -> Rectangle<i32, Logical> {
    let w = rect.size.w.min(area.size.w).max(1);
    let h = rect.size.h.min(area.size.h).max(1);
    let max_x = area.loc.x + area.size.w - w;
    let max_y = area.loc.y + area.size.h - h;
    let x = rect.loc.x.clamp(area.loc.x, max_x.max(area.loc.x));
    let y = rect.loc.y.clamp(area.loc.y, max_y.max(area.loc.y));
    Rectangle::new((x, y).into(), (w, h).into())
}

/// `rect` with its position taken from one monitor's coordinates (top-left corner `from`) to
/// another's (`to`). `to` = 0,0 makes it monitor-relative, as it's remembered.
pub fn moved_between(rect: Rectangle<i32, Logical>, from: Point<i32, Logical>, to: Point<i32, Logical>) -> Rectangle<i32, Logical> {
    Rectangle::new(rect.loc - from + to, rect.size)
}

/// `$XDG_STATE_HOME/kompact/geometry`, falling back to `~/.local/state/kompact/geometry`.
pub fn default_path() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_STATE_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/state")))?;
    Some(base.join("kompact").join("geometry"))
}

/// Load from `path`. A missing file is just an empty memory; any other error is logged.
pub fn load(path: &Path) -> GeometryMemory {
    match std::fs::read_to_string(path) {
        Ok(text) => GeometryMemory::from_text(&text),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => GeometryMemory::default(),
        Err(e) => {
            tracing::warn!("could not read window geometry memory {}: {e}", path.display());
            GeometryMemory::default()
        }
    }
}

/// Write to `path` via a temp file + rename, so a reader never sees a half-written file. No
/// fsync: this runs on the compositor thread when a window closes, and a lost update after a
/// power cut only costs one remembered position.
pub fn save(path: &Path, mem: &GeometryMemory) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("tmp");
    std::fs::File::create(&tmp)?.write_all(mem.to_text().as_bytes())?;
    std::fs::rename(&tmp, path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(x: i32, y: i32, w: i32, h: i32) -> Rectangle<i32, Logical> {
        Rectangle::new((x, y).into(), (w, h).into())
    }

    #[test]
    fn remembers_and_recalls_per_app() {
        let mut m = GeometryMemory::default();
        assert!(m.remember("foot", rect(10, 40, 800, 500)));
        assert!(m.remember("firefox", rect(0, 32, 1440, 868)));
        assert_eq!(m.recall("foot"), Some(rect(10, 40, 800, 500)));
        assert_eq!(m.recall("firefox"), Some(rect(0, 32, 1440, 868)));
        assert_eq!(m.recall("gimp"), None);
    }

    #[test]
    fn later_geometry_replaces_earlier() {
        let mut m = GeometryMemory::default();
        m.remember("foot", rect(10, 40, 800, 500));
        m.remember("foot", rect(100, 100, 600, 400));
        assert_eq!(m.recall("foot"), Some(rect(100, 100, 600, 400)));
        assert_eq!(m.len(), 1);
    }

    #[test]
    fn unchanged_geometry_reports_no_change() {
        let mut m = GeometryMemory::default();
        assert!(m.remember("foot", rect(10, 40, 800, 500)));
        assert!(!m.remember("foot", rect(10, 40, 800, 500)));
    }

    #[test]
    fn ignores_bad_input() {
        let mut m = GeometryMemory::default();
        assert!(!m.remember("", rect(0, 0, 100, 100)));
        assert!(!m.remember("   ", rect(0, 0, 100, 100)));
        assert!(!m.remember("evil\nid", rect(0, 0, 100, 100)));
        assert!(!m.remember("foot", rect(0, 0, 0, 100)));
        assert_eq!(m.len(), 0);
    }

    #[test]
    fn bounded_and_drops_least_recent() {
        let mut m = GeometryMemory::default();
        for i in 0..(MAX_APPS + 10) {
            m.remember(&format!("app{i}"), rect(0, 0, 100, 100));
        }
        assert_eq!(m.len(), MAX_APPS);
        assert_eq!(m.recall("app0"), None);
        assert!(m.recall(&format!("app{}", MAX_APPS + 9)).is_some());

        // Re-remembering an old app makes it recent again, so it survives the next eviction.
        let mut m = GeometryMemory::default();
        for i in 0..MAX_APPS {
            m.remember(&format!("app{i}"), rect(0, 0, 100, 100));
        }
        m.remember("app0", rect(1, 1, 100, 100));
        m.remember("new", rect(0, 0, 100, 100));
        assert!(m.recall("app0").is_some());
        assert_eq!(m.recall("app1"), None);
    }

    #[test]
    fn text_round_trip() {
        let mut m = GeometryMemory::default();
        m.remember("foot", rect(-20, 40, 800, 500));
        m.remember("org.gnome.Some App", rect(0, 32, 1440, 868)); // spaces in the id survive
        let back = GeometryMemory::from_text(&m.to_text());
        assert_eq!(back, m);
    }

    #[test]
    fn malformed_lines_are_skipped() {
        let text = "10 20 300 200 foot\ngarbage\n1 2 3 nope\n5 5 0 0 zero\n\n0 0 640 480 firefox\n";
        let m = GeometryMemory::from_text(text);
        assert_eq!(m.len(), 2);
        assert_eq!(m.recall("foot"), Some(rect(10, 20, 300, 200)));
        assert_eq!(m.recall("firefox"), Some(rect(0, 0, 640, 480)));
    }

    #[test]
    fn fit_inside_moves_offscreen_window_back() {
        let area = rect(0, 32, 1440, 868);
        // Hardware run 2026-10-07: window dropped near the bottom edge, restored to y=893.
        assert_eq!(fit_inside(rect(518, 893, 696, 494), area), rect(518, 406, 696, 494));
        assert_eq!(fit_inside(rect(-300, 0, 696, 494), area), rect(0, 32, 696, 494));
        assert_eq!(fit_inside(rect(1200, 500, 696, 494), area), rect(744, 406, 696, 494));
    }

    #[test]
    fn fit_inside_keeps_windows_that_already_fit() {
        let area = rect(0, 32, 1440, 868);
        assert_eq!(fit_inside(rect(100, 100, 696, 494), area), rect(100, 100, 696, 494));
    }

    #[test]
    fn fit_inside_shrinks_oversized() {
        let area = rect(0, 32, 1440, 868);
        assert_eq!(fit_inside(rect(50, 50, 2000, 1200), area), area);
    }

    #[test]
    fn remembered_relative_to_the_monitor() {
        // Closed on VGA-1, the right monitor at x 1366 (GT 710, 2026-10-10): remembered as 253,224.
        let on_vga = rect(1619, 224, 975, 555);
        let kept = moved_between(on_vga, (1366, 0).into(), (0, 0).into());
        assert_eq!(kept, rect(253, 224, 975, 555));
        // Opened with the pointer on VGA-1 again: same spot; on HDMI-A-1 (x 0): same spot there.
        assert_eq!(moved_between(kept, (0, 0).into(), (1366, 0).into()), on_vga);
        assert_eq!(moved_between(kept, (0, 0).into(), (0, 0).into()), kept);
    }

    #[test]
    fn save_and_load_file() {
        let dir = std::env::temp_dir().join(format!("kompact-geom-test-{}", std::process::id()));
        let path = dir.join("nested").join("geometry");
        let mut m = GeometryMemory::default();
        m.remember("foot", rect(10, 40, 800, 500));
        save(&path, &m).unwrap();
        assert_eq!(load(&path), m);
        assert!(!path.with_extension("tmp").exists());
        std::fs::remove_dir_all(&dir).unwrap();
        // Missing file: empty, no panic.
        assert_eq!(load(&path).len(), 0);
    }
}
