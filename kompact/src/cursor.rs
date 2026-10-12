//! Compositor-drawn cursors, rasterised once at startup: anti-aliased silhouette, solid black
//! outline, white fill (the classic look shared by KDE/GNOME/etc).
//!
//! - The default arrow, used unless a client asks for something else.
//! - Four double-headed resize arrows (Phase 2f) covering the eight resize directions: clients
//!   ask for them by name over their borders via cursor-shape-v1, and the compositor shows them
//!   itself during a resize drag.
//! - 8b: the other shapes apps ask for most: text (I-beam), hand (links, grab), crosshair,
//!   move, not-allowed. Shapes still without an image (wait, help, zoom, …) get the arrow.
//!
//! An earlier version also drew a soft drop shadow; on software-heavy rendering the shadow's
//! low-alpha gradient came through looking smudgy rather than soft, so this version is fully
//! opaque — crisp silhouette, no shadow.

use smithay::{
    backend::{allocator::Fourcc, renderer::element::memory::MemoryRenderBuffer},
    input::pointer::CursorIcon,
    utils::{Logical, Point, Transform},
};

use crate::resize::Edges;

const SIZE: usize = 40;
/// Where the arrow's tip sits inside the buffer (leaves room for the outline's AA fringe).
pub const HOTSPOT: (f64, f64) = (5.0, 5.0);
const SCALE: f32 = 1.3;
/// Outline thickness, in final-buffer pixels.
const OUTLINE: f32 = 1.1;

const ARROW: [(f32, f32); 7] = [
    (0.0, 0.0),
    (0.0, 16.5),
    (4.2, 12.6),
    (7.2, 19.6),
    (9.7, 18.6),
    (6.8, 11.7),
    (12.2, 11.7),
];

fn seg_dist(p: (f32, f32), a: (f32, f32), b: (f32, f32)) -> f32 {
    let (px, py) = (p.0 - a.0, p.1 - a.1);
    let (bx, by) = (b.0 - a.0, b.1 - a.1);
    let t = ((px * bx + py * by) / (bx * bx + by * by)).clamp(0.0, 1.0);
    let (dx, dy) = (px - bx * t, py - by * t);
    (dx * dx + dy * dy).sqrt()
}

fn inside(p: (f32, f32), poly: &[(f32, f32)]) -> bool {
    let mut c = false;
    let mut j = poly.len() - 1;
    for i in 0..poly.len() {
        let (a, b) = (poly[i], poly[j]);
        if (a.1 > p.1) != (b.1 > p.1) && p.0 < (b.0 - a.0) * (p.1 - a.1) / (b.1 - a.1) + a.0 {
            c = !c;
        }
        j = i;
    }
    c
}

/// Signed distance to the polygon: negative inside, positive outside.
fn sdf(p: (f32, f32), poly: &[(f32, f32)]) -> f32 {
    let mut d = f32::MAX;
    for i in 0..poly.len() {
        d = d.min(seg_dist(p, poly[i], poly[(i + 1) % poly.len()]));
    }
    if inside(p, poly) {
        -d
    } else {
        d
    }
}

/// Double-headed arrow along +x, centred on the origin, before `SCALE`: tips at +-ARROW_HALF,
/// heads `HEAD_LEN` long and `HEAD_HALF` wide either side, shaft `SHAFT_HALF` either side.
const ARROW_HALF: f32 = 9.0;
const HEAD_LEN: f32 = 5.5;
const HEAD_HALF: f32 = 5.0;
const SHAFT_HALF: f32 = 1.6;

/// Which cursor image to draw.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shape {
    Arrow,
    /// Left/right (also used for the e and w edges).
    ResizeEw,
    /// Up/down (n and s edges).
    ResizeNs,
    /// Top-left/bottom-right diagonal (nw and se corners).
    ResizeNwse,
    /// Top-right/bottom-left diagonal (ne and sw corners).
    ResizeNesw,
    /// 8b: I-beam over text.
    Text,
    /// 8b: pointing hand (links, buttons; also grab/grabbing).
    Hand,
    /// 8b: thin cross.
    Crosshair,
    /// 8b: four-way arrow.
    Move,
    /// 8b: circle with a slash.
    NotAllowed,
}

/// The image for a cursor a client asked for by name. Anything we don't draw falls back to the
/// arrow, which is what every client got before cursor-shape-v1 was supported.
pub fn shape_for_icon(icon: CursorIcon) -> Shape {
    shape_for(icon, more_shapes())
}

/// 8b: whether the newer shapes are drawn (`KOMPACT_CURSOR_SHAPES=off`: arrow and resize only).
fn more_shapes() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| {
        let on = crate::dmabuf::enabled_from_env(std::env::var("KOMPACT_CURSOR_SHAPES").ok().as_deref());
        tracing::info!("cursor shapes: {} (8b: text, hand, crosshair, move, not-allowed; KOMPACT_CURSOR_SHAPES=off for arrow and resize only)", if on { "all" } else { "arrow and resize only" });
        on
    })
}

/// `shape_for_icon`, with the 8b shapes or without.
pub fn shape_for(icon: CursorIcon, more: bool) -> Shape {
    use CursorIcon as I;
    let shape = match icon {
        I::EResize | I::WResize | I::EwResize | I::ColResize => Shape::ResizeEw,
        I::NResize | I::SResize | I::NsResize | I::RowResize => Shape::ResizeNs,
        I::NwResize | I::SeResize | I::NwseResize => Shape::ResizeNwse,
        I::NeResize | I::SwResize | I::NeswResize => Shape::ResizeNesw,
        I::Text | I::VerticalText => Shape::Text,
        I::Pointer | I::Grab | I::Grabbing => Shape::Hand,
        I::Crosshair | I::Cell => Shape::Crosshair,
        I::Move | I::AllScroll => Shape::Move,
        I::NotAllowed | I::NoDrop => Shape::NotAllowed,
        _ => Shape::Arrow,
    };
    let older = matches!(shape, Shape::Arrow | Shape::ResizeEw | Shape::ResizeNs | Shape::ResizeNwse | Shape::ResizeNesw);
    if more || older {
        shape
    } else {
        Shape::Arrow
    }
}

/// The image to show while the compositor itself resizes a window by `edges`.
pub fn shape_for_edges(e: Edges) -> Shape {
    match (e.left || e.right, e.top || e.bottom) {
        (true, false) => Shape::ResizeEw,
        (false, true) => Shape::ResizeNs,
        (true, true) if (e.left && e.top) || (e.right && e.bottom) => Shape::ResizeNwse,
        (true, true) => Shape::ResizeNesw,
        (false, false) => Shape::Arrow,
    }
}

/// All cursor images, made once. Each `MemoryRenderBuffer` keeps its own texture, so picking a
/// different one per frame costs nothing extra.
pub struct Cursors {
    arrow: MemoryRenderBuffer,
    ew: MemoryRenderBuffer,
    ns: MemoryRenderBuffer,
    nwse: MemoryRenderBuffer,
    nesw: MemoryRenderBuffer,
    text: MemoryRenderBuffer,
    hand: MemoryRenderBuffer,
    crosshair: MemoryRenderBuffer,
    moving: MemoryRenderBuffer,
    not_allowed: MemoryRenderBuffer,
}

impl Cursors {
    pub fn new() -> Self {
        Self {
            arrow: make_arrow(),
            ew: to_buffer(&resize_pixels(0.0)),
            ns: to_buffer(&resize_pixels(90.0)),
            nwse: to_buffer(&resize_pixels(45.0)),
            nesw: to_buffer(&resize_pixels(-45.0)),
            text: to_buffer(&text_pixels()),
            hand: to_buffer(&hand_pixels()),
            crosshair: to_buffer(&crosshair_pixels()),
            moving: to_buffer(&move_pixels()),
            not_allowed: to_buffer(&not_allowed_pixels()),
        }
    }

    /// The image for `shape` and its hotspot: the pixel inside it that sits on the pointer.
    pub fn get(&self, shape: Shape) -> (&MemoryRenderBuffer, Point<f64, Logical>) {
        let centre = Point::from((SIZE as f64 / 2.0, SIZE as f64 / 2.0));
        match shape {
            Shape::Arrow => (&self.arrow, Point::from(HOTSPOT)),
            Shape::ResizeEw => (&self.ew, centre),
            Shape::ResizeNs => (&self.ns, centre),
            Shape::ResizeNwse => (&self.nwse, centre),
            Shape::ResizeNesw => (&self.nesw, centre),
            Shape::Text => (&self.text, centre),
            Shape::Hand => (&self.hand, Point::from(HAND_HOTSPOT)),
            Shape::Crosshair => (&self.crosshair, centre),
            Shape::Move => (&self.moving, centre),
            Shape::NotAllowed => (&self.not_allowed, centre),
        }
    }
}

/// The resize arrow rotated by `degrees` (screen y points down, so +45 runs top-left to
/// bottom-right), centred in the buffer.
fn resize_pixels(degrees: f32) -> Vec<u8> {
    rasterize(&arrow_poly(degrees, 1.0))
}

/// The double-headed arrow's outline, rotated by `degrees` and scaled by `SCALE * size`.
fn arrow_poly(degrees: f32, size: f32) -> Vec<(f32, f32)> {
    let shape = [
        (-ARROW_HALF, 0.0),
        (-ARROW_HALF + HEAD_LEN, -HEAD_HALF),
        (-ARROW_HALF + HEAD_LEN, -SHAFT_HALF),
        (ARROW_HALF - HEAD_LEN, -SHAFT_HALF),
        (ARROW_HALF - HEAD_LEN, -HEAD_HALF),
        (ARROW_HALF, 0.0),
        (ARROW_HALF - HEAD_LEN, HEAD_HALF),
        (ARROW_HALF - HEAD_LEN, SHAFT_HALF),
        (-ARROW_HALF + HEAD_LEN, SHAFT_HALF),
        (-ARROW_HALF + HEAD_LEN, HEAD_HALF),
    ];
    let (sin, cos) = degrees.to_radians().sin_cos();
    let c = SIZE as f32 / 2.0;
    let k = SCALE * size;
    shape.iter().map(|&(x, y)| (c + (x * cos - y * sin) * k, c + (x * sin + y * cos) * k)).collect()
}

/// A shape given around the buffer's centre (before `SCALE`), placed there.
fn centred(points: &[(f32, f32)]) -> Vec<(f32, f32)> {
    let c = SIZE as f32 / 2.0;
    points.iter().map(|&(x, y)| (c + x * SCALE, c + y * SCALE)).collect()
}

/// 8b: the I-beam: a stem with a serif at each end, wide enough to keep some white inside.
fn text_pixels() -> Vec<u8> {
    let (w, h, stem, serif) = (3.5, 9.0, 1.4, 1.8);
    let poly = centred(&[
        (-w, -h),
        (w, -h),
        (w, -h + serif),
        (stem, -h + serif),
        (stem, h - serif),
        (w, h - serif),
        (w, h),
        (-w, h),
        (-w, h - serif),
        (-stem, h - serif),
        (-stem, -h + serif),
        (-w, -h + serif),
    ]);
    rasterize(&poly)
}

/// Where the hand's outline starts in the buffer (its fingertip is the hotspot).
const HAND_ORIGIN: (f32, f32) = (6.0, 4.0);
pub const HAND_HOTSPOT: (f64, f64) = (HAND_ORIGIN.0 as f64 + 5.5 * SCALE as f64, HAND_ORIGIN.1 as f64 + 1.0);

/// 8b: a pointing hand: index finger up from a fist with three knuckles, thumb out left.
fn hand_pixels() -> Vec<u8> {
    const HAND: [(f32, f32); 25] = [
        (4.0, 1.5),
        (4.4, 0.4),
        (5.5, 0.0),
        (6.6, 0.4),
        (7.0, 1.5),
        (7.0, 8.0),
        (8.0, 7.4),
        (9.3, 7.6),
        (10.0, 8.4),
        (11.0, 7.9),
        (12.3, 8.2),
        (12.9, 9.1),
        (13.7, 9.0),
        (14.6, 9.7),
        (14.8, 11.0),
        (14.8, 14.5),
        (13.6, 18.0),
        (12.6, 19.5),
        (5.6, 19.5),
        (4.5, 17.5),
        (1.2, 13.4),
        (0.6, 12.2),
        (1.0, 11.0),
        (2.2, 10.7),
        (4.0, 12.2),
    ];
    let poly: Vec<(f32, f32)> = HAND.iter().map(|&(x, y)| (HAND_ORIGIN.0 + x * SCALE, HAND_ORIGIN.1 + y * SCALE)).collect();
    rasterize(&poly)
}

/// 8b: two thin bars crossing at the centre.
fn crosshair_pixels() -> Vec<u8> {
    let (l, t) = (9.0, 1.3);
    let across = centred(&[(-l, -t), (l, -t), (l, t), (-l, t)]);
    let down = centred(&[(-t, -l), (t, -l), (t, l), (-t, l)]);
    rasterize_sdf(|p| sdf(p, &across).min(sdf(p, &down)))
}

/// 8b: the left/right and up/down arrows together, a little smaller.
fn move_pixels() -> Vec<u8> {
    let (ew, ns) = (arrow_poly(0.0, 0.95), arrow_poly(90.0, 0.95));
    rasterize_sdf(|p| sdf(p, &ew).min(sdf(p, &ns)))
}

/// 8b: a ring with a slash from top-left to bottom-right.
fn not_allowed_pixels() -> Vec<u8> {
    let c = SIZE as f32 / 2.0;
    let (r, w) = (7.5 * SCALE, 1.6 * SCALE);
    let (a, t) = (5.6, 1.6);
    let slash = centred(&[(-a - t, -a + t), (-a + t, -a - t), (a + t, a - t), (a - t, a + t)]);
    rasterize_sdf(|p| {
        let ring = (((p.0 - c).powi(2) + (p.1 - c).powi(2)).sqrt() - r).abs() - w;
        ring.min(sdf(p, &slash))
    })
}

fn to_buffer(px: &[u8]) -> MemoryRenderBuffer {
    MemoryRenderBuffer::from_slice(
        px,
        Fourcc::Argb8888,
        (SIZE as i32, SIZE as i32),
        1,
        Transform::Normal,
        None,
    )
}

fn make_arrow() -> MemoryRenderBuffer {
    let poly: Vec<(f32, f32)> = ARROW
        .iter()
        .map(|&(x, y)| (HOTSPOT.0 as f32 + x * SCALE, HOTSPOT.1 as f32 + y * SCALE))
        .collect();
    to_buffer(&rasterize(&poly))
}

/// Premultiplied BGRA pixels for a SIZE x SIZE buffer showing `poly` outlined in black, filled
/// white.
fn rasterize(poly: &[(f32, f32)]) -> Vec<u8> {
    rasterize_sdf(|p| sdf(p, poly))
}

/// As `rasterize`, for any signed distance (negative inside); 8b: unions are the min of two.
fn rasterize_sdf(dist: impl Fn((f32, f32)) -> f32) -> Vec<u8> {
    let mut px = vec![0u8; SIZE * SIZE * 4];
    for y in 0..SIZE {
        for x in 0..SIZE {
            let p = (x as f32 + 0.5, y as f32 + 0.5);
            let d = dist(p);

            // ~1.5px anti-aliasing band at the true silhouette edge; solid black outline
            // stepping to solid white fill further in. No shadow: fully opaque throughout.
            let shape_a = (0.75 - d).clamp(0.0, 1.0);
            let fill = (-d - OUTLINE + 0.75).clamp(0.0, 1.0); // 1 = white interior, 0 = outline

            let i = (y * SIZE + x) * 4;
            let c8 = (fill * shape_a * 255.0).round() as u8; // premultiplied: black outline = 0
            px[i..i + 4].copy_from_slice(&[c8, c8, c8, (shape_a * 255.0).round() as u8]); // B G R A
        }
    }
    px
}

#[cfg(test)]
mod tests {
    use super::*;
    use smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel::ResizeEdge;

    fn alpha(px: &[u8], x: usize, y: usize) -> u8 {
        px[(y * SIZE + x) * 4 + 3]
    }
    fn edges(e: ResizeEdge) -> Edges {
        Edges::from_xdg(e).unwrap()
    }

    #[test]
    fn all_eight_directions_have_a_resize_cursor() {
        use CursorIcon as I;
        for (icon, want) in [
            (I::WResize, Shape::ResizeEw),
            (I::EResize, Shape::ResizeEw),
            (I::NResize, Shape::ResizeNs),
            (I::SResize, Shape::ResizeNs),
            (I::NwResize, Shape::ResizeNwse),
            (I::SeResize, Shape::ResizeNwse),
            (I::NeResize, Shape::ResizeNesw),
            (I::SwResize, Shape::ResizeNesw),
        ] {
            assert_eq!(shape_for(icon, true), want, "{icon:?}");
        }
        assert_eq!(shape_for(I::Default, true), Shape::Arrow);
        assert_eq!(shape_for(I::Wait, true), Shape::Arrow, "no image yet");
        // KOMPACT_CURSOR_SHAPES=off: the 8b shapes go back to the arrow, resize stays.
        assert_eq!(shape_for(I::Text, false), Shape::Arrow);
        assert_eq!(shape_for(I::EResize, false), Shape::ResizeEw);
    }

    #[test]
    fn named_shapes_apps_ask_for() {
        use CursorIcon as I;
        for (icon, want) in [
            (I::Text, Shape::Text),
            (I::Pointer, Shape::Hand),
            (I::Grab, Shape::Hand),
            (I::Crosshair, Shape::Crosshair),
            (I::Move, Shape::Move),
            (I::NotAllowed, Shape::NotAllowed),
            (I::NoDrop, Shape::NotAllowed),
        ] {
            assert_eq!(shape_for(icon, true), want, "{icon:?}");
        }
    }

    #[test]
    fn new_shapes_cover_their_hotspot_and_fit() {
        let c = SIZE / 2;
        for (name, px, hot) in [
            ("text", text_pixels(), (c, c)),
            ("crosshair", crosshair_pixels(), (c, c)),
            ("move", move_pixels(), (c, c)),
            ("hand", hand_pixels(), (HAND_HOTSPOT.0 as usize, HAND_HOTSPOT.1 as usize + 1)),
        ] {
            assert!(alpha(&px, hot.0, hot.1) > 200, "{name}: hotspot not on the shape");
            for i in 0..SIZE {
                for (x, y) in [(i, 0), (i, SIZE - 1), (0, i), (SIZE - 1, i)] {
                    assert_eq!(alpha(&px, x, y), 0, "{name} clipped at {x},{y}");
                }
            }
        }
        // The ring's centre is see-through except for the slash; its rim is drawn.
        let na = not_allowed_pixels();
        assert!(alpha(&na, c, c) > 200, "slash through the centre");
        let rim = c - (7.5 * SCALE) as usize;
        assert!(alpha(&na, rim, c) > 200, "ring at the left");
        let off = (3.0 * SCALE) as usize;
        assert_eq!(alpha(&na, c + off, c - off), 0, "nothing towards the top-right inside the ring");
        // The I-beam is white inside, not solid outline.
        let t = text_pixels();
        assert!(t[(c * SIZE + c) * 4] > 100, "stem has a white core");
    }

    #[test]
    fn compositor_resize_edges_match_client_names() {
        assert_eq!(shape_for_edges(edges(ResizeEdge::Left)), Shape::ResizeEw);
        assert_eq!(shape_for_edges(edges(ResizeEdge::Right)), Shape::ResizeEw);
        assert_eq!(shape_for_edges(edges(ResizeEdge::Top)), Shape::ResizeNs);
        assert_eq!(shape_for_edges(edges(ResizeEdge::Bottom)), Shape::ResizeNs);
        assert_eq!(shape_for_edges(edges(ResizeEdge::TopLeft)), Shape::ResizeNwse);
        assert_eq!(shape_for_edges(edges(ResizeEdge::BottomRight)), Shape::ResizeNwse);
        assert_eq!(shape_for_edges(edges(ResizeEdge::TopRight)), Shape::ResizeNesw);
        assert_eq!(shape_for_edges(edges(ResizeEdge::BottomLeft)), Shape::ResizeNesw);
    }

    #[test]
    fn resize_arrows_are_centred_and_point_the_right_way() {
        let c = SIZE / 2;
        let reach = (ARROW_HALF * SCALE) as usize - 2; // just inside the tips
        let ew = resize_pixels(0.0);
        assert_eq!(alpha(&ew, c, c), 255, "shaft covers the hotspot");
        assert!(alpha(&ew, c - reach, c) > 0 && alpha(&ew, c + reach, c) > 0);
        assert_eq!(alpha(&ew, c, c - reach), 0, "horizontal arrow has nothing above");

        let ns = resize_pixels(90.0);
        assert!(alpha(&ns, c, c - reach) > 0 && alpha(&ns, c, c + reach) > 0);
        assert_eq!(alpha(&ns, c - reach, c), 0);

        let d = (reach as f32 / std::f32::consts::SQRT_2) as usize;
        let nwse = resize_pixels(45.0);
        assert!(alpha(&nwse, c - d, c - d) > 0 && alpha(&nwse, c + d, c + d) > 0);
        assert_eq!(alpha(&nwse, c + d, c - d), 0, "nwse has nothing towards the top-right");
        let nesw = resize_pixels(-45.0);
        assert!(alpha(&nesw, c + d, c - d) > 0 && alpha(&nesw, c - d, c + d) > 0);
        assert_eq!(alpha(&nesw, c - d, c - d), 0);
    }

    #[test]
    fn shapes_fit_inside_the_buffer() {
        // No opaque pixels on the outermost ring, so nothing got clipped.
        for deg in [0.0, 90.0, 45.0, -45.0] {
            let px = resize_pixels(deg);
            for i in 0..SIZE {
                for (x, y) in [(i, 0), (i, SIZE - 1), (0, i), (SIZE - 1, i)] {
                    assert_eq!(alpha(&px, x, y), 0, "{deg} deg clipped at {x},{y}");
                }
            }
        }
    }
}
