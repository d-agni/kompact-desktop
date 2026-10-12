//! Edge/corner snapping (Phase 2b).
//!
//! Pure geometry and state transitions, no compositor state, so all of it is covered by
//! `cargo test`. `shell.rs` applies the results to real windows.
//!
//! - **Drag:** releasing a moved window with the pointer at the left/right screen edge snaps it
//!   to that half, at a corner to that quadrant, at the top edge it maximizes.
//! - **Keyboard:** `Super+Arrow` steps between floating, halves, quadrants and maximized, like
//!   most desktops do (see [`key_action`]).
//!
//! Region rectangles are computed from the usable area (output minus panels); edge detection
//! uses the full output, because that's where the pointer gets stopped.

use smithay::utils::{Logical, Point, Rectangle, Size};

/// How close (px) the pointer has to be to an output edge for a release to count as "at" it.
pub const EDGE_PX: f64 = 4.0;
/// How far (px) from a corner along an edge still counts as that corner.
pub const CORNER_PX: f64 = 80.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Region {
    Left,
    Right,
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
}

/// Where a dragged window should go when released.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DropTarget {
    Region(Region),
    Maximize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction {
    Left,
    Right,
    Up,
    Down,
}

/// A window's current layout, as far as `Super+Arrow` is concerned.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Layout {
    Floating,
    Snapped(Region),
    Maximized,
}

/// What `Super+Arrow` should do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Snap(Region),
    Maximize,
    /// Back to the floating geometry the window had before it was snapped/maximized.
    Restore,
    Nothing,
}

/// The rectangle a window snapped to `region` should occupy. Halves split odd sizes so the two
/// sides together cover the area exactly, with no gap or overlap.
pub fn region_rect(region: Region, area: Rectangle<i32, Logical>) -> Rectangle<i32, Logical> {
    let left_w = area.size.w / 2;
    let top_h = area.size.h / 2;
    let (x, w) = match region {
        Region::Left | Region::TopLeft | Region::BottomLeft => (area.loc.x, left_w),
        Region::Right | Region::TopRight | Region::BottomRight => {
            (area.loc.x + left_w, area.size.w - left_w)
        }
    };
    let (y, h) = match region {
        Region::Left | Region::Right => (area.loc.y, area.size.h),
        Region::TopLeft | Region::TopRight => (area.loc.y, top_h),
        Region::BottomLeft | Region::BottomRight => (area.loc.y + top_h, area.size.h - top_h),
    };
    Rectangle::new((x, y).into(), (w, h).into())
}

/// Where a window dropped with the pointer at `pos` goes, or `None` to leave it where it is.
pub fn drop_target(pos: Point<f64, Logical>, output: Rectangle<i32, Logical>) -> Option<DropTarget> {
    let left = output.loc.x as f64;
    let top = output.loc.y as f64;
    // The pointer is clamped to the last pixel, so measure against that, not one past it.
    let right = (output.loc.x + output.size.w - 1) as f64;
    let bottom = (output.loc.y + output.size.h - 1) as f64;

    let at_left = pos.x <= left + EDGE_PX;
    let at_right = pos.x >= right - EDGE_PX;
    let at_top = pos.y <= top + EDGE_PX;
    let at_bottom = pos.y >= bottom - EDGE_PX;
    let near_top = pos.y <= top + CORNER_PX;
    let near_bottom = pos.y >= bottom - CORNER_PX;
    let near_left = pos.x <= left + CORNER_PX;
    let near_right = pos.x >= right - CORNER_PX;

    let region = if (at_left && near_top) || (at_top && near_left) {
        Region::TopLeft
    } else if (at_right && near_top) || (at_top && near_right) {
        Region::TopRight
    } else if (at_left && near_bottom) || (at_bottom && near_left) {
        Region::BottomLeft
    } else if (at_right && near_bottom) || (at_bottom && near_right) {
        Region::BottomRight
    } else if at_left {
        Region::Left
    } else if at_right {
        Region::Right
    } else if at_top {
        return Some(DropTarget::Maximize);
    } else {
        return None;
    };
    Some(DropTarget::Region(region))
}

/// What `Super+<dir>` does to a window that is currently in `layout`.
///
/// From floating: Left/Right snap to that half, Up maximizes, Down does nothing. From a half:
/// the same direction does nothing, the opposite one restores, Up/Down go to that quadrant. From
/// a quadrant: towards the other half moves across, away from it goes back to the half (or
/// maximizes from a top quadrant). From maximized: Down restores, Left/Right snap to a half.
pub fn key_action(layout: Layout, dir: Direction) -> Action {
    use Direction as D;
    use Region as R;
    match (layout, dir) {
        (Layout::Floating, D::Left) => Action::Snap(R::Left),
        (Layout::Floating, D::Right) => Action::Snap(R::Right),
        (Layout::Floating, D::Up) => Action::Maximize,
        (Layout::Floating, D::Down) => Action::Nothing,

        (Layout::Snapped(R::Left), D::Left) => Action::Nothing,
        (Layout::Snapped(R::Left), D::Right) => Action::Restore,
        (Layout::Snapped(R::Left), D::Up) => Action::Snap(R::TopLeft),
        (Layout::Snapped(R::Left), D::Down) => Action::Snap(R::BottomLeft),

        (Layout::Snapped(R::Right), D::Right) => Action::Nothing,
        (Layout::Snapped(R::Right), D::Left) => Action::Restore,
        (Layout::Snapped(R::Right), D::Up) => Action::Snap(R::TopRight),
        (Layout::Snapped(R::Right), D::Down) => Action::Snap(R::BottomRight),

        (Layout::Snapped(R::TopLeft), D::Up) => Action::Maximize,
        (Layout::Snapped(R::TopLeft), D::Down) => Action::Snap(R::Left),
        (Layout::Snapped(R::TopLeft), D::Left) => Action::Nothing,
        (Layout::Snapped(R::TopLeft), D::Right) => Action::Snap(R::TopRight),

        (Layout::Snapped(R::TopRight), D::Up) => Action::Maximize,
        (Layout::Snapped(R::TopRight), D::Down) => Action::Snap(R::Right),
        (Layout::Snapped(R::TopRight), D::Right) => Action::Nothing,
        (Layout::Snapped(R::TopRight), D::Left) => Action::Snap(R::TopLeft),

        (Layout::Snapped(R::BottomLeft), D::Up) => Action::Snap(R::Left),
        (Layout::Snapped(R::BottomLeft), D::Down) => Action::Nothing,
        (Layout::Snapped(R::BottomLeft), D::Left) => Action::Nothing,
        (Layout::Snapped(R::BottomLeft), D::Right) => Action::Snap(R::BottomRight),

        (Layout::Snapped(R::BottomRight), D::Up) => Action::Snap(R::Right),
        (Layout::Snapped(R::BottomRight), D::Down) => Action::Nothing,
        (Layout::Snapped(R::BottomRight), D::Right) => Action::Nothing,
        (Layout::Snapped(R::BottomRight), D::Left) => Action::Snap(R::BottomLeft),

        (Layout::Maximized, D::Up) => Action::Nothing,
        (Layout::Maximized, D::Down) => Action::Restore,
        (Layout::Maximized, D::Left) => Action::Snap(R::Left),
        (Layout::Maximized, D::Right) => Action::Snap(R::Right),
    }
}

/// Where a window goes that was filling `filled` and is dragged out of it at `ptr` at its
/// floating `size`: under the pointer at the same fraction across, and as far below its top edge
/// as the pointer was (at most half its height), so it stays "in hand" as on macOS.
pub fn grab_restored(
    filled: Rectangle<i32, Logical>,
    size: Size<i32, Logical>,
    ptr: Point<f64, Logical>,
) -> Point<i32, Logical> {
    let frac = if filled.size.w > 0 {
        ((ptr.x - filled.loc.x as f64) / filled.size.w as f64).clamp(0.0, 1.0)
    } else {
        0.5
    };
    let down = (ptr.y - filled.loc.y as f64).clamp(0.0, (size.h / 2).max(0) as f64);
    Point::from(((ptr.x - frac * size.w as f64).round() as i32, (ptr.y - down).round() as i32))
}

/// Smallest size that counts as a real floating geometry to restore to. A window maximized
/// before its first frame only has the 0x0/1x1 of its initial commit.
pub const MIN_FLOATING: i32 = 64;

/// What to restore to: `saved` when it's a real size, otherwise `remembered` (the app's last
/// geometry), otherwise two thirds of `area`, centred.
pub fn floating_or_default(
    saved: Rectangle<i32, Logical>,
    remembered: Option<Rectangle<i32, Logical>>,
    area: Rectangle<i32, Logical>,
) -> Rectangle<i32, Logical> {
    let real = |r: &Rectangle<i32, Logical>| r.size.w >= MIN_FLOATING && r.size.h >= MIN_FLOATING;
    if real(&saved) {
        return saved;
    }
    if let Some(r) = remembered.filter(real) {
        return r;
    }
    let (w, h) = (area.size.w * 2 / 3, area.size.h * 2 / 3);
    Rectangle::new(
        (area.loc.x + (area.size.w - w) / 2, area.loc.y + (area.size.h - h) / 2).into(),
        (w, h).into(),
    )
}

/// 13f: a window being moved stops with its frame's top at `usable_top` (the menu bar's bottom
/// edge, or the monitor's top where there's no menu bar), as on macOS. `frame_y` is the top of
/// the frame (kompact's title bar included, as `place_window` takes it).
pub fn keep_below_top(frame_y: i32, usable_top: i32) -> i32 {
    frame_y.max(usable_top)
}

#[cfg(test)]
mod tests {

    #[test]
    fn moved_windows_stay_below_the_menu_bar() {
        // Menu bar 28 px high on a monitor at y=0: the frame's top (title bar) stops at 28.
        assert_eq!(keep_below_top(10, 28), 28);
        assert_eq!(keep_below_top(-50, 28), 28);
        assert_eq!(keep_below_top(300, 28), 300, "lower down: untouched");
        // A monitor lower in the layout (HDMI at y=132) without a menu bar.
        assert_eq!(keep_below_top(100, 132), 132);
    }

    use super::*;

    #[test]
    fn dragged_out_of_maximized_stays_under_pointer() {
        let filled = Rectangle::new((0, 28).into(), (1440, 802).into());
        // Grabbed three quarters across the title bar, 14 px down.
        let loc = grab_restored(filled, (600, 400).into(), (1080.0, 42.0).into());
        assert_eq!(loc, Point::from((630, 28)));
        // Pointer at the left edge: the window's left edge is under it.
        assert_eq!(grab_restored(filled, (600, 400).into(), (0.0, 40.0).into()), Point::from((0, 28)));
        // Grabbed far down (a client that drags from anywhere): at most half its height.
        assert_eq!(grab_restored(filled, (600, 400).into(), (720.0, 628.0).into()), Point::from((420, 428)));
    }

    #[test]
    fn restore_geometry_falls_back() {
        let area = Rectangle::new((0, 28).into(), (1440, 802).into());
        let tiny = Rectangle::new((0, 28).into(), (1, 1).into());
        let real = Rectangle::new((50, 60).into(), (702, 431).into());
        assert_eq!(floating_or_default(real, None, area), real);
        assert_eq!(floating_or_default(tiny, Some(real), area), real);
        assert_eq!(
            floating_or_default(tiny, None, area),
            Rectangle::new((240, 162).into(), (960, 534).into())
        );
    }

    const ALL: [Region; 6] = [
        Region::Left,
        Region::Right,
        Region::TopLeft,
        Region::TopRight,
        Region::BottomLeft,
        Region::BottomRight,
    ];

    fn rect(x: i32, y: i32, w: i32, h: i32) -> Rectangle<i32, Logical> {
        Rectangle::new((x, y).into(), (w, h).into())
    }
    fn pt(x: f64, y: f64) -> Point<f64, Logical> {
        (x, y).into()
    }
    /// The GT 710 setup: 1440x900 output, 32px layer-shell panel on top.
    fn output() -> Rectangle<i32, Logical> {
        rect(0, 0, 1440, 900)
    }
    fn usable() -> Rectangle<i32, Logical> {
        rect(0, 32, 1440, 868)
    }

    #[test]
    fn halves_on_hardware_screen() {
        assert_eq!(region_rect(Region::Left, usable()), rect(0, 32, 720, 868));
        assert_eq!(region_rect(Region::Right, usable()), rect(720, 32, 720, 868));
    }

    #[test]
    fn quadrants_on_hardware_screen() {
        assert_eq!(region_rect(Region::TopLeft, usable()), rect(0, 32, 720, 434));
        assert_eq!(region_rect(Region::TopRight, usable()), rect(720, 32, 720, 434));
        assert_eq!(region_rect(Region::BottomLeft, usable()), rect(0, 466, 720, 434));
        assert_eq!(region_rect(Region::BottomRight, usable()), rect(720, 466, 720, 434));
    }

    #[test]
    fn regions_stay_inside_the_usable_area() {
        for area in [usable(), rect(0, 0, 1441, 899), rect(-1920, 40, 1920, 1040), rect(5, 5, 3, 3)] {
            for r in ALL {
                let got = region_rect(r, area);
                assert!(area.contains_rect(got), "{r:?} in {area:?} -> {got:?}");
            }
        }
    }

    #[test]
    fn halves_and_quadrants_tile_odd_sizes_exactly() {
        let area = rect(10, 30, 1441, 869);
        let l = region_rect(Region::Left, area);
        let r = region_rect(Region::Right, area);
        assert!(!l.overlaps(r));
        assert_eq!(l.size.w + r.size.w, area.size.w);
        assert_eq!(l.loc.x + l.size.w, r.loc.x);

        let quads = [Region::TopLeft, Region::TopRight, Region::BottomLeft, Region::BottomRight]
            .map(|q| region_rect(q, area));
        let total: i32 = quads.iter().map(|q| q.size.w * q.size.h).sum();
        assert_eq!(total, area.size.w * area.size.h);
        for (i, a) in quads.iter().enumerate() {
            for b in &quads[i + 1..] {
                assert!(!a.overlaps(*b), "{a:?} overlaps {b:?}");
            }
        }
    }

    #[test]
    fn drop_at_side_edges_snaps_to_halves() {
        assert_eq!(drop_target(pt(0.0, 450.0), output()), Some(DropTarget::Region(Region::Left)));
        assert_eq!(drop_target(pt(1439.0, 450.0), output()), Some(DropTarget::Region(Region::Right)));
        assert_eq!(drop_target(pt(3.0, 300.0), output()), Some(DropTarget::Region(Region::Left)));
    }

    #[test]
    fn drop_at_top_edge_maximizes() {
        assert_eq!(drop_target(pt(720.0, 0.0), output()), Some(DropTarget::Maximize));
    }

    #[test]
    fn drop_in_corners_snaps_to_quadrants() {
        let o = output();
        assert_eq!(drop_target(pt(0.0, 0.0), o), Some(DropTarget::Region(Region::TopLeft)));
        assert_eq!(drop_target(pt(1439.0, 0.0), o), Some(DropTarget::Region(Region::TopRight)));
        assert_eq!(drop_target(pt(0.0, 899.0), o), Some(DropTarget::Region(Region::BottomLeft)));
        assert_eq!(drop_target(pt(1439.0, 899.0), o), Some(DropTarget::Region(Region::BottomRight)));
        // Along an edge but within CORNER_PX of the corner still counts as the corner.
        assert_eq!(drop_target(pt(0.0, 60.0), o), Some(DropTarget::Region(Region::TopLeft)));
        assert_eq!(drop_target(pt(60.0, 0.0), o), Some(DropTarget::Region(Region::TopLeft)));
        assert_eq!(drop_target(pt(1400.0, 899.0), o), Some(DropTarget::Region(Region::BottomRight)));
    }

    #[test]
    fn drop_away_from_edges_does_nothing() {
        let o = output();
        assert_eq!(drop_target(pt(720.0, 450.0), o), None);
        assert_eq!(drop_target(pt(10.0, 450.0), o), None); // close to, but not at, the edge
        assert_eq!(drop_target(pt(720.0, 899.0), o), None); // bottom edge middle: no target
    }

    #[test]
    fn drop_respects_output_offset() {
        let o = rect(1440, 0, 1920, 1080); // a second output to the right
        assert_eq!(drop_target(pt(1440.0, 500.0), o), Some(DropTarget::Region(Region::Left)));
        assert_eq!(drop_target(pt(3359.0, 500.0), o), Some(DropTarget::Region(Region::Right)));
        assert_eq!(drop_target(pt(1439.0 + 500.0, 500.0), o), None);
    }

    #[test]
    fn keyboard_from_floating() {
        assert_eq!(key_action(Layout::Floating, Direction::Left), Action::Snap(Region::Left));
        assert_eq!(key_action(Layout::Floating, Direction::Right), Action::Snap(Region::Right));
        assert_eq!(key_action(Layout::Floating, Direction::Up), Action::Maximize);
        assert_eq!(key_action(Layout::Floating, Direction::Down), Action::Nothing);
    }

    #[test]
    fn keyboard_opposite_direction_restores_from_half() {
        assert_eq!(key_action(Layout::Snapped(Region::Left), Direction::Right), Action::Restore);
        assert_eq!(key_action(Layout::Snapped(Region::Right), Direction::Left), Action::Restore);
        assert_eq!(key_action(Layout::Maximized, Direction::Down), Action::Restore);
    }

    #[test]
    fn keyboard_walks_half_quadrant_maximize() {
        let mut l = Layout::Floating;
        for (dir, want) in [
            (Direction::Left, Layout::Snapped(Region::Left)),
            (Direction::Up, Layout::Snapped(Region::TopLeft)),
            (Direction::Right, Layout::Snapped(Region::TopRight)),
            (Direction::Down, Layout::Snapped(Region::Right)),
            (Direction::Down, Layout::Snapped(Region::BottomRight)),
            (Direction::Left, Layout::Snapped(Region::BottomLeft)),
            (Direction::Up, Layout::Snapped(Region::Left)),
            (Direction::Up, Layout::Snapped(Region::TopLeft)),
            (Direction::Up, Layout::Maximized),
            (Direction::Down, Layout::Floating),
        ] {
            l = match key_action(l, dir) {
                Action::Snap(r) => Layout::Snapped(r),
                Action::Maximize => Layout::Maximized,
                Action::Restore => Layout::Floating,
                Action::Nothing => l,
            };
            assert_eq!(l, want, "after {dir:?}");
        }
    }

    #[test]
    fn keyboard_never_snaps_to_where_it_already_is() {
        for r in ALL {
            for d in [Direction::Left, Direction::Right, Direction::Up, Direction::Down] {
                assert_ne!(key_action(Layout::Snapped(r), d), Action::Snap(r), "{r:?} + {d:?}");
            }
        }
    }
}
