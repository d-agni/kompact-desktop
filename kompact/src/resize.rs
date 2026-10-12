//! Interactive resize geometry (Phase 2f).
//!
//! Pure functions, covered by `cargo test`: which edges a resize grabs, and the rectangle that
//! results from dragging them. Dragging the left/top edge moves the window so the opposite edge
//! stays where it was.

use smithay::{
    reexports::wayland_protocols::xdg::shell::server::xdg_toplevel::ResizeEdge,
    utils::{Logical, Point, Rectangle, Size},
};

/// Smallest size an interactive resize will ask for, whatever the client says its minimum is.
pub const MIN_SIZE: (i32, i32) = (120, 80);

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Edges {
    pub left: bool,
    pub right: bool,
    pub top: bool,
    pub bottom: bool,
}

impl Edges {
    pub const BOTTOM_RIGHT: Edges = Edges { left: false, right: true, top: false, bottom: true };

    /// From the edge a client passed in `xdg_toplevel.resize`. `None` for `ResizeEdge::None` (or
    /// an unknown value): there's nothing to resize.
    pub fn from_xdg(edge: ResizeEdge) -> Option<Edges> {
        let e = |left, right, top, bottom| Some(Edges { left, right, top, bottom });
        match edge {
            ResizeEdge::Top => e(false, false, true, false),
            ResizeEdge::Bottom => e(false, false, false, true),
            ResizeEdge::Left => e(true, false, false, false),
            ResizeEdge::Right => e(false, true, false, false),
            ResizeEdge::TopLeft => e(true, false, true, false),
            ResizeEdge::TopRight => e(false, true, true, false),
            ResizeEdge::BottomLeft => e(true, false, false, true),
            ResizeEdge::BottomRight => e(false, true, false, true),
            _ => None,
        }
    }

    /// Whether resizing with these edges moves the window's origin.
    pub fn moves_origin(self) -> bool {
        self.left || self.top
    }
}

/// For `Super+right-drag`: grab whichever edges/corner the pointer is nearest. The window is
/// split into thirds on each axis; the outer thirds grab that edge, the middle grabs neither.
/// Grabbing dead centre falls back to the bottom-right corner (the old behaviour).
pub fn nearest_edges(ptr: Point<f64, Logical>, rect: Rectangle<i32, Logical>) -> Edges {
    let fx = (ptr.x - rect.loc.x as f64) / rect.size.w.max(1) as f64;
    let fy = (ptr.y - rect.loc.y as f64) / rect.size.h.max(1) as f64;
    let edges = Edges {
        left: fx < 1.0 / 3.0,
        right: fx > 2.0 / 3.0,
        top: fy < 1.0 / 3.0,
        bottom: fy > 2.0 / 3.0,
    };
    if edges == Edges::default() {
        Edges::BOTTOM_RIGHT
    } else {
        edges
    }
}

/// The window rectangle after dragging `edges` of `start` by `delta`, keeping the size within
/// the client's `min`/`max` (0 = unconstrained, as in xdg-shell) and never below `MIN_SIZE`.
/// The edges not being dragged stay exactly where they were.
pub fn resize_rect(
    start: Rectangle<i32, Logical>,
    edges: Edges,
    delta: Point<f64, Logical>,
    min: Size<i32, Logical>,
    max: Size<i32, Logical>,
) -> Rectangle<i32, Logical> {
    let dx = delta.x.round() as i32;
    let dy = delta.y.round() as i32;
    let (x, w) = axis(start.loc.x, start.size.w, edges.left, edges.right, dx, min.w.max(MIN_SIZE.0), max.w);
    let (y, h) = axis(start.loc.y, start.size.h, edges.top, edges.bottom, dy, min.h.max(MIN_SIZE.1), max.h);
    Rectangle::new((x, y).into(), (w, h).into())
}

/// One axis of `resize_rect`. `low`/`high` = dragging the left/top or right/bottom edge.
fn axis(pos: i32, len: i32, low: bool, high: bool, d: i32, min: i32, max: i32) -> (i32, i32) {
    let hi = if max > 0 { max.max(min) } else { i32::MAX };
    let new_len = if low {
        len - d
    } else if high {
        len + d
    } else {
        len
    };
    let new_len = new_len.clamp(min, hi);
    let new_pos = if low { pos + len - new_len } else { pos };
    (new_pos, new_len)
}

/// Where a window being resized with `edges` from `start` should sit once the client has
/// committed `current` size: the edges opposite the dragged ones stay fixed.
pub fn anchored_location(
    start: Rectangle<i32, Logical>,
    edges: Edges,
    current: Size<i32, Logical>,
) -> Point<i32, Logical> {
    let x = if edges.left { start.loc.x + start.size.w - current.w } else { start.loc.x };
    let y = if edges.top { start.loc.y + start.size.h - current.h } else { start.loc.y };
    Point::from((x, y))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(x: i32, y: i32, w: i32, h: i32) -> Rectangle<i32, Logical> {
        Rectangle::new((x, y).into(), (w, h).into())
    }
    fn d(x: f64, y: f64) -> Point<f64, Logical> {
        (x, y).into()
    }
    fn none() -> Size<i32, Logical> {
        (0, 0).into()
    }
    fn edge(name: ResizeEdge) -> Edges {
        Edges::from_xdg(name).unwrap()
    }
    /// A foot window as placed on the GT 710.
    fn start() -> Rectangle<i32, Logical> {
        rect(100, 100, 696, 494)
    }

    #[test]
    fn right_and_bottom_grow_without_moving() {
        assert_eq!(resize_rect(start(), edge(ResizeEdge::Right), d(50.0, 30.0), none(), none()), rect(100, 100, 746, 494));
        assert_eq!(resize_rect(start(), edge(ResizeEdge::Bottom), d(50.0, 30.0), none(), none()), rect(100, 100, 696, 524));
        assert_eq!(
            resize_rect(start(), edge(ResizeEdge::BottomRight), d(50.0, 30.0), none(), none()),
            rect(100, 100, 746, 524)
        );
    }

    #[test]
    fn left_and_top_keep_the_opposite_edge_fixed() {
        let r = resize_rect(start(), edge(ResizeEdge::Left), d(-40.0, 0.0), none(), none());
        assert_eq!(r, rect(60, 100, 736, 494));
        assert_eq!(r.loc.x + r.size.w, start().loc.x + start().size.w);

        let r = resize_rect(start(), edge(ResizeEdge::TopLeft), d(30.0, 20.0), none(), none());
        assert_eq!(r, rect(130, 120, 666, 474));
        assert_eq!(r.loc.x + r.size.w, 796);
        assert_eq!(r.loc.y + r.size.h, 594);
    }

    #[test]
    fn mixed_corners() {
        assert_eq!(
            resize_rect(start(), edge(ResizeEdge::TopRight), d(10.0, -10.0), none(), none()),
            rect(100, 90, 706, 504)
        );
        assert_eq!(
            resize_rect(start(), edge(ResizeEdge::BottomLeft), d(-10.0, 10.0), none(), none()),
            rect(90, 100, 706, 504)
        );
    }

    #[test]
    fn single_edge_ignores_motion_on_the_other_axis() {
        let r = resize_rect(start(), edge(ResizeEdge::Left), d(-40.0, 300.0), none(), none());
        assert_eq!((r.loc.y, r.size.h), (100, 494));
    }

    #[test]
    fn never_below_floor_or_client_minimum() {
        // Shrinking far past zero from the left: clamps, and the right edge still doesn't move.
        let r = resize_rect(start(), edge(ResizeEdge::Left), d(5000.0, 0.0), none(), none());
        assert_eq!(r.size.w, MIN_SIZE.0);
        assert_eq!(r.loc.x + r.size.w, 796);

        let min: Size<i32, Logical> = (300, 200).into();
        let r = resize_rect(start(), edge(ResizeEdge::BottomRight), d(-5000.0, -5000.0), min, none());
        assert_eq!(r, rect(100, 100, 300, 200));
    }

    #[test]
    fn respects_client_maximum() {
        let max: Size<i32, Logical> = (800, 600).into();
        let r = resize_rect(start(), edge(ResizeEdge::TopLeft), d(-500.0, -500.0), none(), max);
        assert_eq!(r, rect(-4, -6, 800, 600));
        // A max smaller than the floor/min can't shrink below the min.
        let tiny: Size<i32, Logical> = (50, 50).into();
        let r = resize_rect(start(), edge(ResizeEdge::BottomRight), d(0.0, 0.0), none(), tiny);
        assert_eq!(r.size, Size::from(MIN_SIZE));
    }

    #[test]
    fn from_xdg_none_means_no_resize() {
        assert_eq!(Edges::from_xdg(ResizeEdge::None), None);
    }

    #[test]
    fn nearest_edges_by_thirds() {
        let r = rect(0, 0, 900, 600);
        let at = |x, y| nearest_edges(d(x, y), r);
        assert_eq!(at(10.0, 10.0), edge(ResizeEdge::TopLeft));
        assert_eq!(at(890.0, 590.0), edge(ResizeEdge::BottomRight));
        assert_eq!(at(450.0, 10.0), edge(ResizeEdge::Top));
        assert_eq!(at(10.0, 300.0), edge(ResizeEdge::Left));
        assert_eq!(at(890.0, 10.0), edge(ResizeEdge::TopRight));
        // Dead centre: old bottom-right behaviour.
        assert_eq!(at(450.0, 300.0), Edges::BOTTOM_RIGHT);
    }

    #[test]
    fn anchored_location_follows_committed_size() {
        // Asked for 736 wide from the left, client only managed 730: the right edge (796) stays put.
        let loc = anchored_location(start(), edge(ResizeEdge::Left), (730, 494).into());
        assert_eq!(loc, Point::from((66, 100)));
        assert_eq!(loc.x + 730, 796);
        // Right/bottom resizes never move the window.
        assert_eq!(anchored_location(start(), edge(ResizeEdge::BottomRight), (900, 700).into()), start().loc);
    }
}
