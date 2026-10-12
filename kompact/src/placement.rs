//! Smart placement for new toplevels (Phase 2a).
//!
//! Pure geometry, no compositor state: given the usable area (output minus layer-shell exclusive
//! zones), the new window's size and the rectangles of windows already on screen, pick where the
//! new window goes. Kept free of `Kompact` so it can be exercised entirely by `cargo test`.
//!
//! Policy:
//! 1. **Open space first.** Try candidate positions built from the area's edges and the edges of
//!    existing windows, top-to-bottom then left-to-right, and take the first one where the window
//!    fits inside the area without overlapping anything.
//! 2. **Otherwise cascade, wrapping at the screen edge.** Step diagonally by `CASCADE_STEP`, and
//!    wrap each axis back to the area's start instead of running off-screen. Take the first slot
//!    that no existing window already sits on, so a new window never lands exactly on top of
//!    another one (e.g. after a window in the middle of the cascade was closed).

use smithay::utils::{Logical, Point, Rectangle, Size};

/// Diagonal offset between cascaded windows when no open space is left.
pub const CASCADE_STEP: i32 = 32;

/// Upper bound on how many existing windows are considered as obstacles. Keeps the candidate
/// search at O(MAX_OBSTACLES^3) worst case (~140k rectangle tests) no matter how many windows
/// are open. Callers pass the top-most windows first, so the ones that get ignored past this
/// limit are the most-buried ones.
pub const MAX_OBSTACLES: usize = 32;

/// Pick the location for a new window of `size` inside `area`.
///
/// `existing` is the on-screen geometry of the other windows, top-most first.
pub fn place(
    area: Rectangle<i32, Logical>,
    size: Size<i32, Logical>,
    existing: &[Rectangle<i32, Logical>],
) -> Point<i32, Logical> {
    let obstacles: Vec<_> = existing
        .iter()
        .take(MAX_OBSTACLES)
        .copied()
        .filter(|r| r.size.w > 0 && r.size.h > 0 && r.overlaps(area))
        .collect();

    if let Some(loc) = find_open_space(area, size, &obstacles) {
        return loc;
    }
    cascade(area, size, &obstacles)
}

fn find_open_space(
    area: Rectangle<i32, Logical>,
    size: Size<i32, Logical>,
    obstacles: &[Rectangle<i32, Logical>],
) -> Option<Point<i32, Logical>> {
    if size.w <= 0 || size.h <= 0 || size.w > area.size.w || size.h > area.size.h {
        return None;
    }
    let max_x = area.loc.x + area.size.w - size.w;
    let max_y = area.loc.y + area.size.h - size.h;

    // Candidate edges per axis: the area's own start/end, plus "just after" and "just before"
    // every obstacle. Any free spot can be slid left/up until it hits one of these without
    // creating an overlap, so checking only these positions still finds a fit if one exists.
    let mut xs = vec![area.loc.x, max_x];
    let mut ys = vec![area.loc.y, max_y];
    for r in obstacles {
        xs.push(r.loc.x + r.size.w);
        xs.push(r.loc.x - size.w);
        ys.push(r.loc.y + r.size.h);
        ys.push(r.loc.y - size.h);
    }
    xs.retain(|x| (area.loc.x..=max_x).contains(x));
    ys.retain(|y| (area.loc.y..=max_y).contains(y));
    xs.sort_unstable();
    xs.dedup();
    ys.sort_unstable();
    ys.dedup();

    for &y in &ys {
        for &x in &xs {
            let rect = Rectangle::new((x, y).into(), size);
            if !obstacles.iter().any(|o| o.overlaps(rect)) {
                return Some(rect.loc);
            }
        }
    }
    None
}

fn cascade(
    area: Rectangle<i32, Logical>,
    size: Size<i32, Logical>,
    obstacles: &[Rectangle<i32, Logical>],
) -> Point<i32, Logical> {
    // How far the window can slide on each axis and still be fully inside the area (0 if it's
    // as big as, or bigger than, the area on that axis: it then just sits at the area's start).
    let range_x = (area.size.w - size.w).max(0);
    let range_y = (area.size.h - size.h).max(0);
    let slots_x = (range_x / CASCADE_STEP) as usize + 1;
    let slots_y = (range_y / CASCADE_STEP) as usize + 1;
    // Walking the diagonal with both axes wrapping repeats after lcm(slots_x, slots_y) steps,
    // which can be well under the number of grid spots (24 of 288 for foot on 1440x900). Each
    // time it would repeat, start the next pass one step further down, so all
    // slots_x * slots_y spots get used before any comes round again.
    let period = slots_x / gcd(slots_x, slots_y) * slots_y;
    let slot = |k: usize| -> Point<i32, Logical> {
        let (i, pass) = (k % period, k / period);
        Point::from((
            area.loc.x + (i % slots_x) as i32 * CASCADE_STEP,
            area.loc.y + ((i + pass) % slots_y) as i32 * CASCADE_STEP,
        ))
    };
    // A slot is taken if some window's top-left corner is within half a step of it, which also
    // catches windows the user nudged a few pixels off the grid.
    let taken = |p: Point<i32, Logical>| {
        obstacles.iter().any(|o| {
            (o.loc.x - p.x).abs() < CASCADE_STEP / 2 && (o.loc.y - p.y).abs() < CASCADE_STEP / 2
        })
    };
    // Each obstacle can take at most one slot, so among the first `obstacles.len() + 1` slots
    // one is free unless there are fewer spots than that (or the window is as big as the area).
    // Then some overlap is unavoidable and the last slot tried is as good as any.
    let last = obstacles.len();
    (0..=last).map(slot).find(|&p| !taken(p)).unwrap_or_else(|| slot(last))
}

fn gcd(a: usize, b: usize) -> usize {
    if b == 0 { a } else { gcd(b, a % b) }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(x: i32, y: i32, w: i32, h: i32) -> Rectangle<i32, Logical> {
        Rectangle::new((x, y).into(), (w, h).into())
    }
    fn size(w: i32, h: i32) -> Size<i32, Logical> {
        (w, h).into()
    }
    fn inside(area: Rectangle<i32, Logical>, loc: Point<i32, Logical>, s: Size<i32, Logical>) -> bool {
        area.contains_rect(Rectangle::new(loc, s))
    }

    fn screen() -> Rectangle<i32, Logical> {
        rect(0, 0, 1920, 1080)
    }

    #[test]
    fn empty_screen_goes_to_top_left() {
        assert_eq!(place(screen(), size(800, 600), &[]), Point::from((0, 0)));
    }

    #[test]
    fn respects_usable_area_offset() {
        // e.g. a 30px panel along the top
        let area = rect(0, 30, 1920, 1050);
        assert_eq!(place(area, size(800, 600), &[]), Point::from((0, 30)));
    }

    #[test]
    fn second_window_goes_beside_first() {
        let first = rect(0, 0, 800, 600);
        assert_eq!(place(screen(), size(800, 600), &[first]), Point::from((800, 0)));
    }

    #[test]
    fn goes_below_when_row_is_full() {
        let a = rect(0, 0, 1000, 500);
        let b = rect(1000, 0, 920, 500);
        assert_eq!(place(screen(), size(800, 400), &[a, b]), Point::from((0, 500)));
    }

    #[test]
    fn fills_gap_between_windows() {
        let a = rect(0, 0, 600, 1080);
        let b = rect(1320, 0, 600, 1080);
        assert_eq!(place(screen(), size(700, 500), &[a, b]), Point::from((600, 0)));
    }

    #[test]
    fn falls_back_to_cascade_when_full() {
        let full = rect(0, 0, 1920, 1080);
        assert_eq!(place(screen(), size(800, 600), &[full]), Point::from((32, 32)));
    }

    /// Open `n` windows of size `s` one after another, each going on top, and return their
    /// locations in opening order.
    fn open_many(area: Rectangle<i32, Logical>, s: Size<i32, Logical>, n: usize) -> Vec<Point<i32, Logical>> {
        let mut stack: Vec<Rectangle<i32, Logical>> = Vec::new(); // top-most first
        let mut locs = Vec::new();
        for _ in 0..n {
            let loc = place(area, s, &stack);
            stack.insert(0, Rectangle::new(loc, s));
            locs.push(loc);
        }
        locs
    }

    #[test]
    fn cascade_wraps_and_stays_on_screen() {
        let s = size(800, 600);
        let locs = open_many(screen(), s, 500);
        for (n, loc) in locs.iter().enumerate() {
            assert!(inside(screen(), *loc, s), "window {n} placed {loc:?} off-screen");
        }
        assert_eq!(locs[1], Point::from((800, 0)));
        assert_eq!(locs[2], Point::from((32, 32)));
        // range_y = 480 -> 16 slots on y, so the cascade wraps back to the top at some point.
        assert!(locs.iter().skip(2).any(|l| l.y == 0 && l.x > 0), "cascade never wrapped on y");
    }

    #[test]
    fn cascade_starts_one_step_in() {
        // Hardware run 2026-10-07: 1440x900 with a 32px panel, two foot windows side by side.
        // The third used to land at +64,+64; it should be one step in from the corner.
        let area = rect(0, 32, 1440, 868);
        let s = size(696, 494);
        let existing = [rect(696, 32, 696, 494), rect(0, 32, 696, 494)];
        assert_eq!(place(area, s, &existing), Point::from((32, 64)));
    }

    #[test]
    fn cascade_skips_slots_already_taken() {
        // Hardware run 2026-10-07 09:19:26: a window at 96,128 already existed and the new one
        // was put exactly on top of it. The free slot at 32,64 must be used instead.
        let area = rect(0, 32, 1440, 868);
        let s = size(696, 494);
        let existing = [rect(64, 96, 696, 494), rect(96, 128, 696, 494), rect(0, 32, 696, 494)];
        assert_eq!(place(area, s, &existing), Point::from((32, 64)));
    }

    #[test]
    fn cascade_treats_nearby_window_as_taken() {
        let full = rect(0, 0, 1920, 1080);
        let nudged = rect(37, 29, 800, 600); // a few px off the 32,32 slot
        assert_eq!(place(screen(), size(800, 600), &[nudged, full]), Point::from((64, 64)));
    }

    #[test]
    fn many_windows_on_hardware_screen_all_get_their_own_corner() {
        // 1440x900 minus the panel, foot's default size: the plain diagonal only has 24 distinct
        // spots here, so this goes past where it used to start stacking windows on each other.
        let area = rect(0, 32, 1440, 868);
        let s = size(696, 494);
        let locs = open_many(area, s, 30);
        for (n, loc) in locs.iter().enumerate() {
            assert!(inside(area, *loc, s), "window {n} placed {loc:?} off-screen");
            assert!(!locs[..n].contains(loc), "window {n} placed on top of another at {loc:?}");
        }
    }

    #[test]
    fn no_two_windows_share_a_corner() {
        // Open, close one from the middle of the cascade, open again, repeatedly: no new window
        // may ever start where another one already starts.
        let area = rect(0, 32, 1440, 868);
        let s = size(696, 494);
        let mut stack: Vec<Rectangle<i32, Logical>> = Vec::new();
        for round in 0..40 {
            if round % 3 == 2 && stack.len() > 2 {
                stack.remove(stack.len() / 2);
            }
            let loc = place(area, s, &stack);
            assert!(inside(area, loc, s), "round {round}: {loc:?} off-screen");
            assert!(
                stack.iter().all(|r| r.loc != loc),
                "round {round}: {loc:?} lands exactly on an existing window"
            );
            stack.insert(0, Rectangle::new(loc, s));
        }
    }

    #[test]
    fn oversized_window_sits_at_area_origin() {
        let area = rect(0, 30, 1920, 1050);
        assert_eq!(place(area, size(2500, 1400), &[]), Point::from((0, 30)));
        // Too tall but not too wide: still cascades, horizontally only.
        let first = rect(0, 30, 800, 1400);
        assert_eq!(place(area, size(800, 1400), &[first]), Point::from((32, 30)));
    }

    #[test]
    fn ignores_windows_outside_the_area() {
        let off = rect(-2000, 0, 800, 600);
        assert_eq!(place(screen(), size(800, 600), &[off]), Point::from((0, 0)));
    }

    #[test]
    fn open_space_result_never_overlaps() {
        let existing = [rect(0, 0, 500, 400), rect(700, 100, 300, 300), rect(200, 500, 900, 200)];
        let s = size(600, 300);
        let loc = place(screen(), s, &existing);
        let r = Rectangle::new(loc, s);
        assert!(inside(screen(), loc, s));
        assert!(existing.iter().all(|e| !e.overlaps(r)), "{r:?} overlaps an existing window");
    }

    #[test]
    fn bounded_with_many_windows() {
        // Far more windows than MAX_OBSTACLES; must still return promptly and stay on screen.
        let existing: Vec<_> = (0..1000).map(|i| rect((i * 7) % 1800, (i * 13) % 1000, 120, 80)).collect();
        let s = size(640, 480);
        let loc = place(screen(), s, &existing);
        assert!(inside(screen(), loc, s));
    }

    #[test]
    fn degenerate_sizes_do_not_panic() {
        let _ = place(rect(0, 0, 0, 0), size(800, 600), &[]);
        let _ = place(screen(), size(0, 0), &[rect(0, 0, 100, 100)]);
    }
}
