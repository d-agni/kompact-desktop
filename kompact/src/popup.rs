//! Popup placement (context menus, dropdowns, tooltips).
//!
//! xdg-shell positions a popup relative to its parent's window geometry. To keep it on screen,
//! Smithay's `PositionerState::get_unconstrained_geometry` flips/slides/resizes it to fit a
//! target rectangle given in that same parent-relative space; this computes that rectangle.

use smithay::utils::{Logical, Point, Rectangle};

/// The output rectangle expressed relative to the popup's parent.
///
/// - `root_loc`: global position of the root surface's window geometry (the toplevel or layer
///   surface the popup chain hangs off).
/// - `parent_offset`: the parent's offset from the root (non-zero for a submenu), as returned by
///   Smithay's `get_popup_toplevel_coords`.
pub fn constraint_target(
    output: Rectangle<i32, Logical>,
    root_loc: Point<i32, Logical>,
    parent_offset: Point<i32, Logical>,
) -> Rectangle<i32, Logical> {
    let mut target = output;
    target.loc -= root_loc;
    target.loc -= parent_offset;
    target
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(x: i32, y: i32, w: i32, h: i32) -> Rectangle<i32, Logical> {
        Rectangle::new((x, y).into(), (w, h).into())
    }

    #[test]
    fn target_is_output_seen_from_the_parent() {
        // gedit at 262,127 on the 1440x900 output: the screen starts 262 px left of and 127 px
        // above the window's own origin.
        let t = constraint_target(rect(0, 0, 1440, 900), (262, 127).into(), (0, 0).into());
        assert_eq!(t, rect(-262, -127, 1440, 900));
    }

    #[test]
    fn submenu_offsets_by_its_parent_menu() {
        let t = constraint_target(rect(0, 0, 1440, 900), (262, 127).into(), (300, 40).into());
        assert_eq!(t, rect(-562, -167, 1440, 900));
    }

    #[test]
    fn window_partly_off_screen() {
        let t = constraint_target(rect(0, 0, 1440, 900), (-100, 600).into(), (0, 0).into());
        assert_eq!(t, rect(100, -600, 1440, 900));
    }
}
