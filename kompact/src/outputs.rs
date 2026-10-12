//! Outputs (monitors) on the DRM backend (Phase 5): which connectors to drive, where each output
//! sits in the layout, and where windows go when outputs come and go. Pure logic; the backend
//! does the KMS calls and `shell.rs` moves the windows.
//!
//! What the backend does is chosen at startup with `KOMPACT_OUTPUTS`, so each Phase 5 step could be
//! tried on hardware without a rebuild and `first` is always a way back. `all` is the default
//! since 5f (confirmed with two monitors on the GT 710, 2026-10-10):
//!
//! | value | behaviour |
//! |---|---|
//! | `first` | one connector (the primary, 5e), set up once at startup, no hotplug: what kompact did before Phase 5 |
//! | `observe` (5a) | as `first`, plus watch udev for hotplug and log every connector's state; no KMS change |
//! | `reconnect` (5b) | one output; when its monitor is unplugged the output is shut down, when a monitor is plugged back in it's set up again |
//! | `all` (default, 5c/5d/5f) | every connected connector, side by side left to right; hotplug adds/removes outputs and windows move off removed ones |

use smithay::utils::{Logical, Point, Rectangle, Size};

use crate::geometry_memory::fit_inside;

type Rect = Rectangle<i32, Logical>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    First,
    Observe,
    Reconnect,
    All,
}

impl Mode {
    /// `KOMPACT_OUTPUTS`. `None` = unset (`all` since 5f); `Err` = not a known value.
    pub fn from_env(value: Option<&str>) -> Result<Mode, String> {
        match value.map(str::trim).map(str::to_ascii_lowercase).as_deref() {
            None | Some("") | Some("all") => Ok(Mode::All),
            Some("first") => Ok(Mode::First),
            Some("observe") => Ok(Mode::Observe),
            Some("reconnect") => Ok(Mode::Reconnect),
            Some(other) => Err(other.to_string()),
        }
    }

    /// Listen to udev for connector changes (everything but `first`).
    pub fn watches_hotplug(self) -> bool {
        self != Mode::First
    }

    /// Set outputs up and shut them down on hotplug (`observe` only logs).
    pub fn drives_hotplug(self) -> bool {
        matches!(self, Mode::Reconnect | Mode::All)
    }

    pub fn max_outputs(self) -> usize {
        match self {
            Mode::All => usize::MAX,
            _ => 1,
        }
    }

    /// A DRM error on one output shuts just that output down (and it's tried again on the next
    /// hotplug event) instead of stopping kompact. In `first`/`observe` a DRM error stays fatal,
    /// as it has been since Phase 0.
    pub fn survives_output_errors(self) -> bool {
        self.drives_hotplug()
    }
}

/// A video mode: width, height (pixels) and refresh (mHz).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VideoMode {
    pub w: i32,
    pub h: i32,
    pub refresh_mhz: u32,
}

/// What a connector scan found about one connector.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Connector {
    /// The DRM connector handle (as a number).
    pub id: u32,
    pub name: String,
    pub connected: bool,
    /// The mode kompact would use (the monitor's preferred one), if it reports any.
    pub mode: Option<VideoMode>,
    /// CRTCs that can drive this connector, best first (the one it's on now, if any).
    pub crtcs: Vec<u32>,
}

/// An output kompact is driving.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Driven {
    pub connector: u32,
    pub crtc: u32,
    pub mode: VideoMode,
}

/// What to change after a scan: shut these connectors' outputs down, then set these up.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Plan {
    pub remove: Vec<u32>,
    /// (connector, crtc)
    pub add: Vec<(u32, u32)>,
}

impl Plan {
    pub fn is_empty(&self) -> bool {
        self.remove.is_empty() && self.add.is_empty()
    }
}

/// Compare what's driven with what a scan found.
///
/// - An output whose connector is gone, disconnected, or now wants a different mode (another
///   monitor on the same port) is removed; in the last case it's set up again with the new mode.
/// - Connected connectors with a mode are added in scan order until `max` outputs are driven,
///   each on the first of its CRTCs not used by another output. `skip` are connectors that failed
///   to come up since the last hotplug event; they aren't retried until the next one.
pub fn plan(driven: &[Driven], scan: &[Connector], max: usize, skip: &[u32]) -> Plan {
    let mut plan = Plan::default();
    let mut kept: Vec<Driven> = Vec::new();
    for d in driven {
        let still = scan
            .iter()
            .find(|c| c.id == d.connector)
            .is_some_and(|c| c.connected && c.mode == Some(d.mode));
        if still {
            kept.push(*d);
        } else {
            plan.remove.push(d.connector);
        }
    }
    let mut used: Vec<u32> = kept.iter().map(|d| d.crtc).collect();
    let mut count = kept.len();
    for c in scan {
        if count >= max {
            break;
        }
        if !c.connected || c.mode.is_none() || skip.contains(&c.id) {
            continue;
        }
        if kept.iter().any(|d| d.connector == c.id) {
            continue;
        }
        let Some(&crtc) = c.crtcs.iter().find(|crtc| !used.contains(crtc)) else { continue };
        used.push(crtc);
        plan.add.push((c.id, crtc));
        count += 1;
    }
    plan
}

/// Connector names compared loosely: kompact calls them `HDMIA-1`, the kernel and xrandr `HDMI-A-1`.
pub fn same_name(a: &str, b: &str) -> bool {
    let norm = |s: &str| s.chars().filter(|c| !matches!(c, '-' | '_' | ' ')).flat_map(char::to_lowercase).collect::<String>();
    norm(a) == norm(b)
}

/// Why `pick_primary` chose the monitor it did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrimaryChoice {
    /// `KOMPACT_PRIMARY` named it.
    Named,
    /// Detected: the largest mode (then the highest refresh, then connector order).
    Largest,
    /// `KOMPACT_PRIMARY` named a monitor that isn't connected; the largest was taken instead.
    NamedMissing,
}

/// The main monitor among the connected ones (name, mode), in connector order: the one
/// `wanted` (`KOMPACT_PRIMARY`; unset, empty or `auto` = detect) names, else the largest.
/// kompact starts on it and it's the one `perf.rs` times; the layout order doesn't change.
pub fn pick_primary(monitors: &[(&str, VideoMode)], wanted: Option<&str>) -> Option<(usize, PrimaryChoice)> {
    let wanted = wanted.map(str::trim).filter(|w| !w.is_empty() && !w.eq_ignore_ascii_case("auto"));
    if let Some(w) = wanted {
        if let Some(i) = monitors.iter().position(|(name, _)| same_name(name, w)) {
            return Some((i, PrimaryChoice::Named));
        }
    }
    let largest = (0..monitors.len()).max_by_key(|&i| {
        let m = monitors[i].1;
        (m.w as i64 * m.h as i64, m.refresh_mhz, std::cmp::Reverse(i))
    })?;
    Some((largest, if wanted.is_some() { PrimaryChoice::NamedMissing } else { PrimaryChoice::Largest }))
}

/// Positions for outputs of these sizes, in order: left to right, tops aligned at y = 0.
pub fn layout(sizes: &[Size<i32, Logical>]) -> Vec<Point<i32, Logical>> {
    let mut x = 0;
    sizes
        .iter()
        .map(|s| {
            let p = Point::from((x, 0));
            x += s.w;
            p
        })
        .collect()
}

/// 12h-1: the arrangement made in Settings → Displays: where each monitor's top-left goes, by
/// name. Kept in `~/.config/kompact/displays`, one `<name> <x> <y>` line each; monitors that
/// aren't connected keep their line for when they come back. 12h-2: also `primary <name>` (the
/// main display: menu bar, dock) and `menu-bars every|primary`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Arrangement {
    pub places: Vec<(String, Point<i32, Logical>)>,
    /// 12h-2: the main display chosen in Settings (`None`: `pick_main` decides).
    pub primary: Option<String>,
    /// 12h-2: a menu bar on every display, not only the main one.
    pub every_menu_bar: bool,
}

/// A monitor shares at least this much of an edge with its neighbour (or all of its shorter
/// side), so the pointer can cross.
pub const MIN_SHARED: i32 = 100;
/// A dragged monitor's edge this close to its neighbour's same edge lines up with it.
pub const ALIGN: i32 = 40;

impl Arrangement {
    pub fn parse(text: &str) -> Arrangement {
        let mut a = Arrangement::default();
        for line in text.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('#')) {
            let words: Vec<&str> = line.split_whitespace().collect();
            match words[..] {
                ["primary", name] if name.len() <= 32 => a.primary = Some(name.to_string()),
                ["menu-bars", "every"] => a.every_menu_bar = true,
                ["menu-bars", "primary"] => a.every_menu_bar = false,
                _ => {}
            }
            let [name, x, y] = words[..] else { continue };
            let (Ok(x), Ok(y)) = (x.parse::<i32>(), y.parse::<i32>()) else { continue };
            if name.len() <= 32 && x.abs() <= 100_000 && y.abs() <= 100_000 {
                a.set(name, Point::from((x, y)));
            }
        }
        a
    }

    pub fn to_text(&self) -> String {
        let mut out = String::from("# kompact display arrangement (written by System Settings → Displays): <monitor> <x> <y>\n");
        for (name, p) in &self.places {
            out.push_str(&format!("{name} {} {}\n", p.x, p.y));
        }
        if let Some(p) = &self.primary {
            out.push_str(&format!("primary {p}\n"));
        }
        out.push_str(if self.every_menu_bar { "menu-bars every\n" } else { "menu-bars primary\n" });
        out
    }

    pub fn get(&self, name: &str) -> Option<Point<i32, Logical>> {
        self.places.iter().find(|(n, _)| n == name).map(|(_, p)| *p)
    }

    pub fn set(&mut self, name: &str, p: Point<i32, Logical>) {
        match self.places.iter_mut().find(|(n, _)| n == name) {
            Some(entry) => entry.1 = p,
            None => self.places.push((name.to_string(), p)),
        }
    }
}

/// `~/.config/kompact/displays`.
fn arrangement_path() -> Option<std::path::PathBuf> {
    let config = std::env::var_os("XDG_CONFIG_HOME").map(std::path::PathBuf::from);
    let home = std::env::var_os("HOME").map(std::path::PathBuf::from);
    crate::prefs::path(config.as_deref(), home.as_deref()).map(|p| p.with_file_name("displays"))
}

/// The saved arrangement (none: side by side, as before 12h).
pub fn load_arrangement() -> Arrangement {
    let a = arrangement_path().and_then(|p| std::fs::read_to_string(p).ok()).map(|t| Arrangement::parse(&t)).unwrap_or_default();
    if a != Arrangement::default() {
        tracing::info!("displays: arrangement {:?}, main display {:?}, menu bar on every display: {}", a.places, a.primary, a.every_menu_bar);
    }
    a
}

pub fn save_arrangement(a: &Arrangement) -> Result<std::path::PathBuf, String> {
    let path = arrangement_path().ok_or("no home directory")?;
    crate::keymap::save_to(&path, &a.to_text())?;
    Ok(path)
}

/// 12h-2: the main display among `outputs` (name, size): the one chosen in Settings, else
/// `KOMPACT_PRIMARY`'s, else the largest (5e's rule, so it's the backend's primary by default).
pub fn pick_main(outputs: &[(&str, Size<i32, Logical>)], chosen: Option<&str>, env: Option<&str>) -> Option<usize> {
    let named = |w: Option<&str>| w.and_then(|w| outputs.iter().position(|(n, _)| same_name(n, w)));
    named(chosen).or_else(|| named(env)).or_else(|| {
        (0..outputs.len()).max_by_key(|&i| (outputs[i].1.w as i64 * outputs[i].1.h as i64, std::cmp::Reverse(i)))
    })
}

/// 12h-2: the output a new menu bar (one that asks for no output) goes on. With a menu bar on
/// every display: the main display first, then each other output in order that has none yet; all
/// taken: the main one. With one menu bar: always the main display. (The shell drops the extra
/// bars and remaps its one at the same time, and an extra one can still be there when the
/// remapped one arrives: it mustn't count.)
pub fn next_menu_bar(names: &[&str], main: Option<usize>, taken: &[&str], every: bool) -> Option<usize> {
    let main = main.filter(|&m| m < names.len()).or((!names.is_empty()).then_some(0))?;
    if !every {
        return Some(main);
    }
    std::iter::once(main).chain((0..names.len()).filter(|&i| i != main)).find(|&i| !taken.contains(&names[i])).or(Some(main))
}

/// Overlapping, not just touching.
fn overlap(a: Rect, b: Rect) -> bool {
    a.loc.x < b.loc.x + b.size.w && b.loc.x < a.loc.x + a.size.w && a.loc.y < b.loc.y + b.size.h && b.loc.y < a.loc.y + a.size.h
}

/// Edge to edge, sharing enough of it (`MIN_SHARED`).
fn touches(a: Rect, b: Rect) -> bool {
    let shared = |a0: i32, al: i32, b0: i32, bl: i32| (a0 + al).min(b0 + bl) - a0.max(b0);
    let side = (a.loc.x + a.size.w == b.loc.x || b.loc.x + b.size.w == a.loc.x)
        && shared(a.loc.y, a.size.h, b.loc.y, b.size.h) >= MIN_SHARED.min(a.size.h).min(b.size.h);
    let stacked = (a.loc.y + a.size.h == b.loc.y || b.loc.y + b.size.h == a.loc.y)
        && shared(a.loc.x, a.size.w, b.loc.x, b.size.w) >= MIN_SHARED.min(a.size.w).min(b.size.w);
    side || stacked
}

/// Right of the monitor reaching furthest right, tops aligned (or 0,0 for the first).
fn right_of_all(placed: &[Rect]) -> Point<i32, Logical> {
    placed
        .iter()
        .max_by_key(|r| r.loc.x + r.size.w)
        .map(|r| Point::from((r.loc.x + r.size.w, r.loc.y)))
        .unwrap_or_default()
}

/// Where a monitor of `size` dragged to `wanted` goes: the nearest spot edge to edge with one of
/// `others` (sharing `MIN_SHARED`, lined up with its edges within `ALIGN`) that overlaps none of
/// them. With no others it stays where it was dropped.
pub fn snap(size: Size<i32, Logical>, wanted: Point<i32, Logical>, others: &[Rect]) -> Point<i32, Logical> {
    if others.is_empty() {
        return wanted;
    }
    // Along `o`'s edge: kept sharing `need` of it, and lined up with its start or end if close.
    let along = |v: i32, len: i32, o0: i32, ol: i32| {
        let need = MIN_SHARED.min(len).min(ol);
        let v = v.clamp(o0 - len + need, o0 + ol - need);
        if (v - o0).abs() <= ALIGN {
            o0
        } else if (v + len - (o0 + ol)).abs() <= ALIGN {
            o0 + ol - len
        } else {
            v
        }
    };
    let mut best: Option<(i64, Point<i32, Logical>)> = None;
    for o in others {
        let y = along(wanted.y, size.h, o.loc.y, o.size.h);
        let x = along(wanted.x, size.w, o.loc.x, o.size.w);
        let spots = [
            (o.loc.x - size.w, y),
            (o.loc.x + o.size.w, y),
            (x, o.loc.y - size.h),
            (x, o.loc.y + o.size.h),
        ];
        for (x, y) in spots {
            let p = Point::from((x, y));
            if others.iter().any(|r| overlap(Rectangle::new(p, size), *r)) {
                continue;
            }
            let (dx, dy) = ((x - wanted.x) as i64, (y - wanted.y) as i64);
            let d = dx * dx + dy * dy;
            if best.is_none_or(|(b, _)| d < b) {
                best = Some((d, p));
            }
        }
    }
    best.map(|(_, p)| p).unwrap_or_else(|| right_of_all(others))
}

/// 12h-1: positions for the outputs `(name, size)`, in order, as `saved` says where it can: the
/// saved spots that don't overlap and touch the rest (starting from the first saved one), then
/// any other saved one snapped to them from its spot, then the unsaved ones to the right. The
/// whole is moved so its top-left is 0,0. With nothing saved it's `layout`.
pub fn arrange(outputs: &[(&str, Size<i32, Logical>)], saved: &Arrangement) -> Vec<Point<i32, Logical>> {
    let mut placed: Vec<Option<Rect>> = vec![None; outputs.len()];
    let wanted: Vec<Option<Rect>> = outputs.iter().map(|(n, s)| saved.get(n).map(|p| Rectangle::new(p, *s))).collect();
    let done = |placed: &[Option<Rect>]| placed.iter().flatten().copied().collect::<Vec<Rect>>();
    // Saved spots, grown from the first one: each that touches what's placed and overlaps none.
    if let Some(first) = wanted.iter().position(Option::is_some) {
        placed[first] = wanted[first];
        loop {
            let now = done(&placed);
            let next = (0..outputs.len()).find(|&i| {
                placed[i].is_none()
                    && wanted[i].is_some_and(|r| !now.iter().any(|q| overlap(r, *q)) && now.iter().any(|q| touches(r, *q)))
            });
            let Some(i) = next else { break };
            placed[i] = wanted[i];
        }
    }
    // Saved ones that didn't fit: snapped from where they were saved. Then the new ones.
    for i in 0..outputs.len() {
        if placed[i].is_none() {
            if let Some(r) = wanted[i] {
                placed[i] = Some(Rectangle::new(snap(r.size, r.loc, &done(&placed)), r.size));
            }
        }
    }
    for i in 0..outputs.len() {
        if placed[i].is_none() {
            placed[i] = Some(Rectangle::new(right_of_all(&done(&placed)), outputs[i].1));
        }
    }
    let rects = done(&placed);
    let min_x = rects.iter().map(|r| r.loc.x).min().unwrap_or(0);
    let min_y = rects.iter().map(|r| r.loc.y).min().unwrap_or(0);
    rects.iter().map(|r| Point::from((r.loc.x - min_x, r.loc.y - min_y))).collect()
}

/// Where a window at `win` goes after the outputs changed. `moves` is every output from before
/// the change with where it is now (`None`: removed). A window on an output that moved follows
/// it; one on a removed output goes to the same spot on `fallback` (the first remaining output),
/// kept inside it. `None` = leave it where it is (its output didn't move, or there's nowhere to
/// go).
pub fn relocate(win: Rect, moves: &[(Rect, Option<Rect>)], fallback: Option<Rect>) -> Option<Point<i32, Logical>> {
    let center = win.loc + Point::from((win.size.w / 2, win.size.h / 2));
    let (old, new) = moves.iter().find(|(old, _)| old.contains(center))?;
    match new {
        Some(new) if new.loc == old.loc => None,
        Some(new) => Some(win.loc + (new.loc - old.loc)),
        None => {
            let target = fallback?;
            let shifted = Rectangle::new(win.loc + (target.loc - old.loc), win.size);
            Some(fit_inside(shifted, target).loc)
        }
    }
}

/// Where a floating window at `win` goes when a panel starts reserving space (Phase 6a): the
/// usable area of the output it's on went from `old` to `new`. Only edges that moved inward
/// count, and only a window reaching into that newly reserved strip moves, just far enough to
/// clear it (not past the far edge of `new`). The size stays; `None` = no move needed.
pub fn clear_of_reserved(win: Rect, old: Rect, new: Rect) -> Option<Point<i32, Logical>> {
    let (mut x, mut y) = (win.loc.x, win.loc.y);
    let (old_r, old_b) = (old.loc.x + old.size.w, old.loc.y + old.size.h);
    let (new_r, new_b) = (new.loc.x + new.size.w, new.loc.y + new.size.h);
    // Never past the area's top or left doing so: a window taller than what's left keeps its
    // title bar out from under a top panel (it overhangs the new bottom panel instead).
    if new_b < old_b && y + win.size.h > new_b {
        y = (new_b - win.size.h).max(new.loc.y.min(y));
    }
    if new_r < old_r && x + win.size.w > new_r {
        x = (new_r - win.size.w).max(new.loc.x.min(x));
    }
    // Top and left win over bottom and right: the title bar must stay reachable.
    if new.loc.y > old.loc.y && y < new.loc.y {
        y = new.loc.y;
    }
    if new.loc.x > old.loc.x && x < new.loc.x {
        x = new.loc.x;
    }
    let to = Point::from((x, y));
    (to != win.loc).then_some(to)
}

/// Keep the pointer on an output. A move that lands on any output is fine (that's how it
/// crosses from one to the next); one that doesn't is clamped to the output the pointer is on
/// (or the first output, if it's on none).
pub fn clamp_pointer(raw: Point<f64, Logical>, current: Point<f64, Logical>, outputs: &[Rect]) -> Point<f64, Logical> {
    let inside = |r: &Rect, p: Point<f64, Logical>| {
        p.x >= r.loc.x as f64
            && p.y >= r.loc.y as f64
            && p.x < (r.loc.x + r.size.w) as f64
            && p.y < (r.loc.y + r.size.h) as f64
    };
    if outputs.is_empty() || outputs.iter().any(|r| inside(r, raw)) {
        return raw;
    }
    let r = outputs.iter().find(|r| inside(r, current)).unwrap_or(&outputs[0]);
    Point::from((
        raw.x.clamp(r.loc.x as f64, (r.loc.x + r.size.w - 1) as f64),
        raw.y.clamp(r.loc.y as f64, (r.loc.y + r.size.h - 1) as f64),
    ))
}

/// 5i: a window kompact moved off a monitor that was unplugged. `rect` is where it was,
/// relative to that monitor (`from`); `at` is where kompact put it, relative to the monitor it
/// went to (`on`). Names are output names, so the same monitor coming back is recognised.
#[derive(Debug, Clone, PartialEq)]
pub struct Displaced {
    pub from: String,
    pub rect: Rect,
    pub on: String,
    pub at: Point<i32, Logical>,
}

/// What to do with a displaced window after the outputs changed.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Return {
    /// Its monitor isn't back yet; keep remembering.
    Wait,
    /// Its monitor is back: move it here (global), and forget it.
    Back(Point<i32, Logical>),
    /// The user moved it since (or it's gone somewhere else): leave it, and forget it.
    Forget,
}

/// `now` is the window's output name and position relative to it; `outputs` every output now
/// (name, global geometry). A window still where kompact put it goes back to the same spot on
/// its returning monitor, kept inside it; one the user moved stays where the user put it.
pub fn return_home(d: &Displaced, now: Option<(&str, Point<i32, Logical>)>, outputs: &[(&str, Rect)]) -> Return {
    let untouched = now.is_some_and(|(on, at)| on == d.on && at == d.at);
    if !untouched {
        return Return::Forget;
    }
    match outputs.iter().find(|(name, _)| *name == d.from) {
        Some((_, home)) => Return::Back(fit_inside(Rectangle::new(d.rect.loc + home.loc, d.rect.size), *home).loc),
        None => Return::Wait,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const HD: VideoMode = VideoMode { w: 1920, h: 1080, refresh_mhz: 60_000 };
    const WX: VideoMode = VideoMode { w: 1440, h: 900, refresh_mhz: 59_887 };

    fn conn(id: u32, connected: bool, mode: Option<VideoMode>, crtcs: &[u32]) -> Connector {
        Connector { id, name: format!("C-{id}"), connected, mode, crtcs: crtcs.to_vec() }
    }

    fn rect(x: i32, y: i32, w: i32, h: i32) -> Rect {
        Rectangle::new((x, y).into(), (w, h).into())
    }

    #[test]
    fn mode_parsing() {
        // 5f: every monitor unless told otherwise.
        assert_eq!(Mode::from_env(None), Ok(Mode::All));
        assert_eq!(Mode::from_env(Some("")), Ok(Mode::All));
        assert_eq!(Mode::from_env(Some("first")), Ok(Mode::First));
        assert_eq!(Mode::from_env(Some(" Observe ")), Ok(Mode::Observe));
        assert_eq!(Mode::from_env(Some("reconnect")), Ok(Mode::Reconnect));
        assert_eq!(Mode::from_env(Some("ALL")), Ok(Mode::All));
        assert_eq!(Mode::from_env(Some("both")), Err("both".to_string()));
        assert!(!Mode::First.watches_hotplug());
        assert!(Mode::Observe.watches_hotplug() && !Mode::Observe.drives_hotplug());
        assert!(!Mode::First.survives_output_errors() && !Mode::Observe.survives_output_errors());
        assert_eq!((Mode::Reconnect.max_outputs(), Mode::All.max_outputs()), (1, usize::MAX));
    }

    #[test]
    fn nothing_changed_nothing_to_do() {
        let driven = [Driven { connector: 1, crtc: 10, mode: WX }];
        let scan = [conn(1, true, Some(WX), &[10, 11]), conn(2, false, None, &[10, 11])];
        assert!(plan(&driven, &scan, 1, &[]).is_empty());
        assert!(plan(&driven, &scan, usize::MAX, &[]).is_empty());
    }

    #[test]
    fn unplug_and_replug_the_only_monitor() {
        let driven = [Driven { connector: 1, crtc: 10, mode: WX }];
        let unplugged = [conn(1, false, None, &[10, 11])];
        assert_eq!(plan(&driven, &unplugged, 1, &[]), Plan { remove: vec![1], add: vec![] });
        let replugged = [conn(1, true, Some(WX), &[10, 11])];
        assert_eq!(plan(&[], &replugged, 1, &[]), Plan { remove: vec![], add: vec![(1, 10)] });
    }

    #[test]
    fn different_monitor_on_the_same_port_is_set_up_again() {
        let driven = [Driven { connector: 1, crtc: 10, mode: WX }];
        let scan = [conn(1, true, Some(HD), &[10, 11])];
        // Removed first, so its CRTC is free again for the new mode.
        assert_eq!(plan(&driven, &scan, 1, &[]), Plan { remove: vec![1], add: vec![(1, 10)] });
    }

    #[test]
    fn reconnect_mode_drives_one_output_only() {
        let scan = [conn(1, true, Some(WX), &[10, 11]), conn(2, true, Some(HD), &[10, 11])];
        assert_eq!(plan(&[], &scan, 1, &[]).add, vec![(1, 10)]);
        // Only monitor unplugged while another is connected: switch to that one.
        let driven = [Driven { connector: 1, crtc: 10, mode: WX }];
        let scan = [conn(1, false, None, &[10, 11]), conn(2, true, Some(HD), &[10, 11])];
        assert_eq!(plan(&driven, &scan, 1, &[]), Plan { remove: vec![1], add: vec![(2, 10)] });
    }

    #[test]
    fn second_monitor_gets_its_own_crtc() {
        let driven = [Driven { connector: 1, crtc: 10, mode: WX }];
        let scan = [conn(1, true, Some(WX), &[10, 11]), conn(2, true, Some(HD), &[10, 11])];
        assert_eq!(plan(&driven, &scan, usize::MAX, &[]), Plan { remove: vec![], add: vec![(2, 11)] });
    }

    #[test]
    fn no_free_crtc_means_not_driven() {
        let driven = [Driven { connector: 1, crtc: 10, mode: WX }];
        let scan = [conn(1, true, Some(WX), &[10]), conn(2, true, Some(HD), &[10])];
        assert!(plan(&driven, &scan, usize::MAX, &[]).is_empty());
    }

    #[test]
    fn connected_without_modes_or_failed_is_skipped() {
        let scan = [conn(1, true, None, &[10]), conn(2, true, Some(HD), &[10])];
        assert_eq!(plan(&[], &scan, usize::MAX, &[]).add, vec![(2, 10)]);
        assert!(plan(&[], &scan, usize::MAX, &[2]).is_empty());
    }

    #[test]
    fn primary_named_or_largest() {
        let m = |w: i32, h: i32, hz: u32| VideoMode { w, h, refresh_mhz: hz * 1000 };
        // The GT 710 on 2026-10-10: HDMI (listed first) smaller than VGA.
        let two = [("HDMIA-1", m(1366, 768, 60)), ("VGA-1", m(1440, 900, 60))];
        assert_eq!(pick_primary(&two, None), Some((1, PrimaryChoice::Largest)));
        assert_eq!(pick_primary(&two, Some("auto")), Some((1, PrimaryChoice::Largest)));
        assert_eq!(pick_primary(&two, Some(" ")), Some((1, PrimaryChoice::Largest)));
        // Named the kernel's way or kompact's way.
        assert_eq!(pick_primary(&two, Some("HDMI-A-1")), Some((0, PrimaryChoice::Named)));
        assert_eq!(pick_primary(&two, Some("hdmia-1")), Some((0, PrimaryChoice::Named)));
        assert_eq!(pick_primary(&two, Some("DP-1")), Some((1, PrimaryChoice::NamedMissing)));
        // Same size: higher refresh, then connector order.
        let same = [("A-1", m(1920, 1080, 60)), ("B-1", m(1920, 1080, 75)), ("C-1", m(1920, 1080, 75))];
        assert_eq!(pick_primary(&same, None), Some((1, PrimaryChoice::Largest)));
        assert_eq!(pick_primary(&same[..1], None), Some((0, PrimaryChoice::Largest)));
        assert_eq!(pick_primary(&[], None), None);
        assert!(same_name("VGA-1", "vga1") && !same_name("VGA-1", "VGA-2"));
    }

    #[test]
    fn layout_left_to_right() {
        let sizes = [Size::from((1440, 900)), Size::from((1920, 1080)), Size::from((800, 600))];
        let xs: Vec<i32> = layout(&sizes).iter().map(|p| p.x).collect();
        assert_eq!(xs, vec![0, 1440, 3360]);
        assert!(layout(&sizes).iter().all(|p| p.y == 0));
        assert!(layout(&[]).is_empty());
    }

    #[test]
    fn arrangement_file() {
        let a = Arrangement::parse("# x\nHDMI-A-1 0 900\nVGA-1 -50 0\nbad line\nDP-1 x 3\nHDMI-A-1 10 900\n");
        assert_eq!(a.places, vec![("HDMI-A-1".to_string(), Point::from((10, 900))), ("VGA-1".to_string(), Point::from((-50, 0)))]);
        assert_eq!((a.primary.as_deref(), a.every_menu_bar), (None, false));
        assert_eq!(Arrangement::parse(&a.to_text()), a);
        assert_eq!(Arrangement::parse(""), Arrangement::default());
        // 12h-2
        let a = Arrangement::parse("VGA-1 0 0\nprimary HDMIA-1\nmenu-bars every\n");
        assert_eq!((a.primary.as_deref(), a.every_menu_bar), (Some("HDMIA-1"), true));
        assert_eq!(Arrangement::parse(&a.to_text()), a);
    }

    #[test]
    fn main_display_and_menu_bars() {
        let two = [("HDMIA-1", Size::from((1366, 768))), ("VGA-1", Size::from((1440, 900)))];
        assert_eq!(pick_main(&two, None, None), Some(1));
        assert_eq!(pick_main(&two, Some("HDMIA-1"), Some("VGA-1")), Some(0));
        assert_eq!(pick_main(&two, Some("DP-1"), Some("hdmia-1")), Some(0));
        assert_eq!(pick_main(&two, Some("DP-1"), None), Some(1));
        assert_eq!(pick_main(&[], None, None), None);
        let names = ["HDMIA-1", "VGA-1", "DP-1"];
        assert_eq!(next_menu_bar(&names, Some(1), &[], true), Some(1));
        assert_eq!(next_menu_bar(&names, Some(1), &["VGA-1"], true), Some(0));
        assert_eq!(next_menu_bar(&names, Some(1), &["VGA-1", "HDMIA-1"], true), Some(2));
        assert_eq!(next_menu_bar(&names, Some(1), &names, true), Some(1));
        assert_eq!(next_menu_bar(&names, None, &[], true), Some(0));
        assert_eq!(next_menu_bar(&[], Some(0), &[], true), None);
        // One menu bar: the main display, even while the extra bar on it is still going away
        // (GT 710, 2026-10-12: it landed on the other display).
        assert_eq!(next_menu_bar(&names, Some(1), &["VGA-1"], false), Some(1));
        assert_eq!(next_menu_bar(&names, None, &["HDMIA-1"], false), Some(0));
    }

    #[test]
    fn snapping_a_dragged_monitor() {
        let vga = rect(0, 0, 1440, 900);
        let hdmi = Size::from((1366, 768));
        // Dropped a bit right of and below VGA's right edge: right of it, tops lined up.
        assert_eq!(snap(hdmi, Point::from((1500, 30)), &[vga]), Point::from((1440, 0)));
        // Bottoms lined up when that's closer.
        assert_eq!(snap(hdmi, Point::from((1400, 120)), &[vga]), Point::from((1440, 132)));
        // Free along the edge, but sharing at least MIN_SHARED.
        assert_eq!(snap(hdmi, Point::from((1440, 400)), &[vga]), Point::from((1440, 400)));
        assert_eq!(snap(hdmi, Point::from((1440, 850)), &[vga]), Point::from((1440, 900 - MIN_SHARED)));
        // Dropped on top of it, nearer its top edge: above it.
        assert_eq!(snap(hdmi, Point::from((300, -200)), &[vga]), Point::from((300, -768)));
        // Left of it, lined up.
        assert_eq!(snap(hdmi, Point::from((-1300, 10)), &[vga]), Point::from((-1366, 0)));
        // No other monitor: where it was dropped.
        assert_eq!(snap(hdmi, Point::from((7, 8)), &[]), Point::from((7, 8)));
        // Never onto a third monitor: right of VGA is taken, so below it.
        let right = rect(1440, 0, 1000, 900);
        let p = snap(hdmi, Point::from((1500, 100)), &[vga, right]);
        assert!(![vga, right].iter().any(|r| overlap(Rectangle::new(p, hdmi), *r)), "{p:?}");
        assert!([vga, right].iter().any(|r| touches(Rectangle::new(p, hdmi), *r)), "{p:?}");
    }

    #[test]
    fn arranging_outputs() {
        let hdmi = ("HDMI-A-1", Size::from((1366, 768)));
        let vga = ("VGA-1", Size::from((1440, 900)));
        let pts = |v: &[(i32, i32)]| v.iter().map(|&p| Point::from(p)).collect::<Vec<_>>();
        // Nothing saved: side by side, as before.
        assert_eq!(arrange(&[hdmi, vga], &Arrangement::default()), pts(&[(0, 0), (1366, 0)]));
        // VGA on the left now, bottoms lined up.
        let a = Arrangement::parse("HDMI-A-1 1440 132\nVGA-1 0 0");
        assert_eq!(arrange(&[hdmi, vga], &a), pts(&[(1440, 132), (0, 0)]));
        // HDMI above VGA, saved off zero: moved so the top-left is 0,0.
        let a = Arrangement::parse("HDMI-A-1 500 -768\nVGA-1 400 0");
        assert_eq!(arrange(&[hdmi, vga], &a), pts(&[(100, 0), (0, 768)]));
        // Overlapping (say a monitor got a bigger mode): the second is snapped next to the first.
        let a = Arrangement::parse("HDMI-A-1 0 0\nVGA-1 1000 0");
        let got = arrange(&[hdmi, vga], &a);
        let (r0, r1) = (Rectangle::new(got[0], hdmi.1), Rectangle::new(got[1], vga.1));
        assert!(!overlap(r0, r1) && touches(r0, r1), "{got:?}");
        // A gap: closed.
        let a = Arrangement::parse("HDMI-A-1 0 0\nVGA-1 1500 0");
        assert_eq!(arrange(&[hdmi, vga], &a), pts(&[(0, 0), (1366, 0)]));
        // Only one saved, or a monitor never seen: the new one goes to the right.
        let a = Arrangement::parse("VGA-1 0 0\nDP-1 0 900");
        assert_eq!(arrange(&[hdmi, vga], &a), pts(&[(1440, 0), (0, 0)]));
        assert_eq!(arrange(&[vga], &a), pts(&[(0, 0)]));
        assert!(arrange(&[], &a).is_empty());
    }

    #[test]
    fn windows_follow_a_moved_output() {
        // Left monitor unplugged: the right one moves from x=1440 to x=0.
        let moves = [(rect(0, 0, 1440, 900), None), (rect(1440, 0, 1920, 1080), Some(rect(0, 0, 1920, 1080)))];
        let fallback = Some(rect(0, 0, 1920, 1080));
        assert_eq!(relocate(rect(1600, 100, 800, 600), &moves, fallback), Some((160, 100).into()));
    }

    #[test]
    fn windows_on_a_removed_output_move_to_the_fallback() {
        let moves = [(rect(0, 0, 1920, 1080), Some(rect(0, 0, 1920, 1080))), (rect(1920, 0, 1440, 900), None)];
        let fallback = Some(rect(0, 0, 1920, 1080));
        // Same spot relative to the output.
        assert_eq!(relocate(rect(2020, 50, 600, 400), &moves, fallback), Some((100, 50).into()));
        // Kept inside a smaller fallback.
        let small = Some(rect(0, 0, 1024, 768));
        let moves = [(rect(0, 0, 1920, 1080), None)];
        assert_eq!(relocate(rect(1500, 700, 600, 400), &moves, small), Some((424, 368).into()));
    }

    #[test]
    fn displaced_windows_go_home() {
        // Was at 100,50 on HDMI (left, 1366x768); kompact put it at 100,50 on VGA.
        let d = Displaced { from: "HDMIA-1".into(), rect: rect(100, 50, 600, 400), on: "VGA-1".into(), at: (100, 50).into() };
        let vga_alone = [("VGA-1", rect(0, 0, 1440, 900))];
        let both = [("HDMIA-1", rect(0, 0, 1366, 768)), ("VGA-1", rect(1366, 0, 1440, 900))];
        // HDMI not back yet.
        assert_eq!(return_home(&d, Some(("VGA-1", (100, 50).into())), &vga_alone), Return::Wait);
        // Back (VGA moved right meanwhile; the window's place relative to VGA is unchanged).
        assert_eq!(return_home(&d, Some(("VGA-1", (100, 50).into())), &both), Return::Back((100, 50).into()));
        // HDMI back at another position: same spot relative to it.
        let swapped = [("VGA-1", rect(0, 0, 1440, 900)), ("HDMIA-1", rect(1440, 0, 1366, 768))];
        assert_eq!(return_home(&d, Some(("VGA-1", (100, 50).into())), &swapped), Return::Back((1540, 50).into()));
        // The user moved it (or moved it to another monitor): it stays.
        assert_eq!(return_home(&d, Some(("VGA-1", (300, 50).into())), &both), Return::Forget);
        assert_eq!(return_home(&d, Some(("HDMIA-1", (100, 50).into())), &both), Return::Forget);
        assert_eq!(return_home(&d, None, &both), Return::Forget);
        // A big window is kept inside the returning monitor.
        let big = Displaced { rect: rect(1000, 500, 600, 400), ..d.clone() };
        assert_eq!(return_home(&big, Some(("VGA-1", (100, 50).into())), &both), Return::Back((766, 368).into()));
    }

    #[test]
    fn windows_stay_when_their_output_stays_or_nothing_is_left() {
        let moves = [(rect(0, 0, 1920, 1080), Some(rect(0, 0, 1920, 1080)))];
        assert_eq!(relocate(rect(100, 100, 400, 300), &moves, None), None);
        // Only output unplugged: nowhere to go, the window keeps its place for when it's back.
        let moves = [(rect(0, 0, 1920, 1080), None)];
        assert_eq!(relocate(rect(100, 100, 400, 300), &moves, None), None);
        // Off every output (centre outside all of them): left alone.
        assert_eq!(relocate(rect(5000, 5000, 10, 10), &moves, Some(rect(0, 0, 10, 10))), None);
    }

    #[test]
    fn pointer_crosses_between_outputs_but_not_off_them() {
        let outputs = [rect(0, 0, 1440, 900), rect(1440, 0, 1920, 1080)];
        let p = |x: f64, y: f64| Point::<f64, Logical>::from((x, y));
        // Crossing to the right output.
        assert_eq!(clamp_pointer(p(1450.0, 500.0), p(1430.0, 500.0), &outputs), p(1450.0, 500.0));
        // Below the shorter left output: clamped to it.
        assert_eq!(clamp_pointer(p(100.0, 950.0), p(100.0, 890.0), &outputs), p(100.0, 899.0));
        // The taller right output allows it.
        assert_eq!(clamp_pointer(p(1500.0, 950.0), p(1500.0, 890.0), &outputs), p(1500.0, 950.0));
        // Off the left edge.
        assert_eq!(clamp_pointer(p(-5.0, 10.0), p(3.0, 10.0), &outputs), p(0.0, 10.0));
        // No outputs at all: anything goes.
        assert_eq!(clamp_pointer(p(-5.0, 10.0), p(3.0, 10.0), &[]), p(-5.0, 10.0));
    }

    #[test]
    fn clear_of_top_panel() {
        let old = rect(0, 0, 1440, 900);
        let new = rect(0, 28, 1440, 872);
        // Under the panel: pushed down just below it.
        assert_eq!(clear_of_reserved(rect(0, 0, 696, 494), old, new), Some(Point::from((0, 28))));
        assert_eq!(clear_of_reserved(rect(100, 10, 696, 494), old, new), Some(Point::from((100, 28))));
        // Already clear, or only off the bottom (not newly reserved): left alone.
        assert_eq!(clear_of_reserved(rect(64, 64, 696, 494), old, new), None);
        assert_eq!(clear_of_reserved(rect(64, 700, 696, 494), old, new), None);
        // The panel going away moves nothing.
        assert_eq!(clear_of_reserved(rect(0, 28, 696, 494), new, old), None);
    }

    #[test]
    fn clear_of_bottom_and_side_panels() {
        let old = rect(0, 0, 1440, 900);
        // Bottom panel: a window reaching into it moves up.
        assert_eq!(clear_of_reserved(rect(10, 500, 600, 400), old, rect(0, 0, 1440, 868)), Some(Point::from((10, 468))));
        // Right panel.
        assert_eq!(clear_of_reserved(rect(1000, 10, 440, 300), old, rect(0, 0, 1400, 900)), Some(Point::from((960, 10))));
        // Left panel; works on a second output too.
        let old2 = rect(1440, 0, 1920, 1080);
        assert_eq!(clear_of_reserved(rect(1440, 50, 800, 600), old2, rect(1488, 0, 1872, 1080)), Some(Point::from((1488, 50))));
        // Taller than what's left: the top stays visible.
        assert_eq!(clear_of_reserved(rect(0, 0, 600, 900), old, rect(0, 28, 1440, 840)), Some(Point::from((0, 28))));
        // The dock coming back under a full-height window just restored from maximized (6m-d2
        // retest: Ptyxis went to y = -42, under the top panel): it stays at the top.
        let docked = rect(0, 28, 1440, 802);
        assert_eq!(clear_of_reserved(rect(0, 28, 1440, 872), rect(0, 28, 1440, 872), docked), None);
        assert_eq!(clear_of_reserved(rect(0, 100, 600, 872), rect(0, 28, 1440, 872), docked), Some(Point::from((0, 28))));
        // Too wide for a new right panel: the left edge stays.
        assert_eq!(clear_of_reserved(rect(20, 10, 1440, 300), old, rect(0, 0, 1400, 900)), Some(Point::from((0, 10))));
    }
}
