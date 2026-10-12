//! 6m-e7: the window's title in kompact's title bar, centred as on macOS.
//!
//! The font is a system TrueType/OpenType file read once at startup (`init`): `KOMPACT_TITLE_FONT`
//! if set, else Ubuntu Sans (a variable font, drawn at `WEIGHT`), else what `fc-match` picks for
//! sans-serif, else DejaVu Sans. Without any, bars simply have no title. `ab_glyph` (pure Rust)
//! turns the title into a coverage mask once per title or width change; `titlebar::pixels`
//! blends it into the bar image, so nothing here runs per frame.
//!
//! `room`, `place` and `fit` are the pure layout rules: centred on the whole bar, but never
//! under the buttons, and cut short with "…" when it doesn't fit.

use ab_glyph::{point, Font, FontVec, GlyphId, PxScale, ScaleFont, VariableFont};

/// Text size in px (macOS: 13 pt at 1×).
pub const SIZE: f32 = 13.0;
/// For variable fonts with a weight axis (macOS titles are semibold-ish; 400 is regular).
const WEIGHT: f32 = 600.0;
/// The title starts right of this: the last button's edge (54 + 6) plus a 12 px gap.
pub const LEFT: i32 = 72;
pub const RIGHT_MARGIN: i32 = 12;
/// Less room than this and the bar gets no title at all.
const MIN_ROOM: i32 = 24;

/// A laid-out title: `width` × `titlebar::HEIGHT` coverage values (0–255), baseline centred.
#[derive(Debug, Clone, PartialEq)]
pub struct Text {
    pub width: i32,
    pub coverage: Vec<u8>,
}

/// The width a title may take in a bar `width` px wide, or `None` if too narrow for one.
pub fn room(width: i32) -> Option<i32> {
    let room = width - LEFT - RIGHT_MARGIN;
    (room >= MIN_ROOM).then_some(room)
}

/// Where a title `text_w` px wide starts: centred on the bar, pushed right of the buttons if
/// that would cover them, and left again if it would run past the right margin.
pub fn place(width: i32, text_w: i32) -> i32 {
    ((width - text_w) / 2).max(LEFT).min(width - RIGHT_MARGIN - text_w).max(LEFT)
}

/// 10d-3: `place` with the buttons on the left or (`right`) the right: then the margins swap.
pub fn place_side(width: i32, text_w: i32, right: bool) -> i32 {
    if !right {
        return place(width, text_w);
    }
    ((width - text_w) / 2).min(width - LEFT - text_w).max(RIGHT_MARGIN)
}

/// How many characters (with these advances) fit in `max` px, and whether an ellipsis
/// (`ellipsis` px) goes after them.
pub fn fit(advances: &[f32], ellipsis: f32, max: f32) -> (usize, bool) {
    if advances.iter().sum::<f32>() <= max {
        return (advances.len(), false);
    }
    let mut used = 0.0;
    let mut n = 0;
    for &a in advances {
        if used + a + ellipsis > max {
            break;
        }
        used += a;
        n += 1;
    }
    (n, true)
}

/// Load the font now, so the first bar doesn't wait for it.
pub fn init() {
    let _ = font();
}

fn font() -> Option<&'static FontVec> {
    static FONT: std::sync::OnceLock<Option<FontVec>> = std::sync::OnceLock::new();
    FONT.get_or_init(|| {
        let found = candidates().into_iter().find_map(|path| {
            let mut font = FontVec::try_from_vec(std::fs::read(&path).ok()?).ok()?;
            let weighted = font.set_variation(b"wght", WEIGHT);
            Some((font, path, weighted))
        });
        match found {
            Some((font, path, weighted)) => {
                let weight = if weighted { format!(", weight {WEIGHT}") } else { String::new() };
                tracing::info!("title text: font {path}{weight} (6m-e7; KOMPACT_TITLE_FONT=<file> for another)");
                Some(font)
            }
            None => {
                tracing::warn!("title text: no usable font found, title bars show no titles (KOMPACT_TITLE_FONT=<file>)");
                None
            }
        }
    })
    .as_ref()
}

fn candidates() -> Vec<String> {
    let mut paths: Vec<String> = std::env::var("KOMPACT_TITLE_FONT").ok().filter(|p| !p.is_empty()).into_iter().collect();
    paths.push("/usr/share/fonts/truetype/ubuntu/UbuntuSans[wdth,wght].ttf".into());
    if let Ok(out) = std::process::Command::new("fc-match").args(["-f", "%{file}", "sans-serif:weight=medium"]).output() {
        let path = String::from_utf8_lossy(&out.stdout).trim().to_string();
        if out.status.success() && !path.is_empty() {
            paths.push(path);
        }
    }
    paths.push("/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf".into());
    paths
}

/// `title` laid out in at most `max` px (cut short with "…"), or `None` without a font or text.
pub fn layout(title: &str, max: i32) -> Option<Text> {
    let font = font()?;
    let scaled = font.as_scaled(PxScale::from(SIZE));
    let chars: Vec<char> = title.trim().chars().filter(|c| !c.is_control()).collect();
    if chars.is_empty() {
        return None;
    }
    let ids: Vec<GlyphId> = chars.iter().map(|&c| scaled.glyph_id(c)).collect();
    let advance = |i: usize, ids: &[GlyphId]| scaled.h_advance(ids[i]) + if i > 0 { scaled.kern(ids[i - 1], ids[i]) } else { 0.0 };
    let advances: Vec<f32> = (0..ids.len()).map(|i| advance(i, &ids)).collect();
    // "…" if the font has it, else three dots.
    let ellipsis: Vec<GlyphId> = match scaled.glyph_id('…') {
        GlyphId(0) => vec![scaled.glyph_id('.'); 3],
        id => vec![id],
    };
    let ellipsis_w: f32 = ellipsis.iter().map(|&id| scaled.h_advance(id)).sum();
    let (n, cut) = fit(&advances, ellipsis_w, max as f32);
    let mut glyphs = ids[..n].to_vec();
    if cut {
        // No space right before the "…".
        while glyphs.len() > 0 && chars[glyphs.len() - 1].is_whitespace() {
            glyphs.pop();
        }
        if glyphs.is_empty() {
            return None;
        }
        glyphs.extend(ellipsis);
    }

    let height = crate::titlebar::HEIGHT;
    // Centre the ascent–descent box on the bar (descent is negative).
    let baseline = (height as f32 / 2.0 + (scaled.ascent() + scaled.descent()) / 2.0).round();
    let width: f32 = (0..glyphs.len()).map(|i| advance(i, &glyphs)).sum();
    let width = (width.ceil() as i32 + 1).min(max.max(1));
    let mut coverage = vec![0u8; (width * height) as usize];
    let mut x = 0.0;
    for i in 0..glyphs.len() {
        x += if i > 0 { scaled.kern(glyphs[i - 1], glyphs[i]) } else { 0.0 };
        let glyph = glyphs[i].with_scale_and_position(SIZE, point(x, baseline));
        x += scaled.h_advance(glyphs[i]);
        let Some(outlined) = font.outline_glyph(glyph) else { continue };
        let bounds = outlined.px_bounds();
        outlined.draw(|gx, gy, c| {
            let (px, py) = (bounds.min.x as i32 + gx as i32, bounds.min.y as i32 + gy as i32);
            if px >= 0 && px < width && py >= 0 && py < height {
                let cell = &mut coverage[(py * width + px) as usize];
                *cell = (*cell as f32 + c.clamp(0.0, 1.0) * 255.0).min(255.0).round() as u8;
            }
        });
    }
    Some(Text { width, coverage })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn room_and_place() {
        assert_eq!(room(640), Some(640 - LEFT - RIGHT_MARGIN));
        assert_eq!(room(LEFT + RIGHT_MARGIN + 23), None); // too narrow for a title
        // Short title: centred on the whole bar.
        assert_eq!(place(640, 40), 300);
        // Centring would put it under the buttons: pushed right of them.
        assert_eq!(place(200, 100), LEFT);
        // As wide as the room: from LEFT to the right margin.
        assert_eq!(place(640, 640 - LEFT - RIGHT_MARGIN), LEFT);
        // 10d-3, buttons on the right: centred, kept left of them, never past the left margin.
        assert_eq!(place_side(640, 40, true), 300);
        assert_eq!(place_side(200, 100, true), 200 - LEFT - 100);
        assert_eq!(place_side(640, 640 - LEFT - RIGHT_MARGIN, true), RIGHT_MARGIN);
    }

    #[test]
    fn fits_or_cuts_with_ellipsis() {
        let a = [10.0; 5]; // 50 px of text
        assert_eq!(fit(&a, 8.0, 50.0), (5, false)); // fits exactly, no ellipsis
        assert_eq!(fit(&a, 8.0, 49.0), (4, true)); // 40 + 8 ≤ 49
        assert_eq!(fit(&a, 8.0, 47.0), (3, true)); // 40 + 8 > 47, so 30 + 8
        assert_eq!(fit(&a, 8.0, 5.0), (0, true));
        assert_eq!(fit(&[], 8.0, 5.0), (0, false));
    }

    /// With a font on this machine (the GT 710 has several): real text, inside its room.
    #[test]
    fn real_font_layout() {
        let Some(short) = layout("foot", 500) else {
            eprintln!("no font here; skipping");
            return;
        };
        assert!(short.width > 10 && short.width < 60, "width {}", short.width);
        assert_eq!(short.coverage.len(), (short.width * crate::titlebar::HEIGHT) as usize);
        assert!(short.coverage.iter().any(|&c| c == 255), "no solid pixels");
        // Ink only between the top and bottom few rows (the bar's bottom line stays clear).
        let row_ink = |y: i32| (0..short.width).any(|x| short.coverage[(y * short.width + x) as usize] > 0);
        assert!(!row_ink(0) && !row_ink(crate::titlebar::HEIGHT - 1));
        // A long title is cut to the room given.
        let long = layout(&"very long window title ".repeat(10), 150).unwrap();
        assert!(long.width <= 150);
        assert!(layout("   ", 150).is_none());
        assert!(layout("x", 0).map_or(true, |t| t.width <= 1));
    }
}
