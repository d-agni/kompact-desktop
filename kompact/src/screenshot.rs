//! 9a: screenshots to PNG files, taken by kompact itself.
//!
//! **Render path, off screen only.** A key (`keymap.rs`) queues a request; after the backend
//! has drawn and queued this turn's frames, `capture_pending` draws each wanted output once
//! more, without the cursor (as on macOS), into an off-screen GLES texture with the same
//! renderer, reads it back (`glReadPixels`) and unbinds: the method window thumbnails use
//! (6m-d3), at the output's full size. Scanout buffers are never read. The PNG is encoded and
//! written on its own thread, so only the readback holds up the loop.
//!
//! Files go to `KOMPACT_SCREENSHOT_DIR`, else `~/Pictures/Screenshots`, named like macOS:
//! `Screenshot 2026-10-11 at 14.03.22.png`, with the monitor's name added when there are
//! several. Refused while the screen is locked. `KOMPACT_SCREENSHOTS=off` turns the keys off.
//!
//! 9c: Shift+Print lets you drag out a region in the shell, which sends it back (`shot x y w h`);
//! the shot waits until the shell's selection layer is gone (at most `SELECTION_WAIT`), so it
//! isn't in the picture. Each written file is reported to the shell (`screenshot` message),
//! which shows a "Screenshot saved" banner.
//!
//! The keys are in `keymap.rs` (10a), with the other shortcuts.

use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use smithay::{
    backend::{
        allocator::Fourcc,
        renderer::{
            damage::OutputDamageTracker,
            gles::{GlesRenderer, GlesTexture},
            Bind, ExportMem, Offscreen, Unbind,
        },
    },
    output::Output,
    utils::{Physical, Rectangle, Size, Transform},
};

use crate::state::Kompact;

/// A queued screenshot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Shot {
    AllOutputs,
    PointerOutput,
    /// 9c: part of one output, output-local and already clipped to it.
    Region { output: String, rect: Rectangle<i32, Physical> },
}

/// How long a region shot waits for the shell's selection layer to go away, at most.
pub const SELECTION_WAIT: Duration = Duration::from_secs(1);
/// The shell's overlay layer surface (namespace), which shows the selection.
pub const OVERLAY_NAMESPACE: &str = "kompact-overlay";

pub fn enabled_from_env(value: Option<&str>) -> bool {
    !matches!(value.map(|v| v.trim().to_ascii_lowercase()).as_deref(), Some("off" | "0" | "no" | "false"))
}

/// Where screenshots go: `KOMPACT_SCREENSHOT_DIR` if set, else `<home>/Pictures/Screenshots`.
#[cfg(test)]
pub fn shots_dir(env_dir: Option<&str>, home: Option<&Path>) -> Option<PathBuf> {
    shots_dir_with(env_dir, None, home)
}

/// 10d-4: the same with the folder chosen in Settings → Screenshots between the two.
pub fn shots_dir_with(env_dir: Option<&str>, setting: Option<&Path>, home: Option<&Path>) -> Option<PathBuf> {
    match env_dir.map(str::trim).filter(|d| !d.is_empty()) {
        Some(d) => Some(PathBuf::from(d)),
        None => setting.map(Path::to_path_buf).or_else(|| home.map(|h| h.join("Pictures").join("Screenshots"))),
    }
}

/// 10d-4: where they go now, for this process.
pub fn current_dir(setting: Option<&Path>) -> Option<PathBuf> {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    shots_dir_with(std::env::var("KOMPACT_SCREENSHOT_DIR").ok().as_deref(), setting, home.as_deref())
}

/// Local time, enough for a file name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Stamp {
    pub year: i32,
    pub month: u32,
    pub day: u32,
    pub hour: u32,
    pub minute: u32,
    pub second: u32,
}

impl Stamp {
    pub fn now() -> Option<Stamp> {
        // SAFETY: localtime_r only writes the `tm` we hand it.
        unsafe {
            let t = libc::time(std::ptr::null_mut());
            let mut tm: libc::tm = std::mem::zeroed();
            if libc::localtime_r(&t, &mut tm).is_null() {
                return None;
            }
            Some(Stamp {
                year: tm.tm_year + 1900,
                month: (tm.tm_mon + 1) as u32,
                day: tm.tm_mday as u32,
                hour: tm.tm_hour as u32,
                minute: tm.tm_min as u32,
                second: tm.tm_sec as u32,
            })
        }
    }
}

/// `Screenshot 2026-10-11 at 14.03.22.png`, or `… 14.03.22 (VGA-1).png` with a monitor name;
/// `n` > 1 adds ` n` before the extension, for a name already taken.
pub fn file_name(at: Stamp, output: Option<&str>, n: u32) -> String {
    let mut name = format!(
        "Screenshot {:04}-{:02}-{:02} at {:02}.{:02}.{:02}",
        at.year, at.month, at.day, at.hour, at.minute, at.second
    );
    if let Some(o) = output {
        name.push_str(&format!(" ({})", o.replace('/', "_")));
    }
    if n > 1 {
        name.push_str(&format!(" {n}"));
    }
    name.push_str(".png");
    name
}

/// `rect` of an RGBA image `src_w` px wide (top row first) as RGB, dropping alpha.
pub fn crop_rgb(rgba: &[u8], src_w: i32, rect: Rectangle<i32, Physical>) -> Vec<u8> {
    let (x, y, w, h) = (rect.loc.x as usize, rect.loc.y as usize, rect.size.w as usize, rect.size.h as usize);
    let stride = src_w as usize * 4;
    (y..y + h).flat_map(|row| rgba_to_rgb(&rgba[row * stride + x * 4..][..w * 4])).collect()
}

/// RGBA (premultiplied; the screen is opaque) to RGB, dropping alpha.
pub fn rgba_to_rgb(rgba: &[u8]) -> Vec<u8> {
    rgba.chunks_exact(4).flat_map(|p| [p[0], p[1], p[2]]).collect()
}

/// A file written (9c: the shell shows a banner for it).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Saved {
    pub path: PathBuf,
    pub width: i32,
    pub height: i32,
}

/// Queued screenshot requests (at most one of each until it's taken), with when they came.
#[derive(Debug)]
pub struct Shots {
    pub enabled: bool,
    pending: Vec<(Shot, Instant)>,
    /// 9b: screencopy copies waiting for this turn's frames (`screencopy.rs`).
    pub copies: Vec<crate::screencopy::Copy>,
    /// 9e-a: screencopy streams (`copy_with_damage` frame after frame), their damage trackers.
    pub streams: crate::screencopy::Streams,
    /// 9c: the encoding threads report written files here.
    saved_tx: mpsc::Sender<Saved>,
    saved_rx: mpsc::Receiver<Saved>,
}

impl Default for Shots {
    fn default() -> Self {
        Shots::new(enabled_from_env(std::env::var("KOMPACT_SCREENSHOTS").ok().as_deref()))
    }
}

impl Shots {
    pub fn new(enabled: bool) -> Shots {
        let (saved_tx, saved_rx) = mpsc::channel();
        Shots { enabled, pending: Vec::new(), copies: Vec::new(), streams: Default::default(), saved_tx, saved_rx }
    }

    pub fn request(&mut self, shot: Shot) {
        self.request_at(shot, Instant::now());
    }

    fn request_at(&mut self, shot: Shot, now: Instant) {
        if !self.pending.iter().any(|(s, _)| *s == shot) {
            self.pending.push((shot, now));
        }
    }

    /// The oldest shot that may be taken now. A region waits while the shell's selection is
    /// still up (`selection_up`), for at most `SELECTION_WAIT`; it would be in the picture.
    pub fn take(&mut self, selection_up: bool, now: Instant) -> Option<Shot> {
        let i = self.pending.iter().position(|(shot, asked)| {
            !matches!(shot, Shot::Region { .. }) || !selection_up || now.duration_since(*asked) >= SELECTION_WAIT
        })?;
        Some(self.pending.remove(i).0)
    }

    /// Files written since the last call.
    pub fn take_saved(&self) -> Vec<Saved> {
        self.saved_rx.try_iter().collect()
    }

    /// Whether the backend should call `capture_pending` this turn. A stream counts until it
    /// has been idle a while (9e-a), so its end is noticed and its tracker dropped.
    pub fn is_pending(&self) -> bool {
        !self.pending.is_empty() || !self.copies.is_empty() || self.streams.is_active()
    }
}

/// Opaque black behind everything (where nothing is drawn, e.g. no wallpaper yet).
const CLEAR: [f32; 4] = [0.0, 0.0, 0.0, 1.0];

/// Draw `output` (with or without the cursor) off screen at its mode size and read it back as
/// RGBA, top row first. Always leaves the renderer unbound.
pub fn capture(renderer: &mut GlesRenderer, state: &Kompact, output: &Output, cursor: bool) -> Result<(i32, i32, Vec<u8>), String> {
    capture_with(renderer, state, output, cursor, Fourcc::Abgr8888, |w, h, bytes| (w, h, bytes.to_vec())).map(|(c, _)| c)
}

/// 9e-a: how long a capture took. `draw` is issuing the drawing; `read_back` includes waiting
/// for the GPU to finish it.
#[derive(Debug, Clone, Copy, Default)]
pub struct CaptureTime {
    pub draw: Duration,
    pub read_back: Duration,
}

/// `capture`, read back as `fourcc` (`Abgr8888`: R, G, B, A bytes; 9e-a2 `Argb8888`: B, G, R,
/// A, via `GL_EXT_read_format_bgra`), the mapped pixels handed to `with` (width, height,
/// pixels) without copying them first. Also used by 9b (`screencopy.rs`).
pub fn capture_with<T>(
    renderer: &mut GlesRenderer,
    state: &Kompact,
    output: &Output,
    cursor: bool,
    fourcc: Fourcc,
    with: impl FnOnce(i32, i32, &[u8]) -> T,
) -> Result<(T, CaptureTime), String> {
    let mode = output.current_mode().ok_or("the output has no mode")?;
    let (w, h) = (mode.size.w, mode.size.h);
    let texture: GlesTexture = Offscreen::<GlesTexture>::create_buffer(renderer, Fourcc::Abgr8888, Size::from((w, h)))
        .map_err(|e| format!("off-screen texture: {e:?}"))?;
    renderer.bind(texture).map_err(|e| format!("bind: {e:?}"))?;
    let result = (|| {
        let elements = crate::render::build_elements_with(renderer, state, output, cursor);
        let mut tracker = OutputDamageTracker::new((w, h), 1.0, Transform::Normal);
        let started = Instant::now();
        tracker.render_output(renderer, 0, &elements, CLEAR).map_err(|e| format!("draw: {e:?}"))?;
        let drawn = Instant::now();
        let mapping = renderer
            .copy_framebuffer(Rectangle::new((0, 0).into(), (w, h).into()), fourcc)
            .map_err(|e| format!("read back as {fourcc:?}: {e:?}"))?;
        let bytes = renderer.map_texture(&mapping).map_err(|e| format!("map: {e:?}"))?;
        let time = CaptureTime { draw: drawn - started, read_back: drawn.elapsed() };
        Ok((with(w, h, bytes), time))
    })();
    if let Err(e) = renderer.unbind() {
        tracing::warn!("screenshot: unbind failed: {e:?}");
    }
    result
}

fn encode(path: &Path, w: i32, h: i32, rgb: &[u8]) -> Result<(), String> {
    let tmp = path.with_extension("png.part");
    let file = std::fs::File::create(&tmp).map_err(|e| format!("{}: {e}", tmp.display()))?;
    let mut encoder = png::Encoder::new(std::io::BufWriter::new(file), w as u32, h as u32);
    encoder.set_color(png::ColorType::Rgb);
    encoder.set_depth(png::BitDepth::Eight);
    encoder.set_compression(png::Compression::Fast);
    let mut writer = encoder.write_header().map_err(|e| format!("png: {e}"))?;
    writer.write_image_data(rgb).map_err(|e| format!("png: {e}"))?;
    writer.finish().map_err(|e| format!("png: {e}"))?;
    std::fs::rename(&tmp, path).map_err(|e| format!("{}: {e}", path.display()))
}

/// A free path for this shot in `dir`.
fn free_path(dir: &Path, at: Stamp, output: Option<&str>) -> PathBuf {
    (1..)
        .map(|n| dir.join(file_name(at, output, n)))
        .find(|p| !p.exists())
        .expect("some name is free")
}

/// From the backend's loop, after its frames are queued: take the queued screenshots and make
/// the queued screencopy copies (9b), if any.
pub fn capture_pending(renderer: &mut GlesRenderer, state: &mut Kompact) {
    crate::screencopy::run_pending(renderer, state);
    let selection_up = state
        .outputs
        .iter()
        .any(|o| smithay::desktop::layer_map_for_output(o).layers().any(|l| l.namespace() == OVERLAY_NAMESPACE));
    while let Some(shot) = state.shots.take(selection_up, Instant::now()) {
        if state.lock.is_locked() {
            tracing::info!("screenshot: refused, the screen is locked");
            continue;
        }
        if selection_up && matches!(shot, Shot::Region { .. }) {
            tracing::warn!("screenshot: the shell's selection is still up after {} ms; taking the region anyway", SELECTION_WAIT.as_millis());
        }
        let (outputs, rect): (Vec<Output>, _) = match &shot {
            Shot::AllOutputs => (state.outputs.clone(), None),
            Shot::PointerOutput => (state.active_output().cloned().into_iter().collect(), None),
            Shot::Region { output, rect } => (state.outputs.iter().filter(|o| o.name() == *output).cloned().collect(), Some(*rect)),
        };
        if outputs.is_empty() {
            tracing::info!("screenshot: {shot:?}: no such output now");
            continue;
        }
        let Some(dir) = current_dir(state.prefs.shots_dir.as_deref()) else {
            tracing::warn!("screenshot: no HOME and no KOMPACT_SCREENSHOT_DIR, nowhere to save");
            continue;
        };
        if let Err(e) = std::fs::create_dir_all(&dir) {
            tracing::warn!("screenshot: {}: {e}", dir.display());
            continue;
        }
        let Some(at) = Stamp::now() else {
            tracing::warn!("screenshot: no local time");
            continue;
        };
        // A monitor's name in the file name only when there are several and it's a whole one.
        let named = state.outputs.len() > 1 && rect.is_none();
        for output in outputs {
            let name = output.name();
            let started = Instant::now();
            // 10d-4: with the pointer if Settings → Screenshots says so (off, as on macOS).
            let captured = capture(renderer, state, &output, state.prefs.shot_pointer);
            let took = started.elapsed();
            let (src_w, src_h, rgba) = match captured {
                Ok(c) => c,
                Err(e) => {
                    tracing::warn!("screenshot: {name}: {e}");
                    continue;
                }
            };
            tracing::info!("screenshot: {name} {src_w}x{src_h} read back in {:.1} ms ({shot:?})", took.as_secs_f64() * 1000.0);
            let full = Rectangle::new((0, 0).into(), (src_w, src_h).into());
            let Some(rect) = rect.map_or(Some(full), |r| r.intersection(full)) else {
                tracing::info!("screenshot: the region is off {name} now");
                continue;
            };
            let (w, h) = (rect.size.w, rect.size.h);
            let path = free_path(&dir, at, named.then_some(name.as_str()));
            let saved_tx = state.shots.saved_tx.clone();
            std::thread::spawn(move || {
                let started = Instant::now();
                let rgb = if rect == full { rgba_to_rgb(&rgba) } else { crop_rgb(&rgba, src_w, rect) };
                match encode(&path, w, h, &rgb) {
                    Ok(()) => {
                        let kb = std::fs::metadata(&path).map(|m| m.len() / 1024).unwrap_or(0);
                        tracing::info!(
                            "screenshot: saved {} ({w}x{h}, {kb} KB, encoded in {:.0} ms)",
                            path.display(),
                            started.elapsed().as_secs_f64() * 1000.0
                        );
                        let _ = saved_tx.send(Saved { path, width: w, height: h });
                    }
                    Err(e) => tracing::warn!("screenshot: {name}: {e}"),
                }
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn env_switch_and_dir() {
        assert!(enabled_from_env(None));
        assert!(enabled_from_env(Some("on")));
        assert!(!enabled_from_env(Some(" OFF ")));
        let home = Path::new("/home/u");
        assert_eq!(shots_dir(None, Some(home)), Some(PathBuf::from("/home/u/Pictures/Screenshots")));
        assert_eq!(shots_dir(Some(" "), Some(home)), Some(PathBuf::from("/home/u/Pictures/Screenshots")));
        // 10d-4: the setting between the variable and the default.
        let set = Path::new("/home/u/Desktop");
        assert_eq!(shots_dir_with(None, Some(set), Some(home)), Some(set.to_path_buf()));
        assert_eq!(shots_dir_with(Some("/tmp/s"), Some(set), Some(home)), Some(PathBuf::from("/tmp/s")));
        assert_eq!(shots_dir(Some("/tmp/s"), Some(home)), Some(PathBuf::from("/tmp/s")));
        assert_eq!(shots_dir(None, None), None);
    }

    #[test]
    fn names() {
        let at = Stamp { year: 2026, month: 10, day: 1, hour: 9, minute: 3, second: 7 };
        assert_eq!(file_name(at, None, 1), "Screenshot 2026-10-01 at 09.03.07.png");
        assert_eq!(file_name(at, Some("VGA-1"), 1), "Screenshot 2026-10-01 at 09.03.07 (VGA-1).png");
        assert_eq!(file_name(at, None, 2), "Screenshot 2026-10-01 at 09.03.07 2.png");
        assert!(Stamp::now().is_some());
    }

    #[test]
    fn free_names() {
        let dir = std::env::temp_dir().join(format!("kompact-shots-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let at = Stamp { year: 2026, month: 10, day: 1, hour: 9, minute: 3, second: 7 };
        let first = free_path(&dir, at, None);
        std::fs::write(&first, b"x").unwrap();
        assert_eq!(free_path(&dir, at, None), dir.join("Screenshot 2026-10-01 at 09.03.07 2.png"));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn pixels_and_png() {
        assert_eq!(rgba_to_rgb(&[1, 2, 3, 255, 4, 5, 6, 255]), vec![1, 2, 3, 4, 5, 6]);
        let dir = std::env::temp_dir().join(format!("kompact-png-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("t.png");
        let rgb: Vec<u8> = (0..2 * 3 * 3).map(|i| i as u8).collect();
        encode(&path, 2, 3, &rgb).unwrap();
        let decoder = png::Decoder::new(std::fs::File::open(&path).unwrap());
        let mut reader = decoder.read_info().unwrap();
        let mut buf = vec![0; reader.output_buffer_size()];
        let info = reader.next_frame(&mut buf).unwrap();
        assert_eq!((info.width, info.height), (2, 3));
        assert_eq!(&buf[..info.buffer_size()], &rgb[..]);
        assert!(!dir.join("t.png.part").exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn queue() {
        let t0 = Instant::now();
        let region = Shot::Region { output: "VGA-1".into(), rect: Rectangle::new((10, 20).into(), (30, 40).into()) };
        let mut s = Shots::new(true);
        assert!(!s.is_pending());
        s.request_at(Shot::AllOutputs, t0);
        s.request_at(Shot::AllOutputs, t0);
        s.request_at(region.clone(), t0);
        s.request_at(Shot::PointerOutput, t0);
        assert!(s.is_pending());
        // While the selection is up, the region waits and the others go first.
        assert_eq!(s.take(true, t0), Some(Shot::AllOutputs));
        assert_eq!(s.take(true, t0), Some(Shot::PointerOutput));
        assert_eq!(s.take(true, t0 + Duration::from_millis(500)), None);
        assert!(s.is_pending());
        // Gone: taken. (Or after SELECTION_WAIT anyway.)
        assert_eq!(s.take(false, t0 + Duration::from_millis(600)), Some(region.clone()));
        s.request_at(region.clone(), t0);
        assert_eq!(s.take(true, t0 + SELECTION_WAIT), Some(region));
        assert_eq!(s.take(false, t0), None);
        assert!(!s.is_pending());
        // Written files come back from the encoding threads.
        let tx = s.saved_tx.clone();
        std::thread::spawn(move || tx.send(Saved { path: "/x.png".into(), width: 1, height: 2 }).unwrap()).join().unwrap();
        assert_eq!(s.take_saved(), vec![Saved { path: "/x.png".into(), width: 1, height: 2 }]);
        assert!(s.take_saved().is_empty());
    }

    #[test]
    fn crop() {
        // 3x2 RGBA, pixel (x, y) = [10x+y, 0, 0, 255].
        let mut src = Vec::new();
        for y in 0..2u8 {
            for x in 0..3u8 {
                src.extend([10 * x + y, 0, 0, 255]);
            }
        }
        let rect = Rectangle::<i32, Physical>::new((1, 1).into(), (2, 1).into());
        assert_eq!(crop_rgb(&src, 3, rect), vec![11, 0, 0, 21, 0, 0]);
    }
}
