//! 6m-e2b-1: kompact's own title bar, for apps that ask for one (xdg-decoration server side;
//! so far only foot). On by default since 6m-e4; `KOMPACT_SSD=off` lets every app draw its own.
//!
//! A macOS-style dark bar, `HEIGHT` px high and as wide as the window, with the three traffic
//! lights on the left (close, minimize, maximize). It sits right above the window, outside the
//! window's geometry. 6m-e2b-2: window management (placement, snap, maximize, remembered
//! geometry, drags) works on the *frame*, the window plus its bar (`frame_of`/`client_of`);
//! `shell.rs` converts only where a size goes to the client or a window goes into the space.
//! 6m-e3: `part_at` says what the pointer is on: a button, the bar (drag to move, double-click
//! to maximize), or the `BORDER` px just outside the frame (drag to resize; these apps draw no
//! borders of their own). No title text (that needs a font). Fullscreen windows get no bar.
//! 6m-e5: only the focused window's bar is lit; the others are flatter, with grey buttons.
//! 6m-e6: with the pointer over the buttons they show their symbols (✕ − +, drawn as lines, no
//! font), in colour even on a window without the keyboard, as on macOS.
//! 6m-e7: the window's title, centred (`title_text.rs` lays it out; it's blended in here).
//!
//! 10d-3: a light bar too, and the buttons on the right (Windows-style order: minimize,
//! maximize, close) if System Settings → Appearance says so (`Style`, set by `set_style`).
//!
//! Everything here is pure: the image is premultiplied ARGB8888 bytes (B, G, R, A in memory),
//! like the built-in cursors, and `render.rs` turns it into a `MemoryRenderBuffer`.

pub const HEIGHT: i32 = 28;
/// Rounded top corners, like a macOS window.
pub const CORNER: f32 = 10.0;
pub const BUTTON_DIAMETER: f32 = 12.0;

/// 10d-3: how bars look now (Settings → Appearance).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Style {
    pub light: bool,
    /// Buttons on the right.
    pub right: bool,
}

static STYLE: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0);

pub fn style() -> Style {
    let v = STYLE.load(std::sync::atomic::Ordering::Relaxed);
    Style { light: v & 1 != 0, right: v & 2 != 0 }
}

pub fn set_style(s: Style) {
    STYLE.store(s.light as u8 | (s.right as u8) << 1, std::sync::atomic::Ordering::Relaxed);
}

/// The colours of a bar that aren't the buttons'.
struct Palette {
    bar: [u8; 3],
    bar_inactive: [u8; 3],
    line: [u8; 3],
    text: [u8; 3],
    text_inactive: [u8; 3],
    button_inactive: ([u8; 3], [u8; 3]),
}

const DARK: Palette = Palette {
    bar: BAR,
    bar_inactive: BAR_INACTIVE,
    line: LINE,
    text: TEXT,
    text_inactive: TEXT_INACTIVE,
    button_inactive: BUTTON_INACTIVE,
};
/// 10d-3: macOS light mode: a light grey bar, a paler one without the keyboard, dark text.
const LIGHT: Palette = Palette {
    bar: [0xe8, 0xe8, 0xe8],
    bar_inactive: [0xf6, 0xf6, 0xf6],
    line: [0xc8, 0xc8, 0xc8],
    text: [0x26, 0x26, 0x26],
    text_inactive: [0xa8, 0xa8, 0xa8],
    button_inactive: ([0xd6, 0xd6, 0xd6], [0xc4, 0xc4, 0xc4]),
};

const BAR: [u8; 3] = [0x2e, 0x2e, 0x2e];
const LINE: [u8; 3] = [0x1a, 0x1a, 0x1a];
/// Close, minimize, maximize (macOS colours), with a slightly darker rim.
const BUTTONS: [([u8; 3], [u8; 3]); 3] = [
    ([0xff, 0x5f, 0x57], [0xe0, 0x44, 0x3e]),
    ([0xfe, 0xbc, 0x2e], [0xde, 0xa1, 0x23]),
    ([0x28, 0xc8, 0x40], [0x1a, 0xab, 0x29]),
];
/// 6m-e5: a window without the keyboard (macOS dark mode: a flatter bar, all three buttons grey).
const BAR_INACTIVE: [u8; 3] = [0x26, 0x26, 0x26];
const BUTTON_INACTIVE: ([u8; 3], [u8; 3]) = ([0x4d, 0x4d, 0x4d], [0x42, 0x42, 0x42]);
/// 6m-e6: the symbols, dark on each button (macOS draws them in a darker shade of the button).
const SYMBOLS: [[u8; 3]; 3] = [[0x4d, 0x00, 0x00], [0x99, 0x58, 0x00], [0x00, 0x65, 0x00]];
/// 6m-e7: title text on a focused window and on the others (macOS dark: ~85% and ~45% white).
const TEXT: [u8; 3] = [0xdd, 0xdd, 0xdd];
const TEXT_INACTIVE: [u8; 3] = [0x7a, 0x7a, 0x7a];
/// Half the length of a symbol's stroke.
const SYMBOL_HALF: f32 = 3.0;

/// On unless `off`/`0`/`no`/`false` (6m-e4, after e1–e3 were confirmed on the GT 710).
pub fn enabled_from_env(value: Option<&str>) -> bool {
    !matches!(value.map(|v| v.trim().to_ascii_lowercase()).as_deref(), Some("off" | "0" | "no" | "false"))
}

pub fn enabled() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| {
        let on = enabled_from_env(std::env::var("KOMPACT_SSD").ok().as_deref());
        if on {
            tracing::info!("titlebar: kompact draws title bars for apps that ask (6m-e4; KOMPACT_SSD=off to let every app draw its own)");
        } else {
            tracing::info!("titlebar: off, every app draws its own (KOMPACT_SSD=off)");
        }
        on
    })
}

// These read the toplevel's state, which takes its lock: never call them inside
// `with_pending_state` (the lock isn't re-entrant; it would hang).

/// Switched on and the app took server-side decorations: it has a bar whenever it isn't fullscreen.
pub fn has_bar_mode(window: &smithay::desktop::Window) -> bool {
    use smithay::reexports::wayland_protocols::xdg::decoration::zv1::server::zxdg_toplevel_decoration_v1::Mode;
    enabled() && window.toplevel().is_some_and(|t| t.current_state().decoration_mode == Some(Mode::ServerSide))
}

/// A window drawn with kompact's bar now: `has_bar_mode` and not (about to be) fullscreen. The
/// pending state counts, so a window just told to go fullscreen (or to leave it) is placed for
/// what it is becoming.
pub fn decorated(window: &smithay::desktop::Window) -> bool {
    use smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel;
    has_bar_mode(window)
        && window
            .toplevel()
            .is_some_and(|t| !t.with_pending_state(|s| s.states.contains(xdg_toplevel::State::Fullscreen)))
}

/// The bar's height above `window`: `HEIGHT` if decorated, else 0.
pub fn bar_height(window: &smithay::desktop::Window) -> i32 {
    if decorated(window) {
        HEIGHT
    } else {
        0
    }
}

type Rect = smithay::utils::Rectangle<i32, smithay::utils::Logical>;

/// The frame (window + a `bar` px bar on top) around a window at `client`.
pub fn frame_of(client: Rect, bar: i32) -> Rect {
    Rect::new((client.loc.x, client.loc.y - bar).into(), (client.size.w, client.size.h + bar).into())
}

/// The window inside a frame (at least 1 px high, so a tiny frame never asks for size 0,
/// which would mean "you choose" to the client).
pub fn client_of(frame: Rect, bar: i32) -> Rect {
    Rect::new((frame.loc.x, frame.loc.y + bar).into(), (frame.size.w, (frame.size.h - bar).max(1)).into())
}

/// The invisible resize margin just outside a decorated window's frame.
pub const BORDER: i32 = 6;
/// Along an edge, this close to a corner grabs the corner (both edges).
pub const CORNER_GRAB: i32 = 16;
/// A button reacts a little beyond its drawn circle.
const BUTTON_SLOP: f32 = 2.0;
/// Two presses on the bar within this many ms are a double-click.
pub const DOUBLE_CLICK_MS: u128 = 400;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Part {
    Close,
    Minimize,
    Maximize,
    /// The rest of the bar.
    Bar,
    /// The resize margin; which edges a drag there moves.
    Edge(crate::resize::Edges),
}

/// What of a decorated window (`frame` = window + `bar` px bar) is at `p`; `None` for the
/// window's own content and anything outside the resize margin.
pub fn part_at(frame: Rect, bar: i32, p: smithay::utils::Point<f64, smithay::utils::Logical>) -> Option<Part> {
    part_at_side(frame, bar, p, style().right)
}

/// `part_at` with the buttons on the left or (`right`) the right.
pub fn part_at_side(frame: Rect, bar: i32, p: smithay::utils::Point<f64, smithay::utils::Logical>, right: bool) -> Option<Part> {
    let (x0, y0) = (frame.loc.x as f64, frame.loc.y as f64);
    let (x1, y1) = (x0 + frame.size.w as f64, y0 + frame.size.h as f64);
    let b = BORDER as f64;
    if p.x < x0 - b || p.x >= x1 + b || p.y < y0 - b || p.y >= y1 + b {
        return None;
    }
    let inside = p.x >= x0 && p.x < x1 && p.y >= y0 && p.y < y1;
    if !inside {
        let c = CORNER_GRAB as f64;
        let near_left = p.x < x0 + c;
        let near_right = p.x >= x1 - c;
        let near_top = p.y < y0 + c;
        let near_bottom = p.y >= y1 - c;
        let (out_l, out_r, out_t, out_b) = (p.x < x0, p.x >= x1, p.y < y0, p.y >= y1);
        return Some(Part::Edge(crate::resize::Edges {
            left: out_l || ((out_t || out_b) && near_left),
            right: out_r || ((out_t || out_b) && near_right),
            top: out_t || ((out_l || out_r) && near_top),
            bottom: out_b || ((out_l || out_r) && near_bottom),
        }));
    }
    if p.y >= y0 + bar as f64 {
        return None;
    }
    let local = (p.x - x0, p.y - y0);
    let r = BUTTON_DIAMETER / 2.0 + BUTTON_SLOP;
    let fits = |cx: f32| fits(cx, frame.size.w);
    for ((cx, cy), part) in button_centres_in(frame.size.w, right).into_iter().zip([Part::Close, Part::Minimize, Part::Maximize]) {
        let d = ((local.0 as f32 - cx).powi(2) + (local.1 as f32 - cy).powi(2)).sqrt();
        if fits(cx) && d <= r {
            return Some(part);
        }
    }
    Some(Part::Bar)
}

/// Centres of the three buttons in bar coordinates (macOS: 8 px margin, 20 px apart), in the
/// order close, minimize, maximize.
pub fn button_centres() -> [(f32, f32); 3] {
    let y = HEIGHT as f32 / 2.0;
    [(14.0, y), (34.0, y), (54.0, y)]
}

/// 10d-3: the same for a bar `width` px wide with the buttons on the left or (`right`) at the
/// right edge, there in the order minimize, maximize, close (close in the corner, as on
/// Windows). Still close, minimize, maximize in the array.
pub fn button_centres_in(width: i32, right: bool) -> [(f32, f32); 3] {
    let c = button_centres();
    if !right {
        return c;
    }
    let w = width as f32;
    let y = c[0].1;
    [(w - 14.0, y), (w - 54.0, y), (w - 34.0, y)]
}

/// A button at `cx` is drawn (and takes clicks) only if it fits in the bar.
fn fits(cx: f32, width: i32) -> bool {
    cx - BUTTON_DIAMETER / 2.0 >= 0.0 && cx + BUTTON_DIAMETER / 2.0 <= width as f32
}

/// How much of the pixel at (`px`, `py`) (its centre) lies within `r` of (`cx`, `cy`):
/// 1 inside, 0 outside, a one-pixel ramp at the edge for smooth circles.
fn coverage(px: f32, py: f32, cx: f32, cy: f32, r: f32) -> f32 {
    let d = ((px - cx).powi(2) + (py - cy).powi(2)).sqrt();
    (r + 0.5 - d).clamp(0.0, 1.0)
}

/// How much of the pixel is inside the bar's shape (only the two top corners are rounded).
fn bar_coverage(px: f32, py: f32, width: f32) -> f32 {
    let r = CORNER.min(width / 2.0);
    if py >= r {
        return 1.0;
    }
    if px < r {
        coverage(px, py, r, r, r)
    } else if px > width - r {
        coverage(px, py, width - r, r, r)
    } else {
        1.0
    }
}

/// How much of the pixel lies within `half` of the segment (`ax`, `ay`)–(`bx`, `by`).
fn stroke(px: f32, py: f32, half: f32, (ax, ay): (f32, f32), (bx, by): (f32, f32)) -> f32 {
    let (dx, dy) = (bx - ax, by - ay);
    let t = (((px - ax) * dx + (py - ay) * dy) / (dx * dx + dy * dy)).clamp(0.0, 1.0);
    let d = ((px - ax - t * dx).powi(2) + (py - ay - t * dy).powi(2)).sqrt();
    (half + 0.5 - d).clamp(0.0, 1.0)
}

/// The segments of button `i`'s symbol (0 ✕ close, 1 − minimize, 2 + maximize) and their half
/// width. The button centres lie between pixels, so the symbols sit half a pixel off, on a pixel
/// centre: − and + then come out as crisp 1 px lines instead of 2 px blurs.
fn symbol(i: usize, cx: f32, cy: f32) -> (f32, Vec<((f32, f32), (f32, f32))>) {
    let (cx, cy) = (cx + 0.5, cy + 0.5);
    let h = SYMBOL_HALF;
    let d = h * 0.85; // the ✕'s arms are diagonal; keep them inside the same box
    match i {
        0 => (0.7, vec![((cx - d, cy - d), (cx + d, cy + d)), ((cx - d, cy + d), (cx + d, cy - d))]),
        1 => (0.5, vec![((cx - h, cy), (cx + h, cy))]),
        _ => (0.5, vec![((cx - h, cy), (cx + h, cy)), ((cx, cy - h), (cx, cy + h))]),
    }
}

fn blend(under: [f32; 3], over: [u8; 3], a: f32) -> [f32; 3] {
    [0, 1, 2].map(|i| under[i] * (1.0 - a) + over[i] as f32 * a)
}

/// The bar for a window `width` px wide: premultiplied ARGB8888, `width * HEIGHT * 4` bytes.
/// `active`: the window has the keyboard (coloured buttons), else the dimmed bar. `symbols`:
/// the pointer is over the buttons (6m-e6): coloured buttons with their symbols either way.
/// `title`: laid out for this width by `title_text::layout` (6m-e7).
#[cfg(test)]
pub fn pixels(width: i32, active: bool, symbols: bool, title: Option<&crate::title_text::Text>) -> Vec<u8> {
    pixels_styled(width, active, symbols, title, style())
}

/// `pixels` for a given look (10d-3).
pub fn pixels_styled(width: i32, active: bool, symbols: bool, title: Option<&crate::title_text::Text>, style: Style) -> Vec<u8> {
    let width = width.max(1);
    let pal = if style.light { &LIGHT } else { &DARK };
    let mut out = Vec::with_capacity((width * HEIGHT * 4) as usize);
    let centres = button_centres_in(width, style.right);
    // Only draw the buttons that fit (a very narrow window gets fewer).
    let fits = |cx: f32| fits(cx, width);
    let title_x = title.map_or(0, |t| crate::title_text::place_side(width, t.width, style.right));
    for y in 0..HEIGHT {
        for x in 0..width {
            let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
            let bar = if active { pal.bar } else { pal.bar_inactive };
            let mut rgb = if y == HEIGHT - 1 { pal.line } else { bar }.map(|c| c as f32);
            if let Some(t) = title.filter(|t| y < HEIGHT - 1 && x >= title_x && x < title_x + t.width) {
                let ink = t.coverage[(y * t.width + x - title_x) as usize] as f32 / 255.0;
                rgb = blend(rgb, if active { pal.text } else { pal.text_inactive }, ink);
            }
            for (i, (&(cx, cy), button)) in centres.iter().zip(BUTTONS).enumerate() {
                let (fill, rim) = if active || symbols { button } else { pal.button_inactive };
                if fits(cx) {
                    let r = BUTTON_DIAMETER / 2.0;
                    rgb = blend(rgb, rim, coverage(px, py, cx, cy, r));
                    rgb = blend(rgb, fill, coverage(px, py, cx, cy, r - 0.75));
                    if symbols && (px - cx).abs() < r && (py - cy).abs() < r {
                        let (half, segments) = symbol(i, cx, cy);
                        let ink = segments.into_iter().map(|(a, b)| stroke(px, py, half, a, b)).fold(0.0, f32::max);
                        rgb = blend(rgb, SYMBOLS[i], ink);
                    }
                }
            }
            let a = bar_coverage(px, py, width as f32);
            let [r, g, b] = rgb.map(|c| (c * a).round() as u8);
            out.extend_from_slice(&[b, g, r, (a * 255.0).round() as u8]);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pixel(buf: &[u8], width: i32, x: i32, y: i32) -> [u8; 4] {
        let i = ((y * width + x) * 4) as usize;
        [buf[i], buf[i + 1], buf[i + 2], buf[i + 3]]
    }

    #[test]
    fn buttons_on_the_right() {
        let frame = Rect::new((100, 100).into(), (400, 300).into());
        let [(cx, cy), (mx, _), (zx, _)] = button_centres_in(400, true);
        assert_eq!((cx, mx, zx), (386.0, 346.0, 366.0), "minimize, maximize, close from the left");
        let at = |x: f32| part_at_side(frame, HEIGHT, (100.0 + x as f64, 100.0 + cy as f64).into(), true);
        assert_eq!(at(cx), Some(Part::Close));
        assert_eq!(at(mx), Some(Part::Minimize));
        assert_eq!(at(zx), Some(Part::Maximize));
        // Where the close button is on the left, it's bar now.
        assert_eq!(at(14.0), Some(Part::Bar));
        assert_eq!(button_centres_in(400, false), button_centres());
        // Drawn there: the close button's red at its centre on the right, bar colour on the left.
        let buf = pixels_styled(400, true, false, None, Style { light: false, right: true });
        assert_eq!(pixel(&buf, 400, 386, 14)[2], 0xff, "red channel of close");
        assert_eq!(pixel(&buf, 400, 14, 14), [BAR[2], BAR[1], BAR[0], 255]);
    }

    #[test]
    fn light_bars() {
        let dark = pixels_styled(300, true, false, None, Style::default());
        let light = pixels_styled(300, true, false, None, Style { light: true, right: false });
        assert_eq!(pixel(&dark, 300, 150, 10), [BAR[2], BAR[1], BAR[0], 255]);
        assert_eq!(pixel(&light, 300, 150, 10), [0xe8, 0xe8, 0xe8, 255]);
        let idle = pixels_styled(300, false, false, None, Style { light: true, right: false });
        assert_eq!(pixel(&idle, 300, 150, 10), [0xf6, 0xf6, 0xf6, 255]);
    }

    #[test]
    fn on_unless_switched_off() {
        assert!(enabled_from_env(None));
        assert!(enabled_from_env(Some("on")));
        assert!(!enabled_from_env(Some("off")));
        assert!(!enabled_from_env(Some(" FALSE ")));
    }

    #[test]
    fn frame_and_client() {
        let client = Rect::new((100, 228).into(), (640, 400).into());
        let frame = frame_of(client, HEIGHT);
        assert_eq!(frame, Rect::new((100, 200).into(), (640, 428).into()));
        assert_eq!(client_of(frame, HEIGHT), client);
        // No bar: the same rectangle both ways.
        assert_eq!(frame_of(client, 0), client);
        assert_eq!(client_of(client, 0), client);
        // A frame smaller than the bar still leaves a 1 px window.
        assert_eq!(client_of(Rect::new((0, 0).into(), (50, 10).into()), HEIGHT).size.h, 1);
    }

    #[test]
    fn parts() {
        use crate::resize::Edges;
        let frame = Rect::new((100, 100).into(), (400, 300).into());
        let at = |x: f64, y: f64| part_at(frame, HEIGHT, (x, y).into());
        let [(rx, ry), (yx, _), (gx, _)] = button_centres();
        assert_eq!(at(100.0 + rx as f64, 100.0 + ry as f64), Some(Part::Close));
        assert_eq!(at(100.0 + yx as f64, 100.0 + ry as f64), Some(Part::Minimize));
        assert_eq!(at(100.0 + gx as f64 + 7.0, 100.0 + ry as f64), Some(Part::Maximize)); // within the slop
        assert_eq!(at(300.0, 110.0), Some(Part::Bar));
        assert_eq!(at(300.0, 100.0 + HEIGHT as f64), None); // the window's own content
        assert_eq!(at(300.0, 250.0), None);
        assert_eq!(at(300.0, 90.0), None); // beyond the margin
        // The margin outside each edge, and corners.
        let edge = |l, r, t, b| Some(Part::Edge(Edges { left: l, right: r, top: t, bottom: b }));
        assert_eq!(at(97.0, 250.0), edge(true, false, false, false));
        assert_eq!(at(503.0, 250.0), edge(false, true, false, false));
        assert_eq!(at(300.0, 97.0), edge(false, false, true, false));
        assert_eq!(at(300.0, 403.0), edge(false, false, false, true));
        assert_eq!(at(97.0, 97.0), edge(true, false, true, false));
        assert_eq!(at(503.0, 395.0), edge(false, true, false, true)); // beside the corner, within the grab
        assert_eq!(at(110.0, 403.0), edge(true, false, false, true));
    }

    #[test]
    fn size_and_shape() {
        let w = 200;
        let buf = pixels(w, true, false, None);
        assert_eq!(buf.len(), (w * HEIGHT * 4) as usize);
        // Top corners are see-through, the rest of the top row and the bottom corners opaque.
        assert_eq!(pixel(&buf, w, 0, 0)[3], 0);
        assert_eq!(pixel(&buf, w, w - 1, 0)[3], 0);
        assert_eq!(pixel(&buf, w, w / 2, 0), [0x2e, 0x2e, 0x2e, 255]);
        assert_eq!(pixel(&buf, w, 0, HEIGHT - 1), [0x1a, 0x1a, 0x1a, 255]);
    }

    #[test]
    fn traffic_lights() {
        let w = 200;
        let buf = pixels(w, true, false, None);
        let [(rx, ry), (yx, _), (gx, _)] = button_centres();
        let at = |x: f32| pixel(&buf, w, x as i32, ry as i32);
        assert_eq!(at(rx), [0x57, 0x5f, 0xff, 255]); // red (B, G, R, A)
        assert_eq!(at(yx), [0x2e, 0xbc, 0xfe, 255]); // yellow
        assert_eq!(at(gx), [0x40, 0xc8, 0x28, 255]); // green
        assert_eq!(at(rx + 10.0), [0x2e, 0x2e, 0x2e, 255]); // bar between the buttons
    }

    #[test]
    fn premultiplied_and_narrow_windows() {
        for w in [1, 5, 30, 61] {
            let buf = pixels(w, true, false, None);
            assert_eq!(buf.len(), (w * HEIGHT * 4) as usize);
            for p in buf.chunks(4) {
                assert!(p[0] <= p[3] && p[1] <= p[3] && p[2] <= p[3], "not premultiplied: {p:?}");
            }
        }
        // 39 px: the yellow button (centre 34, radius 6) doesn't fit, so only bar at its centre.
        let buf = pixels(39, true, false, None);
        assert_eq!(pixel(&buf, 39, 34, 14), [0x2e, 0x2e, 0x2e, 255]);
        assert_eq!(pixel(&pixels(40, true, false, None), 40, 34, 14), [0x2e, 0xbc, 0xfe, 255]);
        for look in [(false, false), (true, true), (false, true)] {
            for p in pixels(61, look.0, look.1, None).chunks(4) {
                assert!(p[0] <= p[3] && p[1] <= p[3] && p[2] <= p[3], "not premultiplied: {p:?}");
            }
        }
    }

    #[test]
    fn dimmed_when_not_focused() {
        let w = 200;
        let buf = pixels(w, false, false, None);
        assert_eq!(buf.len(), (w * HEIGHT * 4) as usize);
        let [(rx, ry), (yx, _), (gx, _)] = button_centres();
        let at = |x: f32| pixel(&buf, w, x as i32, ry as i32);
        for x in [rx, yx, gx] {
            assert_eq!(at(x), [0x4d, 0x4d, 0x4d, 255]); // grey, all three
        }
        assert_eq!(at(rx + 10.0), [0x26, 0x26, 0x26, 255]); // the flatter bar
        assert_eq!(pixel(&buf, w, 0, HEIGHT - 1), [0x1a, 0x1a, 0x1a, 255]); // same bottom line
        // Same shape as the lit bar: only the colours differ.
        let lit = pixels(w, true, false, None);
        assert!(buf.chunks(4).zip(lit.chunks(4)).all(|(a, b)| a[3] == b[3]));
    }

    #[test]
    fn title_blended_where_placed() {
        use crate::title_text::{place, Text};
        let w = 300;
        // A made-up 10 px title: one solid column at its x = 4, half ink at x = 6, row 10.
        let mut coverage = vec![0u8; (10 * HEIGHT) as usize];
        for y in 5..20 {
            coverage[(y * 10 + 4) as usize] = 255;
        }
        coverage[(10 * 10 + 6) as usize] = 128;
        let text = Text { width: 10, coverage };
        let x0 = place(w, 10);
        assert_eq!(x0, 145); // centred
        let lit = pixels(w, true, false, Some(&text));
        let dim = pixels(w, false, false, Some(&text));
        let plain = pixels(w, true, false, None);
        assert_eq!(pixel(&lit, w, x0 + 4, 10), [0xdd, 0xdd, 0xdd, 255]);
        assert_eq!(pixel(&dim, w, x0 + 4, 10), [0x7a, 0x7a, 0x7a, 255]);
        let half = pixel(&lit, w, x0 + 6, 10);
        assert!(half[0] > 0x2e && half[0] < 0xdd, "half ink {half:?}");
        // Everywhere else the bar is untouched, and the bottom line is never inked.
        let changed = lit.chunks(4).zip(plain.chunks(4)).filter(|(a, b)| a != b).count();
        assert_eq!(changed, 15 + 1);
        assert_eq!(pixel(&lit, w, x0 + 4, HEIGHT - 1), [0x1a, 0x1a, 0x1a, 255]);
    }

    #[test]
    fn symbols_on_hover() {
        let w = 200;
        let [(rx, ry), (yx, _), (gx, _)] = button_centres();
        let plain = pixels(w, true, false, None);
        for active in [true, false] {
            let buf = pixels(w, active, true, None);
            let at = |x: f32, dy: f32| pixel(&buf, w, x as i32, (ry + dy) as i32);
            // Every centre is inked (✕ crossing, − and + through it), in each button's dark shade.
            assert_eq!(at(rx, 0.0), [0x00, 0x00, 0x4d, 255]);
            assert_eq!(at(yx, 0.0), [0x00, 0x58, 0x99, 255]);
            assert_eq!(at(gx, 0.0), [0x00, 0x65, 0x00, 255]);
            // Crisp: − is one row, so the rows beside it are plain yellow.
            assert_eq!(at(yx, 1.0), [0x2e, 0xbc, 0xfe, 255]);
            // Above the centre: + has its upright, − doesn't (the plain yellow shows).
            assert_eq!(at(gx, -2.0), [0x00, 0x65, 0x00, 255]);
            assert_eq!(at(yx, -2.0), [0x2e, 0xbc, 0xfe, 255]);
            // Off the buttons the bar is the same as without symbols.
            assert_eq!(at(rx + 10.0, 0.0), pixel(&pixels(w, active, false, None), w, (rx + 10.0) as i32, ry as i32));
        }
        // The symbols stay inside the buttons: the lit bar without them differs only there.
        let buf = pixels(w, true, true, None);
        for (i, (a, b)) in buf.chunks(4).zip(plain.chunks(4)).enumerate() {
            if a != b {
                let (x, y) = ((i as i32 % w) as f32 + 0.5, (i as i32 / w) as f32 + 0.5);
                let near = button_centres().iter().any(|&(cx, cy)| ((x - cx).powi(2) + (y - cy).powi(2)).sqrt() < BUTTON_DIAMETER / 2.0);
                assert!(near, "symbol ink outside a button at ({x}, {y})");
            }
        }
    }
}
