//! 19k: which window you mean. "The Firefox on the left", "the one in the back", "the other
//! Firefox", "the newest terminal", "all Firefox windows", "Firefox, which is put on the left
//! side": a name (or none) and how to tell windows apart, read from your words (`WinRef`), then
//! matched against where the windows are (`pick`, with kompact's `layout`: each window's place,
//! and the window list's order, bottom to top). Pure and tested.

use super::run::{best_window, score, WinCand};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    Left,
    Right,
    Top,
    Bottom,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pick {
    /// As before: the best match, the focused one first.
    Best,
    /// In front (on top).
    Front,
    /// Behind the others.
    Back,
    /// Not the one in front (or not the one named last).
    Other,
    Newest,
    Oldest,
    Biggest,
    Smallest,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WinRef {
    /// The app or title, empty for "the one on the left".
    pub name: String,
    pub side: Option<Side>,
    pub pick: Pick,
    /// "all", "every", "both".
    pub all: bool,
}

impl WinRef {
    /// Whether it says more than a name.
    pub fn qualified(&self) -> bool {
        self.side.is_some() || self.pick != Pick::Best || self.all
    }
}

/// Words that only join the description ("which is put on the …").
const FILLER: [&str; 30] = [
    "which", "that", "thats", "is", "are", "was", "put", "placed", "sitting", "located", "kept", "open", "opened", "on", "in",
    "at", "of", "to", "the", "a", "an", "one", "ones", "side", "half", "part", "screen", "window", "windows", "app",
];

/// Your words for a window, read apart.
pub fn parse(query: &str) -> WinRef {
    let q = super::rules::normalize(query);
    let words: Vec<&str> = q.split_whitespace().collect();
    let mut r = WinRef { name: String::new(), side: None, pick: Pick::Best, all: false };
    let mut name: Vec<&str> = Vec::new();
    let mut i = 0;
    while i < words.len() {
        let w = words[i];
        match w {
            "left" | "leftmost" => r.side = Some(Side::Left),
            "right" | "rightmost" => r.side = Some(Side::Right),
            // "on top" is in front; "at the top" / "top half" is the top.
            "top" if words.get(i.wrapping_sub(1)) == Some(&"on") => r.pick = Pick::Front,
            "top" | "upper" => r.side = Some(Side::Top),
            "bottom" | "lower" => r.side = Some(Side::Bottom),
            "front" | "foreground" | "focused" | "active" | "current" => r.pick = Pick::Front,
            "back" | "behind" | "background" | "hidden" | "underneath" => r.pick = Pick::Back,
            "other" | "another" | "second" => r.pick = Pick::Other,
            "new" | "newest" | "latest" | "recent" => r.pick = Pick::Newest,
            "last" => r.pick = Pick::Newest,
            "old" | "oldest" | "first" | "earliest" => r.pick = Pick::Oldest,
            "big" | "bigger" | "biggest" | "large" | "larger" | "largest" => r.pick = Pick::Biggest,
            "small" | "smaller" | "smallest" | "little" => r.pick = Pick::Smallest,
            "all" | "every" | "each" | "both" | "everything" => r.all = true,
            // 20b: "all applications" is all windows.
            "apps" | "applications" | "programs" if r.all => {}
            w if FILLER.contains(&w) => {}
            w => name.push(w),
        }
        i += 1;
    }
    r.name = name.join(" ");
    r
}

/// A window with where it is: `(x, y, w, h)` in global coordinates, and its place in the stack
/// (0 at the bottom).
#[derive(Debug, Clone)]
pub struct Placed<'a> {
    pub win: &'a WinCand,
    pub rect: Option<(i32, i32, i32, i32)>,
    pub stack: usize,
}

/// The windows `r` means: all of them for "all", else one (none: nothing fits). `screen` is the
/// monitor's `(x, y, w, h)`, `last` the window named last in this conversation (for "the other
/// one" without a name).
pub fn pick<'a>(r: &WinRef, windows: &[Placed<'a>], screen: (i32, i32, i32, i32), last: Option<&str>) -> Vec<&'a WinCand> {
    let name = if r.name.is_empty() && r.pick == Pick::Other { last.unwrap_or("") } else { r.name.as_str() };
    // By name (any that fits), or all windows that aren't minimized.
    let mut c: Vec<&Placed> = if name.is_empty() {
        windows.iter().filter(|p| !p.win.minimized).collect()
    } else {
        let all: Vec<WinCand> = windows.iter().map(|p| p.win.clone()).collect();
        windows.iter().filter(|p| score(name, &p.win.name, &p.win.app_id, &p.win.about, &p.win.title) > 0 || best_window(name, &all).is_some_and(|b| b.id == p.win.id)).collect()
    };
    if let Some(side) = r.side {
        let (sx, sy, sw, sh) = screen;
        let (mx, my) = (sx + sw / 2, sy + sh / 2);
        c.retain(|p| {
            let Some((x, y, w, h)) = p.rect else { return false };
            let (cx, cy) = (x + w / 2, y + h / 2);
            match side {
                Side::Left => cx < mx,
                Side::Right => cx >= mx,
                Side::Top => cy < my,
                Side::Bottom => cy >= my,
            }
        });
        // Leftmost first for "left", and so on, so one pick takes the most that side.
        c.sort_by_key(|p| {
            let (x, y, w, h) = p.rect.unwrap_or_default();
            match side {
                Side::Left => x,
                Side::Right => -(x + w),
                Side::Top => y,
                Side::Bottom => -(y + h),
            }
        });
    }
    if r.all {
        return c.into_iter().map(|p| p.win).collect();
    }
    let area = |p: &Placed| p.rect.map(|(_, _, w, h)| i64::from(w) * i64::from(h)).unwrap_or(0);
    let front = |list: &[&Placed<'a>]| list.iter().filter(|p| !p.win.minimized).max_by_key(|p| p.stack).map(|p| p.win);
    let one = match r.pick {
        Pick::Best if r.side.is_some() => c.first().map(|p| p.win),
        Pick::Best => {
            let list: Vec<WinCand> = c.iter().map(|p| p.win.clone()).collect();
            best_window(name, &list).and_then(|b| c.iter().find(|p| p.win.id == b.id)).map(|p| p.win)
        }
        Pick::Front => front(&c),
        Pick::Back => c.iter().filter(|p| !p.win.minimized).min_by_key(|p| p.stack).map(|p| p.win),
        Pick::Other => {
            let top = front(&c).map(|w| w.id);
            c.iter().filter(|p| Some(p.win.id) != top).max_by_key(|p| p.stack).map(|p| p.win)
        }
        Pick::Newest => c.iter().max_by_key(|p| p.win.id).map(|p| p.win),
        Pick::Oldest => c.iter().min_by_key(|p| p.win.id).map(|p| p.win),
        Pick::Biggest => c.iter().max_by_key(|p| area(p)).map(|p| p.win),
        Pick::Smallest => c.iter().min_by_key(|p| area(p)).map(|p| p.win),
    };
    one.into_iter().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn w(id: u64, name: &str, focused: bool) -> WinCand {
        WinCand { id, name: name.into(), app_id: name.to_lowercase(), title: format!("{name} window"), about: String::new(), focused, minimized: false }
    }

    #[test]
    fn reading_the_words() {
        let r = |q: &str| parse(q);
        // The user's own request, 2026-10-24.
        assert_eq!(r("Firefox, which is put on the left side"), WinRef { name: "firefox".into(), side: Some(Side::Left), pick: Pick::Best, all: false });
        assert_eq!(r("the firefox which is in back").pick, Pick::Back);
        assert_eq!(r("the one on the right"), WinRef { name: String::new(), side: Some(Side::Right), pick: Pick::Best, all: false });
        assert_eq!(r("the other firefox").pick, Pick::Other);
        assert!(r("all firefox windows").all);
        assert_eq!(r("all firefox windows").name, "firefox");
        assert_eq!(r("the window on top").pick, Pick::Front, "on top: in front");
        assert_eq!(r("the terminal at the top").side, Some(Side::Top));
        assert_eq!(r("the newest terminal").pick, Pick::Newest);
        assert_eq!(r("text editor").name, "text editor");
        assert!(!r("firefox").qualified());
        assert!(r("left firefox").qualified());
    }

    #[test]
    fn picking_the_window() {
        // Two Firefox windows side by side (left one in front), a terminal behind, on 1366×768.
        let fl = w(3, "Firefox", true);
        let fr = w(5, "Firefox", false);
        let t = w(7, "Terminal", false);
        let screen = (0, 0, 1366, 768);
        let placed = vec![
            Placed { win: &t, rect: Some((100, 100, 600, 400)), stack: 0 },
            Placed { win: &fr, rect: Some((683, 28, 683, 740)), stack: 1 },
            Placed { win: &fl, rect: Some((0, 28, 683, 740)), stack: 2 },
        ];
        let ids = |q: &str| pick(&parse(q), &placed, screen, Some("firefox")).iter().map(|w| w.id).collect::<Vec<_>>();
        assert_eq!(ids("Firefox, which is put on the left side"), [3]);
        assert_eq!(ids("the firefox on the right"), [5]);
        assert_eq!(ids("firefox"), [3], "no description: as before, the focused one");
        assert_eq!(ids("the other firefox"), [5]);
        assert_eq!(ids("the other one"), [5], "the app named last");
        assert_eq!(ids("the one on the left"), [3], "the leftmost window of any app");
        assert_eq!(ids("all firefox windows").len(), 2);
        assert_eq!(ids("the window in the back"), [7]);
        assert_eq!(ids("the newest window"), [7]);
        assert_eq!(ids("the biggest firefox").len(), 1);
        assert!(ids("the spotify on the left").is_empty(), "nothing fits");
    }
}
