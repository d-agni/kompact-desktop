//! 6p-d: which clients make kompact copy pixels to the GPU, and how many.
//!
//! kompact offers no linux-dmabuf, so every client (the shell's WebKit pages, Firefox, GTK apps)
//! draws into shared memory (`wl_shm`), and each part of a buffer a client marks as damaged has
//! to be uploaded to a GL texture before it can be drawn. On the GT 710 that is a likely cost
//! behind the missed frames, but which client causes it wasn't visible. The commit handler adds
//! each commit's new damage here, by owner (layer-shell namespace, app-id, or "other" for
//! popups, cursors and the like), and the stats timer logs and clears it:
//!
//! `uploads: mpx=31.2 shm=100% dock=24.0/310 panel=4.1/12 firefox_firefox=3.1/55 …`
//!
//! (megapixels damaged in the interval, then per owner megapixels/commits, largest first.)
//! It counts damage as committed: several commits between two frames are uploaded together,
//! so this is an upper bound for what is uploaded. Measuring only, nothing is changed.

use std::collections::BTreeMap;

#[derive(Default)]
pub struct Meter {
    by_owner: BTreeMap<String, (u64, u32)>,
    shm_px: u64,
}

/// Pixels covered by damage rectangles `(w, h)`; overlaps count twice (it's an upper bound).
pub fn pixels(rects: impl IntoIterator<Item = (i32, i32)>) -> u64 {
    rects.into_iter().map(|(w, h)| w.max(0) as u64 * h.max(0) as u64).sum()
}

impl Meter {
    pub fn add(&mut self, owner: &str, px: u64, shm: bool) {
        if px == 0 {
            return;
        }
        let e = self.by_owner.entry(owner.to_string()).or_default();
        e.0 += px;
        e.1 += 1;
        if shm {
            self.shm_px += px;
        }
    }

    /// The log line for the interval, and start a new one. `None` if nothing was damaged.
    pub fn take_line(&mut self) -> Option<String> {
        let total: u64 = self.by_owner.values().map(|(px, _)| px).sum();
        if total == 0 {
            return None;
        }
        let mpx = |px: u64| px as f64 / 1_000_000.0;
        let mut owners: Vec<_> = std::mem::take(&mut self.by_owner).into_iter().collect();
        owners.sort_by(|a, b| b.1 .0.cmp(&a.1 .0).then(a.0.cmp(&b.0)));
        let mut line = format!("uploads: mpx={:.1} shm={}%", mpx(total), self.shm_px * 100 / total);
        for (owner, (px, commits)) in owners {
            line.push_str(&format!(" {owner}={:.1}/{commits}", mpx(px)));
        }
        self.shm_px = 0;
        Some(line)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_rectangles() {
        assert_eq!(pixels([(10, 10), (5, 2)]), 110);
        assert_eq!(pixels([(-1, 10)]), 0);
        assert_eq!(pixels([]), 0);
    }

    #[test]
    fn line_largest_first_then_cleared() {
        let mut m = Meter::default();
        assert_eq!(m.take_line(), None);
        m.add("panel", 100_000, true);
        m.add("dock", 2_000_000, true);
        m.add("dock", 1_000_000, true);
        m.add("foot", 900_000, false);
        m.add("idle", 0, true); // an empty commit isn't counted
        assert_eq!(m.take_line().as_deref(), Some("uploads: mpx=4.0 shm=77% dock=3.0/2 foot=0.9/1 panel=0.1/1"));
        assert_eq!(m.take_line(), None);
    }
}
