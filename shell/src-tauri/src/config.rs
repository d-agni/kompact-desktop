//! Start-up settings for kompact-shell, read from the environment. Pure, so `cargo test` covers it.

use std::path::Path;

/// Panel height in logical pixels. The panel reserves it as its exclusive zone, so windows that
/// maximize or snap stay below it.
pub const PANEL_HEIGHT: i32 = 28;

/// Panel background shown before the page has painted (same colour as `panel.css` and kompact's
/// clear colour), so there's no white flash at start.
pub const PANEL_BG: (f64, f64, f64) = (0.1, 0.1, 0.15);

/// `KOMPACT_SHELL_GPU`: whether WebKit may render with OpenGL.
///
/// Off by default: kompact offers clients no linux-dmabuf, so a GL client ends up on Mesa's
/// software fallback anyway, and nouveau on the GT 710 is the part we least want to stress.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Gpu {
    Off,
    On,
}

impl Gpu {
    pub fn from_env(value: Option<&str>) -> Result<Gpu, String> {
        match value.map(|v| v.trim().to_ascii_lowercase()).as_deref() {
            None | Some("") | Some("off") => Ok(Gpu::Off),
            Some("on") => Ok(Gpu::On),
            Some(other) => Err(format!("KOMPACT_SHELL_GPU={other}: expected on or off")),
        }
    }

    /// Environment variables to set before GTK and WebKit start, skipping any the user already
    /// set.
    ///
    /// Always `GDK_BACKEND=wayland`: the shell only makes sense as a Wayland layer-shell client.
    /// With the GPU off, WebKit (webkit2gtk-4.1, GTK3) doesn't try its DMA-BUF renderer (the
    /// GTK4 panel that drew in 6b ran like that), and, unless `compositing` (6t-a test variant,
    /// `KOMPACT_SHELL_COMPOSITING=on`), doesn't composite either.
    pub fn env_defaults(
        self,
        compositing: bool,
        get: impl Fn(&str) -> Option<String>,
    ) -> Vec<(&'static str, &'static str)> {
        let mut vars = vec![("GDK_BACKEND", "wayland")];
        if self == Gpu::Off {
            if !compositing {
                vars.push(("WEBKIT_DISABLE_COMPOSITING_MODE", "1"));
            }
            vars.push(("WEBKIT_DISABLE_DMABUF_RENDERER", "1"));
        }
        vars.retain(|(key, _)| get(key).is_none());
        vars
    }
}

/// URI scheme the wallpaper page loads its image from (`kompact-wallpaper://localhost/image`):
/// the page itself is served by Tauri and can't read files on disk.
pub const WALLPAPER_SCHEME: &str = "kompact-wallpaper";

/// MIME type for a wallpaper image, from its extension; `None` for formats we don't serve.
pub fn image_mime(path: &Path) -> Option<&'static str> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    Some(match ext.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "webp" => "image/webp",
        "gif" => "image/gif",
        "svg" => "image/svg+xml",
        "bmp" => "image/bmp",
        _ => return None,
    })
}

/// 14b: the image type from the first bytes, for files without an extension (AccountsService
/// keeps users' pictures as `/var/lib/AccountsService/icons/<user>`).
pub fn sniff_mime(head: &[u8]) -> Option<&'static str> {
    if head.starts_with(b"\x89PNG") {
        Some("image/png")
    } else if head.starts_with(&[0xff, 0xd8, 0xff]) {
        Some("image/jpeg")
    } else if head.starts_with(b"GIF8") {
        Some("image/gif")
    } else if head.len() >= 12 && &head[..4] == b"RIFF" && &head[8..12] == b"WEBP" {
        Some("image/webp")
    } else if head.starts_with(b"BM") {
        Some("image/bmp")
    } else {
        let text = String::from_utf8_lossy(&head[..head.len().min(256)]);
        (text.trim_start().starts_with("<svg") || (text.trim_start().starts_with("<?xml") && text.contains("<svg"))).then_some("image/svg+xml")
    }
}

/// URI scheme the launcher loads app icons from: `kompact-icon://localhost/<desktop id>`, the id
/// escaped with JS `encodeURIComponent`.
pub const ICON_SCHEME: &str = "kompact-icon";

/// Undo `%XX` escapes (what `encodeURIComponent` produces). `None` for a bad escape or a result
/// that isn't UTF-8.
pub fn percent_decode(s: &str) -> Option<String> {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hex = std::str::from_utf8(bytes.get(i + 1..i + 3)?).ok()?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

/// `KOMPACT_SHELL_COMPOSITING`: on or off (default off).
pub fn compositing_from_env(value: Option<&str>) -> Result<bool, String> {
    match value.map(|v| v.trim().to_ascii_lowercase()).as_deref() {
        None | Some("") | Some("off") => Ok(false),
        Some("on") => Ok(true),
        Some(other) => Err(format!("KOMPACT_SHELL_COMPOSITING={other}: expected on or off")),
    }
}

/// 6p-a: how the pages share WebKit. `KOMPACT_SHELL_WEB_PROCESS=shared` builds every page as a
/// "related view" of the first, so all of them live in one web process instead of one each
/// (measured 2026-10-09: four processes, ~690 MB; shared, one of 229 MB). Shared is the default
/// since 6p-c; `separate` keeps a crash in one page from blanking the others.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WebProcess {
    Separate,
    Shared,
}

impl WebProcess {
    pub fn from_env(value: Option<&str>) -> Result<WebProcess, String> {
        match value.map(|v| v.trim().to_ascii_lowercase()).as_deref() {
            None | Some("") | Some("shared") => Ok(WebProcess::Shared),
            Some("separate") => Ok(WebProcess::Separate),
            Some(other) => Err(format!("KOMPACT_SHELL_WEB_PROCESS={other}: expected shared or separate")),
        }
    }
}

/// 6p-a: WebKit's cache model. `KOMPACT_SHELL_CACHE=viewer` is its "document viewer" model: no
/// browser-sized memory cache, which local pages and images don't need; the default since 6p-c.
/// `browser` is WebKit's own default.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cache {
    Browser,
    Viewer,
}

impl Cache {
    pub fn from_env(value: Option<&str>) -> Result<Cache, String> {
        match value.map(|v| v.trim().to_ascii_lowercase()).as_deref() {
            None | Some("") | Some("viewer") => Ok(Cache::Viewer),
            Some("browser") => Ok(Cache::Browser),
            Some(other) => Err(format!("KOMPACT_SHELL_CACHE={other}: expected viewer or browser")),
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn web_process_and_cache_values() {
        use super::{Cache, WebProcess};
        assert_eq!(WebProcess::from_env(None), Ok(WebProcess::Shared));
        assert_eq!(WebProcess::from_env(Some(" Separate ")), Ok(WebProcess::Separate));
        assert!(WebProcess::from_env(Some("one")).is_err());
        assert_eq!(Cache::from_env(None), Ok(Cache::Viewer));
        assert_eq!(Cache::from_env(Some("browser")), Ok(Cache::Browser));
        assert!(Cache::from_env(Some("none")).is_err());
    }

    #[test]
    fn percent_decoding() {
        assert_eq!(percent_decode("firefox_firefox.desktop").as_deref(), Some("firefox_firefox.desktop"));
        assert_eq!(percent_decode("My%20App%C3%A9.desktop").as_deref(), Some("My Appé.desktop"));
        assert_eq!(percent_decode("bad%2"), None);
        assert_eq!(percent_decode("bad%zz"), None);
        assert_eq!(percent_decode("%FF"), None);
    }

    #[test]
    fn sniffed_mimes() {
        assert_eq!(sniff_mime(b"\x89PNG\r\n\x1a\n...."), Some("image/png"));
        assert_eq!(sniff_mime(&[0xff, 0xd8, 0xff, 0xe0, 0, 0x10]), Some("image/jpeg"));
        assert_eq!(sniff_mime(b"RIFF\0\0\0\0WEBPVP8 "), Some("image/webp"));
        assert_eq!(sniff_mime(b"<?xml version=\"1.0\"?>\n<svg xmlns="), Some("image/svg+xml"));
        assert_eq!(sniff_mime(b"#!/bin/sh\nrm -rf"), None);
        assert_eq!(sniff_mime(b""), None);
    }

    #[test]
    fn image_mimes() {
        assert_eq!(image_mime(Path::new("/x/y.PNG")), Some("image/png"));
        assert_eq!(image_mime(Path::new("y.jpeg")), Some("image/jpeg"));
        assert_eq!(image_mime(Path::new("y.svg")), Some("image/svg+xml"));
        assert_eq!(image_mime(Path::new("y.txt")), None);
        assert_eq!(image_mime(Path::new("noext")), None);
    }

    use super::*;

    fn env<'a>(vars: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<String> + 'a {
        move |key| vars.iter().find(|(k, _)| *k == key).map(|(_, v)| v.to_string())
    }

    #[test]
    fn gpu_values() {
        assert_eq!(Gpu::from_env(None), Ok(Gpu::Off));
        assert_eq!(Gpu::from_env(Some("")), Ok(Gpu::Off));
        assert_eq!(Gpu::from_env(Some("OFF")), Ok(Gpu::Off));
        assert_eq!(Gpu::from_env(Some(" On ")), Ok(Gpu::On));
        assert!(Gpu::from_env(Some("yes")).unwrap_err().contains("expected on or off"));
    }

    #[test]
    fn env_defaults_gpu_off() {
        assert_eq!(
            Gpu::Off.env_defaults(false, env(&[])),
            vec![("GDK_BACKEND", "wayland"), ("WEBKIT_DISABLE_COMPOSITING_MODE", "1"), ("WEBKIT_DISABLE_DMABUF_RENDERER", "1")]
        );
        // A value the user set wins.
        assert_eq!(
            Gpu::Off.env_defaults(false, env(&[("WEBKIT_DISABLE_COMPOSITING_MODE", "0"), ("GDK_BACKEND", "wayland")])),
            vec![("WEBKIT_DISABLE_DMABUF_RENDERER", "1")]
        );
    }

    #[test]
    fn env_defaults_gpu_on() {
        assert_eq!(Gpu::On.env_defaults(false, env(&[])), vec![("GDK_BACKEND", "wayland")]);
        assert_eq!(Gpu::On.env_defaults(true, env(&[])), vec![("GDK_BACKEND", "wayland")]);
    }

    #[test]
    fn compositing_variant() {
        assert_eq!(compositing_from_env(None), Ok(false));
        assert_eq!(compositing_from_env(Some("ON")), Ok(true));
        assert!(compositing_from_env(Some("1")).is_err());
        assert_eq!(
            Gpu::Off.env_defaults(true, env(&[])),
            vec![("GDK_BACKEND", "wayland"), ("WEBKIT_DISABLE_DMABUF_RENDERER", "1")]
        );
    }
}
