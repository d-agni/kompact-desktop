//! Debug overlay (Phase 3c): a small box in the top-right corner with live numbers, toggled with
//! `Super+D`, off by default. For us, not end users: when something feels slow or leaky there's
//! a number on screen instead of a guess.
//!
//! - The image is one fixed-size `MemoryRenderBuffer`, updated *in place* once a second (never per
//!   frame), so it keeps a single texture and can't itself grow memory during a soak test.
//! - Timing comes from `render.rs` without touching the backends: the DRM backend builds elements
//!   once per render pass and never while a page flip is pending, so while something is moving
//!   "passes/s" is the frame rate, and the largest gap between passes shows stutter. True GPU
//!   render time needs backend instrumentation and belongs to Phase 4.
//!
//! Text layout, the bitmap font and the timing statistics are pure and covered by `cargo test`.

use std::{
    cell::RefCell,
    collections::VecDeque,
    time::{Duration, Instant},
};

use smithay::{
    backend::{allocator::Fourcc, renderer::element::memory::MemoryRenderBuffer},
    utils::{Rectangle, Transform},
};

/// Overlay state kept in `Kompact`.
pub struct Overlay {
    pub enabled: bool,
    /// The one image, rewritten in place by `show`.
    pub buffer: MemoryRenderBuffer,
    /// Fed by `render.rs` (which only has `&Kompact`) while enabled.
    pub frames: RefCell<FrameStats>,
    shown: Vec<String>,
}

impl Overlay {
    pub fn new() -> Self {
        let buffer = MemoryRenderBuffer::from_slice(
            &rasterize(&[]),
            Fourcc::Argb8888,
            (WIDTH as i32, HEIGHT as i32),
            1,
            Transform::Normal,
            None,
        );
        Self { enabled: false, buffer, frames: RefCell::default(), shown: Vec::new() }
    }

    /// Redraw the image with `lines` (skipped if nothing changed). Updates the existing buffer
    /// and marks it all damaged, so the renderer re-uploads into the same texture.
    pub fn show(&mut self, lines: Vec<String>) {
        if lines == self.shown {
            return;
        }
        let px = rasterize(&lines);
        let mut ctx = self.buffer.render();
        let res: Result<(), std::convert::Infallible> = ctx.draw(|mem| {
            mem.copy_from_slice(&px);
            Ok(vec![Rectangle::new((0, 0).into(), (WIDTH as i32, HEIGHT as i32).into())])
        });
        let _ = res;
        drop(ctx);
        self.shown = lines;
    }
}

/// Text grid: fixed, so the buffer never needs resizing.
pub const COLS: usize = 26;
pub const ROWS: usize = 7;
/// Font pixels are drawn as SCALE x SCALE blocks.
const SCALE: usize = 2;
const GLYPH_W: usize = 5;
const GLYPH_H: usize = 7;
const CELL_W: usize = (GLYPH_W + 1) * SCALE;
const CELL_H: usize = (GLYPH_H + 2) * SCALE;
const PAD: usize = 8;
pub const WIDTH: usize = COLS * CELL_W + 2 * PAD;
pub const HEIGHT: usize = ROWS * CELL_H + 2 * PAD - 2 * SCALE;

/// 5x7 glyphs, one byte per row, bit 4 = leftmost pixel. Upper-case only; lower-case input is
/// upper-cased, anything unknown is drawn as a blank.
fn glyph(c: char) -> [u8; 7] {
    match c.to_ascii_uppercase() {
        '0' => [0x0E, 0x11, 0x13, 0x15, 0x19, 0x11, 0x0E],
        '1' => [0x04, 0x0C, 0x04, 0x04, 0x04, 0x04, 0x0E],
        '2' => [0x0E, 0x11, 0x01, 0x02, 0x04, 0x08, 0x1F],
        '3' => [0x1F, 0x02, 0x04, 0x02, 0x01, 0x11, 0x0E],
        '4' => [0x02, 0x06, 0x0A, 0x12, 0x1F, 0x02, 0x02],
        '5' => [0x1F, 0x10, 0x1E, 0x01, 0x01, 0x11, 0x0E],
        '6' => [0x06, 0x08, 0x10, 0x1E, 0x11, 0x11, 0x0E],
        '7' => [0x1F, 0x01, 0x02, 0x04, 0x08, 0x08, 0x08],
        '8' => [0x0E, 0x11, 0x11, 0x0E, 0x11, 0x11, 0x0E],
        '9' => [0x0E, 0x11, 0x11, 0x0F, 0x01, 0x02, 0x0C],
        'A' => [0x0E, 0x11, 0x11, 0x1F, 0x11, 0x11, 0x11],
        'B' => [0x1E, 0x11, 0x11, 0x1E, 0x11, 0x11, 0x1E],
        'C' => [0x0E, 0x11, 0x10, 0x10, 0x10, 0x11, 0x0E],
        'D' => [0x1C, 0x12, 0x11, 0x11, 0x11, 0x12, 0x1C],
        'E' => [0x1F, 0x10, 0x10, 0x1E, 0x10, 0x10, 0x1F],
        'F' => [0x1F, 0x10, 0x10, 0x1E, 0x10, 0x10, 0x10],
        'G' => [0x0E, 0x11, 0x10, 0x17, 0x11, 0x11, 0x0F],
        'H' => [0x11, 0x11, 0x11, 0x1F, 0x11, 0x11, 0x11],
        'I' => [0x0E, 0x04, 0x04, 0x04, 0x04, 0x04, 0x0E],
        'J' => [0x07, 0x02, 0x02, 0x02, 0x02, 0x12, 0x0C],
        'K' => [0x11, 0x12, 0x14, 0x18, 0x14, 0x12, 0x11],
        'L' => [0x10, 0x10, 0x10, 0x10, 0x10, 0x10, 0x1F],
        'M' => [0x11, 0x1B, 0x15, 0x15, 0x11, 0x11, 0x11],
        'N' => [0x11, 0x11, 0x19, 0x15, 0x13, 0x11, 0x11],
        'O' => [0x0E, 0x11, 0x11, 0x11, 0x11, 0x11, 0x0E],
        'P' => [0x1E, 0x11, 0x11, 0x1E, 0x10, 0x10, 0x10],
        'Q' => [0x0E, 0x11, 0x11, 0x11, 0x15, 0x12, 0x0D],
        'R' => [0x1E, 0x11, 0x11, 0x1E, 0x14, 0x12, 0x11],
        'S' => [0x0F, 0x10, 0x10, 0x0E, 0x01, 0x01, 0x1E],
        'T' => [0x1F, 0x04, 0x04, 0x04, 0x04, 0x04, 0x04],
        'U' => [0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x0E],
        'V' => [0x11, 0x11, 0x11, 0x11, 0x11, 0x0A, 0x04],
        'W' => [0x11, 0x11, 0x11, 0x15, 0x15, 0x15, 0x0A],
        'X' => [0x11, 0x11, 0x0A, 0x04, 0x0A, 0x11, 0x11],
        'Y' => [0x11, 0x11, 0x11, 0x0A, 0x04, 0x04, 0x04],
        'Z' => [0x1F, 0x01, 0x02, 0x04, 0x08, 0x10, 0x1F],
        '.' => [0x00, 0x00, 0x00, 0x00, 0x00, 0x0C, 0x0C],
        ':' => [0x00, 0x0C, 0x0C, 0x00, 0x0C, 0x0C, 0x00],
        '/' => [0x01, 0x01, 0x02, 0x04, 0x08, 0x10, 0x10],
        '-' => [0x00, 0x00, 0x00, 0x1F, 0x00, 0x00, 0x00],
        '=' => [0x00, 0x00, 0x1F, 0x00, 0x1F, 0x00, 0x00],
        '%' => [0x18, 0x19, 0x02, 0x04, 0x08, 0x13, 0x03],
        '?' => [0x0E, 0x11, 0x01, 0x02, 0x04, 0x00, 0x04],
        _ => [0; 7],
    }
}

/// Premultiplied BGRA pixels (WIDTH x HEIGHT): translucent dark box, white text, one row per
/// line. Lines longer than COLS are cut, extra lines dropped.
pub fn rasterize(lines: &[String]) -> Vec<u8> {
    const BG: [u8; 4] = [20, 16, 16, 200]; // premultiplied dark, ~78% opaque
    const FG: [u8; 4] = [255, 255, 255, 255];
    let mut px = vec![0u8; WIDTH * HEIGHT * 4];
    for p in px.chunks_exact_mut(4) {
        p.copy_from_slice(&BG);
    }
    for (row, line) in lines.iter().take(ROWS).enumerate() {
        for (col, ch) in line.chars().take(COLS).enumerate() {
            let g = glyph(ch);
            for (gy, bits) in g.iter().enumerate() {
                for gx in 0..GLYPH_W {
                    if bits & (0x10 >> gx) == 0 {
                        continue;
                    }
                    for sy in 0..SCALE {
                        for sx in 0..SCALE {
                            let x = PAD + col * CELL_W + gx * SCALE + sx;
                            let y = PAD + row * CELL_H + gy * SCALE + sy;
                            let i = (y * WIDTH + x) * 4;
                            px[i..i + 4].copy_from_slice(&FG);
                        }
                    }
                }
            }
        }
    }
    px
}

/// Render-pass timing over a sliding one-second window, fed from `render.rs`.
#[derive(Default)]
pub struct FrameStats {
    /// (start of pass, time spent building its elements)
    passes: VecDeque<(Instant, Duration)>,
}

const WINDOW: Duration = Duration::from_secs(1);

#[derive(Debug, PartialEq)]
pub struct Timing {
    pub passes: usize,
    pub max_gap: Duration,
    pub avg_build: Duration,
    pub max_build: Duration,
}

impl FrameStats {
    pub fn record(&mut self, start: Instant, build: Duration) {
        self.passes.push_back((start, build));
        self.trim(start);
    }

    fn trim(&mut self, now: Instant) {
        // Keep one pass from before the window so the first gap inside it can be measured.
        while self.passes.len() > 2 && now.duration_since(self.passes[1].0) > WINDOW {
            self.passes.pop_front();
        }
    }

    /// Stats for the second ending at `now`. The gap from the last pass to `now` counts too, so a
    /// hang shows up even while no passes happen.
    pub fn timing(&mut self, now: Instant) -> Timing {
        self.trim(now);
        let in_window: Vec<&(Instant, Duration)> =
            self.passes.iter().filter(|(t, _)| now.duration_since(*t) <= WINDOW).collect();
        let mut max_gap = Duration::ZERO;
        let times: Vec<Instant> = self.passes.iter().map(|(t, _)| *t).collect();
        for w in times.windows(2) {
            if now.duration_since(w[1]) <= WINDOW {
                max_gap = max_gap.max(w[1].duration_since(w[0]));
            }
        }
        if let Some(last) = times.last() {
            max_gap = max_gap.max(now.duration_since(*last).min(WINDOW));
        }
        let builds: Vec<Duration> = in_window.iter().map(|(_, b)| *b).collect();
        let avg_build = if builds.is_empty() {
            Duration::ZERO
        } else {
            builds.iter().sum::<Duration>() / builds.len() as u32
        };
        Timing {
            passes: in_window.len(),
            max_gap,
            avg_build,
            max_build: builds.iter().copied().max().unwrap_or_default(),
        }
    }
}

/// Numbers shown on the overlay (gathered by the caller).
pub struct Snapshot {
    pub rss_kib: Option<u64>,
    pub fds: Option<crate::stats::FdCounts>,
    pub timing: Timing,
    pub visible: usize,
    pub parked: usize,
    pub workspace: usize,
    pub surfaces_with_buffer: usize,
    pub uptime: Duration,
}

pub fn format_lines(s: &Snapshot) -> Vec<String> {
    let ms = |d: Duration| d.as_secs_f64() * 1000.0;
    let rss = s.rss_kib.map_or("?".into(), |k| k.to_string());
    let (fds, dmabuf) = s.fds.map_or(("?".into(), "?".into()), |f| (f.total.to_string(), f.dmabuf.to_string()));
    let up = s.uptime.as_secs();
    vec![
        format!("KOMPACT DEBUG  UP {}:{:02}:{:02}", up / 3600, up / 60 % 60, up % 60),
        format!("RSS {rss} KIB"),
        format!("FDS {fds} DMABUF {dmabuf}"),
        format!("PASSES {}/S GAP {:.0}MS", s.timing.passes, ms(s.timing.max_gap)),
        format!("BUILD {:.2} MAX {:.2}MS", ms(s.timing.avg_build), ms(s.timing.max_build)),
        format!("WIN {} PARKED {} WS {}", s.visible, s.parked, s.workspace + 1),
        format!("BUFFERS {}", s.surfaces_with_buffer),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn alpha(px: &[u8], x: usize, y: usize) -> u8 {
        px[(y * WIDTH + x) * 4 + 3]
    }
    fn white(px: &[u8], x: usize, y: usize) -> bool {
        px[(y * WIDTH + x) * 4..][..4] == [255, 255, 255, 255]
    }

    #[test]
    fn buffer_size_is_fixed() {
        assert_eq!(rasterize(&[]).len(), WIDTH * HEIGHT * 4);
        let long = vec!["X".repeat(100); 20];
        assert_eq!(rasterize(&long).len(), WIDTH * HEIGHT * 4, "long text is clipped, not grown");
    }

    #[test]
    fn background_is_translucent_and_text_is_white() {
        let px = rasterize(&["1".to_string()]);
        assert_eq!(alpha(&px, 0, 0), 200);
        // '1' has its stem in glyph column 2 on every row from 1 to 6.
        let x = PAD + 2 * SCALE;
        let y = PAD + 3 * SCALE;
        assert!(white(&px, x, y));
        assert!(!white(&px, PAD + CELL_W + 2 * SCALE, y), "second cell is empty");
    }

    #[test]
    fn every_overlay_character_has_a_glyph() {
        let lines = format_lines(&Snapshot {
            rss_kib: Some(95436),
            fds: None,
            timing: Timing { passes: 60, max_gap: Duration::ZERO, avg_build: Duration::ZERO, max_build: Duration::ZERO },
            visible: 3,
            parked: 6,
            workspace: 0,
            surfaces_with_buffer: 12,
            uptime: Duration::from_secs(3723),
        });
        for line in &lines {
            assert!(line.len() <= COLS, "{line:?} is wider than the overlay");
            for c in line.chars().filter(|c| *c != ' ') {
                assert_ne!(glyph(c), [0; 7], "no glyph for {c:?} in {line:?}");
            }
        }
        assert_eq!(lines.len(), ROWS);
        assert_eq!(lines[0], "KOMPACT DEBUG  UP 1:02:03");
    }

    #[test]
    fn steady_60hz_passes() {
        let t0 = Instant::now();
        let mut f = FrameStats::default();
        let frame = Duration::from_micros(16_667);
        for i in 0..180 {
            f.record(t0 + frame * i, Duration::from_micros(300));
        }
        let t = f.timing(t0 + frame * 179);
        assert!((59..=61).contains(&t.passes), "{t:?}");
        assert!(t.max_gap >= frame - Duration::from_micros(5) && t.max_gap <= frame + Duration::from_micros(5));
        assert_eq!(t.avg_build, Duration::from_micros(300));
    }

    #[test]
    fn a_stall_shows_as_a_large_gap() {
        let t0 = Instant::now();
        let mut f = FrameStats::default();
        let frame = Duration::from_millis(16);
        for i in 0..30 {
            f.record(t0 + frame * i, Duration::from_micros(200));
        }
        let after = t0 + frame * 29 + Duration::from_millis(250);
        f.record(after, Duration::from_millis(3));
        let t = f.timing(after);
        assert_eq!(t.max_gap, Duration::from_millis(250));
        assert_eq!(t.max_build, Duration::from_millis(3));
    }

    #[test]
    fn idle_reports_time_since_last_pass_capped_at_a_second() {
        let t0 = Instant::now();
        let mut f = FrameStats::default();
        f.record(t0, Duration::from_micros(100));
        let t = f.timing(t0 + Duration::from_secs(5));
        assert_eq!(t.passes, 0);
        assert_eq!(t.max_gap, WINDOW);
    }

    #[test]
    fn memory_stays_bounded() {
        let t0 = Instant::now();
        let mut f = FrameStats::default();
        for i in 0..100_000u32 {
            f.record(t0 + Duration::from_millis(i as u64), Duration::ZERO);
        }
        assert!(f.passes.len() <= 1_002, "{} entries kept", f.passes.len());
    }
}
