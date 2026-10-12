//! 6m-d3: window thumbnails for the dock's previews.
//!
//! **Render path.** The shell asks for a window over the private channel (`thumb <id>`); after
//! the backend has drawn and queued its frames, `capture_next` draws that one window, scaled
//! down, into an off-screen GLES texture with the same renderer, reads it back
//! (`glReadPixels`) and unbinds. The next frame binds its own target as always, so scanout is
//! untouched; the cost is one small draw and readback, only on request (the shell asks while a
//! preview is open), at most one per loop turn.
//!
//! The pixels (RGBA, premultiplied, top row first) go to a file in
//! `$XDG_RUNTIME_DIR/kompact-thumbs-<WAYLAND_DISPLAY>/<id>.rgba`, written whole and renamed into
//! place, and the shell is told `{"type":"thumb","id":…,"width":…,"height":…,"path":"…"}`.
//!
//! On by default since 6p-c (confirmed on the GT 710 in 6m-d3…6e). `KOMPACT_THUMBNAILS=off`
//! turns it off: requests are then ignored and the dock and overview keep their icon cards.

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Instant;

use smithay::{
    backend::{
        allocator::Fourcc,
        renderer::{
            damage::OutputDamageTracker,
            element::{surface::WaylandSurfaceRenderElement, AsRenderElements},
            gles::{GlesRenderer, GlesTexture},
            Bind, ExportMem, Offscreen, Unbind,
        },
    },
    desktop::Window,
    utils::{Physical, Point, Rectangle, Scale, Size, Transform},
};

use crate::state::Kompact;

/// The largest thumbnail the dock shows, in pixels; we render at up to twice this, for the
/// shell to scale down smoothly (GL sampling alone would alias text at a fifth of the size).
pub const BOX: (i32, i32) = (352, 200);

pub fn enabled_from_env(value: Option<&str>) -> bool {
    !matches!(value.map(|v| v.trim().to_ascii_lowercase()).as_deref(), Some("off" | "0" | "no" | "false"))
}

/// The scale to draw a `w` x `h` window at: twice what fits it in `BOX`, never above 1.
pub fn render_scale(w: i32, h: i32) -> Option<f64> {
    if w <= 0 || h <= 0 {
        return None;
    }
    let s = (2.0 * BOX.0 as f64 / w as f64).min(2.0 * BOX.1 as f64 / h as f64);
    Some(s.min(1.0))
}

pub fn scaled_size(w: i32, h: i32, s: f64) -> (i32, i32) {
    (((w as f64 * s).round() as i32).max(1), ((h as f64 * s).round() as i32).max(1))
}

pub fn encode_thumb(id: u64, width: i32, height: i32, path: &Path) -> String {
    format!(
        "{{\"type\":\"thumb\",\"id\":{id},\"width\":{width},\"height\":{height},\"path\":{}}}\n",
        crate::ipc::json_string(&path.to_string_lossy())
    )
}

pub fn thumbs_dir(runtime_dir: &Path, display: &str) -> PathBuf {
    runtime_dir.join(format!("kompact-thumbs-{}", display.replace('/', "_")))
}

/// Requested windows, oldest first, each once.
pub struct Thumbs {
    pub enabled: bool,
    pending: Vec<u64>,
    said_off: bool,
}

impl Default for Thumbs {
    fn default() -> Self {
        let enabled = enabled_from_env(std::env::var("KOMPACT_THUMBNAILS").ok().as_deref());
        Thumbs { enabled, pending: Vec::new(), said_off: false }
    }
}

impl Thumbs {
    pub fn request(&mut self, id: u64) {
        if !self.enabled {
            if !self.said_off {
                tracing::info!("thumbnails: off (KOMPACT_THUMBNAILS=off); ignoring the shell's requests");
                self.said_off = true;
            }
            return;
        }
        if !self.pending.contains(&id) {
            self.pending.push(id);
        }
    }

    pub fn next(&mut self) -> Option<u64> {
        (!self.pending.is_empty()).then(|| self.pending.remove(0))
    }

    pub fn is_idle(&self) -> bool {
        self.pending.is_empty()
    }
}

pub struct Captured {
    pub width: i32,
    pub height: i32,
    /// RGBA, premultiplied alpha, top row first, no row padding.
    pub rgba: Vec<u8>,
}

/// Draw `window` (its geometry, without client-side shadows; popups included) into an
/// off-screen texture at a reduced scale and read it back. Always leaves the renderer unbound.
pub fn capture(renderer: &mut GlesRenderer, window: &Window) -> Result<Captured, String> {
    let geo = window.geometry();
    let s = render_scale(geo.size.w, geo.size.h).ok_or("the window has no size yet")?;
    let (w, h) = scaled_size(geo.size.w, geo.size.h, s);
    let texture: GlesTexture =
        Offscreen::<GlesTexture>::create_buffer(renderer, Fourcc::Abgr8888, Size::from((w, h)))
            .map_err(|e| format!("off-screen texture: {e:?}"))?;
    renderer.bind(texture).map_err(|e| format!("bind: {e:?}"))?;
    let result = (|| {
        let scale = Scale::from(s);
        // The window's geometry (not its shadow) lands at 0,0.
        let loc: Point<i32, Physical> = (Point::<f64, Physical>::default() - geo.loc.to_f64().to_physical(scale)).to_i32_round();
        let elements: Vec<WaylandSurfaceRenderElement<GlesRenderer>> = window.render_elements(renderer, loc, scale, 1.0);
        let size: Size<i32, Physical> = (w, h).into();
        let mut tracker = OutputDamageTracker::new(size, s, Transform::Normal);
        tracker
            .render_output(renderer, 0, &elements, [0.0, 0.0, 0.0, 0.0])
            .map_err(|e| format!("draw: {e:?}"))?;
        let mapping = renderer
            .copy_framebuffer(Rectangle::new((0, 0).into(), (w, h).into()), Fourcc::Abgr8888)
            .map_err(|e| format!("read back: {e:?}"))?;
        let bytes = renderer.map_texture(&mapping).map_err(|e| format!("map: {e:?}"))?;
        Ok(Captured { width: w, height: h, rgba: bytes.to_vec() })
    })();
    if let Err(e) = renderer.unbind() {
        tracing::warn!("thumbnails: unbind failed: {e:?}");
    }
    result
}

static DIR: Mutex<Option<PathBuf>> = Mutex::new(None);

/// Remove the thumbnails directory (called from `main` after the backend returns).
pub fn cleanup() {
    if let Some(dir) = DIR.lock().ok().and_then(|mut d| d.take()) {
        let _ = std::fs::remove_dir_all(dir);
    }
}

fn write(id: u64, c: &Captured) -> Result<PathBuf, String> {
    let (Some(runtime), Ok(display)) = (std::env::var_os("XDG_RUNTIME_DIR"), std::env::var("WAYLAND_DISPLAY")) else {
        return Err("no XDG_RUNTIME_DIR or WAYLAND_DISPLAY".into());
    };
    let dir = thumbs_dir(Path::new(&runtime), &display);
    if !dir.is_dir() {
        use std::os::unix::fs::DirBuilderExt;
        std::fs::DirBuilder::new().mode(0o700).create(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        if let Ok(mut d) = DIR.lock() {
            *d = Some(dir.clone());
        }
    }
    let path = dir.join(format!("{id}.rgba"));
    let tmp = dir.join(format!("{id}.rgba.tmp"));
    std::fs::write(&tmp, &c.rgba).map_err(|e| format!("{}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, &path).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(path)
}

/// From the backend's loop, after its frames are queued: capture one requested window, if any.
pub fn capture_next(renderer: &mut GlesRenderer, state: &mut Kompact) {
    let Some(id) = state.thumbs.next() else { return };
    let Some(window) = state.window_by_id(id) else {
        tracing::info!("thumbnails: no window {id}");
        return;
    };
    let started = Instant::now();
    let captured = capture(renderer, &window);
    let took = started.elapsed();
    match captured.and_then(|c| write(id, &c).map(|p| (c, p))) {
        Ok((c, path)) => {
            tracing::info!(
                "thumbnail of window {id} ({:?}): {}x{} in {:.1} ms",
                crate::shell::app_id(&window),
                c.width,
                c.height,
                took.as_secs_f64() * 1000.0
            );
            state.ipc.broadcast(&encode_thumb(id, c.width, c.height, &path));
        }
        Err(e) => tracing::warn!("thumbnails: window {id}: {e}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scale_fits_twice_the_box() {
        assert_eq!(render_scale(1408, 800), Some(0.5));
        let s = render_scale(1440, 872).unwrap();
        assert_eq!(scaled_size(1440, 872, s), (661, 400));
        assert_eq!(render_scale(300, 150), Some(1.0), "small windows aren't enlarged");
        assert_eq!(render_scale(0, 100), None);
        assert_eq!(scaled_size(1, 1, 0.01), (1, 1));
    }

    #[test]
    fn env_switch() {
        assert!(enabled_from_env(Some("on")));
        assert!(enabled_from_env(Some(" ON ")));
        assert!(!enabled_from_env(Some("off")));
        assert!(!enabled_from_env(Some(" No ")));
        assert!(enabled_from_env(None));
        assert!(enabled_from_env(Some("")));
    }

    #[test]
    fn queue_keeps_order_without_repeats() {
        let mut t = Thumbs { enabled: true, pending: Vec::new(), said_off: false };
        t.request(3);
        t.request(1);
        t.request(3);
        assert_eq!((t.next(), t.next(), t.next()), (Some(3), Some(1), None));
        let mut off = Thumbs { enabled: false, pending: Vec::new(), said_off: false };
        off.request(3);
        assert!(off.is_idle());
    }

    #[test]
    fn message() {
        assert_eq!(
            encode_thumb(7, 10, 5, &thumbs_dir(Path::new("/run/user/1000"), "wayland-1").join("7.rgba")),
            "{\"type\":\"thumb\",\"id\":7,\"width\":10,\"height\":5,\"path\":\"/run/user/1000/kompact-thumbs-wayland-1/7.rgba\"}\n"
        );
    }
}
