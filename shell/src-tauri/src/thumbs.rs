//! 6m-d3: window thumbnails for the dock's previews, from kompact (`kompact/src/thumbnail.rs`).
//!
//! kompact writes a window's pixels (RGBA, premultiplied, top row first) to a file in the
//! runtime dir and sends `{"type":"thumb","id":…,"width":…,"height":…,"path":…}`. We read it,
//! undo the premultiplication, scale it into `BOX` with gdk-pixbuf (smoother than GL's
//! sampling), keep it as PNG and serve it as `kompact-thumb://localhost/<id>`; the dock hears of
//! each new one through the `kompact-thumb` event (`{id, version}`, the version for the URL so
//! WebKit doesn't show a cached old one).

use std::collections::HashMap;
use std::sync::Mutex;

use serde::Serialize;
use serde_json::Value;
use tracing::{info, warn};

pub const THUMB_SCHEME: &str = "kompact-thumb";

/// The card's picture at twice its CSS size (`.card .thumb` in dock.css), for sharpness.
pub const BOX: (i32, i32) = (352, 200);

/// RGBA with premultiplied alpha → straight alpha, in place (what gdk-pixbuf and PNG expect).
pub fn unpremultiply(rgba: &mut [u8]) {
    for px in rgba.chunks_exact_mut(4) {
        let a = px[3] as u32;
        if a != 0 && a != 255 {
            for c in &mut px[..3] {
                *c = ((*c as u32 * 255 + a / 2) / a).min(255) as u8;
            }
        }
    }
}

/// `w` x `h` scaled to fit in `bx`, keeping its shape, never enlarged.
pub fn fit(w: i32, h: i32, bx: (i32, i32)) -> (i32, i32) {
    if w <= 0 || h <= 0 {
        return (1, 1);
    }
    let s = (bx.0 as f64 / w as f64).min(bx.1 as f64 / h as f64).min(1.0);
    (((w as f64 * s).round() as i32).max(1), ((h as f64 * s).round() as i32).max(1))
}

#[derive(Debug, PartialEq, Eq)]
pub struct ThumbMsg {
    pub id: u64,
    pub width: i32,
    pub height: i32,
    pub path: String,
}

pub fn parse(msg: &Value) -> Option<ThumbMsg> {
    let int = |k: &str| msg.get(k).and_then(Value::as_i64);
    Some(ThumbMsg {
        id: msg.get("id")?.as_u64()?,
        width: i32::try_from(int("width")?).ok().filter(|w| *w > 0)?,
        height: i32::try_from(int("height")?).ok().filter(|h| *h > 0)?,
        path: msg.get("path")?.as_str()?.to_string(),
    })
}

#[derive(Serialize, Clone)]
pub struct ThumbEvent {
    pub id: u64,
    pub version: u64,
}

/// The PNGs, by window id.
#[derive(Default)]
pub struct Thumbs {
    inner: Mutex<(u64, HashMap<u64, (u64, Vec<u8>)>)>,
}

impl Thumbs {
    /// Turn kompact's file into a PNG and keep it; returns the event for the dock.
    pub fn take(&self, m: &ThumbMsg) -> Option<ThumbEvent> {
        let started = std::time::Instant::now();
        let png = match png_of(m) {
            Ok(png) => png,
            Err(e) => {
                warn!("thumbnails: window {}: {e}", m.id);
                return None;
            }
        };
        let kib = png.len() / 1024;
        let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        inner.0 += 1;
        let version = inner.0;
        inner.1.insert(m.id, (version, png));
        info!("thumbnails: window {} ready ({} KiB, {} ms)", m.id, kib, started.elapsed().as_millis());
        Some(ThumbEvent { id: m.id, version })
    }

    pub fn png(&self, id: u64) -> Option<Vec<u8>> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner()).1.get(&id).map(|(_, png)| png.clone())
    }

    /// Forget windows that are gone.
    pub fn retain(&self, alive: &[u64]) {
        self.inner.lock().unwrap_or_else(|e| e.into_inner()).1.retain(|id, _| alive.contains(id));
    }
}

fn png_of(m: &ThumbMsg) -> Result<Vec<u8>, String> {
    use gtk::gdk_pixbuf::{Colorspace, InterpType, Pixbuf};
    let mut rgba = std::fs::read(&m.path).map_err(|e| format!("{}: {e}", m.path))?;
    let stride = m.width as usize * 4;
    if rgba.len() != stride * m.height as usize {
        return Err(format!("{} bytes, expected {}x{} RGBA", rgba.len(), m.width, m.height));
    }
    unpremultiply(&mut rgba);
    let full = Pixbuf::from_mut_slice(rgba, Colorspace::Rgb, true, 8, m.width, m.height, stride as i32);
    let (w, h) = fit(m.width, m.height, BOX);
    let small = if (w, h) == (m.width, m.height) {
        full
    } else {
        full.scale_simple(w, h, InterpType::Bilinear).ok_or("scaling failed")?
    };
    small.save_to_bufferv("png", &[]).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn straight_alpha() {
        let mut px = [0, 0, 0, 0, 255, 128, 0, 255, 64, 32, 0, 128];
        unpremultiply(&mut px);
        assert_eq!(px, [0, 0, 0, 0, 255, 128, 0, 255, 128, 64, 0, 128]);
    }

    #[test]
    fn fitting() {
        assert_eq!(fit(661, 400, BOX), (331, 200));
        assert_eq!(fit(704, 200, BOX), (352, 100));
        assert_eq!(fit(100, 50, BOX), (100, 50), "not enlarged");
        assert_eq!(fit(0, 5, BOX), (1, 1));
    }

    #[test]
    fn parses_messages() {
        let m = serde_json::json!({"type":"thumb","id":7,"width":10,"height":5,"path":"/run/x/7.rgba"});
        assert_eq!(parse(&m), Some(ThumbMsg { id: 7, width: 10, height: 5, path: "/run/x/7.rgba".into() }));
        assert_eq!(parse(&serde_json::json!({"id":7,"width":0,"height":5,"path":"p"})), None);
    }
}
