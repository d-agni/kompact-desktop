//! Start-up settings for kompact-shell, read from the environment. Pure, so `cargo test` covers it.

use std::path::{Path, PathBuf};

/// Panel height in logical pixels. The panel reserves it as its exclusive zone, so windows that
/// maximize or snap stay below it.
pub const PANEL_HEIGHT: i32 = 28;

/// Panel background shown before the page has painted (same colour as `panel.css` and kompact's
/// clear colour), so there's no white flash at start.
pub const PANEL_BG: (f32, f32, f32) = (0.1, 0.1, 0.15);

/// `KOMPACT_SHELL_GPU`: whether GTK and WebKit may render with OpenGL.
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
}

/// `KOMPACT_WALLPAPER` (6b): what the background layer shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Wallpaper {
    /// Unset or empty: the page's CSS gradient.
    Gradient,
    /// An image file, scaled to cover each output.
    Image(PathBuf),
    /// `none`: no wallpaper surface at all (kompact's clear colour shows, as before 6b).
    Off,
}

impl Wallpaper {
    pub fn from_env(value: Option<&str>) -> Wallpaper {
        match value.map(str::trim) {
            None | Some("") => Wallpaper::Gradient,
            Some(v) if v.eq_ignore_ascii_case("none") => Wallpaper::Off,
            Some(v) => Wallpaper::Image(PathBuf::from(v)),
        }
    }

    /// Query string for `wallpaper.html`, given the image's `file://` URI (from GIO, so it's
    /// already a valid URI): `?image=<uri, escaped as a query value>`, or empty.
    pub fn query(&self, image_uri: Option<&str>) -> String {
        match (self, image_uri) {
            (Wallpaper::Image(_), Some(uri)) => format!("?image={}", query_escape(uri)),
            _ => String::new(),
        }
    }
}

/// Escape a string as one query-string value, like JS `encodeURIComponent`, so the page's
/// `URLSearchParams` gets it back unchanged.
pub fn query_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'!' | b'~' | b'*' | b'\'' | b'(' | b')' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    /// Directory holding the HTML/CSS/JS pages (`KOMPACT_SHELL_WEB`, else the crate's `web/`).
    pub web_dir: PathBuf,
    pub gpu: Gpu,
    pub wallpaper: Wallpaper,
}

impl Config {
    pub fn from_env(get: impl Fn(&str) -> Option<String>, default_web: &Path) -> Result<Config, String> {
        let gpu = Gpu::from_env(get("KOMPACT_SHELL_GPU").as_deref())?;
        let web_dir = match get("KOMPACT_SHELL_WEB") {
            Some(dir) if !dir.trim().is_empty() => PathBuf::from(dir.trim()),
            _ => default_web.to_path_buf(),
        };
        let wallpaper = Wallpaper::from_env(get("KOMPACT_WALLPAPER").as_deref());
        Ok(Config { web_dir, gpu, wallpaper })
    }

    pub fn page(&self, name: &str) -> PathBuf {
        self.web_dir.join(name)
    }

    /// Environment variables to set before GTK and WebKit start, skipping any the user already
    /// set.
    ///
    /// Always: `GDK_DEBUG=no-portals`. GTK 4.22 asks xdg-desktop-portal for its version at
    /// start-up and waits up to 25 s when the portal is stuck starting (seen on the GT 710
    /// machine); the shell uses no portals. With the GPU off, GTK also draws with cairo and
    /// WebKit doesn't try its DMA-BUF renderer.
    pub fn env_defaults(&self, get: impl Fn(&str) -> Option<String>) -> Vec<(&'static str, &'static str)> {
        let mut vars = vec![("GDK_DEBUG", "no-portals")];
        if self.gpu == Gpu::Off {
            vars.extend([("GSK_RENDERER", "cairo"), ("WEBKIT_DISABLE_DMABUF_RENDERER", "1")]);
        }
        vars.retain(|(key, _)| get(key).is_none());
        vars
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env<'a>(vars: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<String> + 'a {
        move |key| vars.iter().find(|(k, _)| *k == key).map(|(_, v)| v.to_string())
    }

    const WEB: &str = "/opt/kompact-shell/web";

    #[test]
    fn defaults() {
        let c = Config::from_env(env(&[]), Path::new(WEB)).unwrap();
        assert_eq!(c, Config { web_dir: WEB.into(), gpu: Gpu::Off, wallpaper: Wallpaper::Gradient });
        assert_eq!(c.page("panel.html"), PathBuf::from("/opt/kompact-shell/web/panel.html"));
    }

    #[test]
    fn web_dir_override() {
        let c = Config::from_env(env(&[("KOMPACT_SHELL_WEB", " /tmp/web ")]), Path::new(WEB)).unwrap();
        assert_eq!(c.web_dir, PathBuf::from("/tmp/web"));
        let c = Config::from_env(env(&[("KOMPACT_SHELL_WEB", "")]), Path::new(WEB)).unwrap();
        assert_eq!(c.web_dir, PathBuf::from(WEB));
    }

    #[test]
    fn gpu_values() {
        assert_eq!(Gpu::from_env(None), Ok(Gpu::Off));
        assert_eq!(Gpu::from_env(Some("")), Ok(Gpu::Off));
        assert_eq!(Gpu::from_env(Some("OFF")), Ok(Gpu::Off));
        assert_eq!(Gpu::from_env(Some(" On ")), Ok(Gpu::On));
        assert!(Gpu::from_env(Some("yes")).unwrap_err().contains("expected on or off"));
        assert!(Config::from_env(env(&[("KOMPACT_SHELL_GPU", "1")]), Path::new(WEB)).is_err());
    }

    #[test]
    fn env_defaults_gpu_off() {
        let c = Config::from_env(env(&[]), Path::new(WEB)).unwrap();
        assert_eq!(
            c.env_defaults(env(&[])),
            vec![("GDK_DEBUG", "no-portals"), ("GSK_RENDERER", "cairo"), ("WEBKIT_DISABLE_DMABUF_RENDERER", "1")]
        );
        // A value the user set wins.
        assert_eq!(
            c.env_defaults(env(&[("GSK_RENDERER", "gl"), ("GDK_DEBUG", "")])),
            vec![("WEBKIT_DISABLE_DMABUF_RENDERER", "1")]
        );
    }

    #[test]
    fn env_defaults_gpu_on() {
        let c = Config::from_env(env(&[("KOMPACT_SHELL_GPU", "on")]), Path::new(WEB)).unwrap();
        assert_eq!(c.env_defaults(env(&[])), vec![("GDK_DEBUG", "no-portals")]);
    }

    #[test]
    fn wallpaper_values() {
        assert_eq!(Wallpaper::from_env(None), Wallpaper::Gradient);
        assert_eq!(Wallpaper::from_env(Some("  ")), Wallpaper::Gradient);
        assert_eq!(Wallpaper::from_env(Some("None")), Wallpaper::Off);
        assert_eq!(Wallpaper::from_env(Some(" /home/u/Pictures/a b.jpg ")), Wallpaper::Image("/home/u/Pictures/a b.jpg".into()));
        let c = Config::from_env(env(&[("KOMPACT_WALLPAPER", "none")]), Path::new(WEB)).unwrap();
        assert_eq!(c.wallpaper, Wallpaper::Off);
    }

    #[test]
    fn wallpaper_query() {
        let img = Wallpaper::Image("/x/a b.jpg".into());
        assert_eq!(img.query(Some("file:///x/a%20b.jpg")), "?image=file%3A%2F%2F%2Fx%2Fa%2520b.jpg");
        assert_eq!(img.query(None), "");
        assert_eq!(Wallpaper::Gradient.query(Some("file:///x.jpg")), "");
        assert_eq!(query_escape("a-Z_0.~*'()&=?#+é"), "a-Z_0.~*'()%26%3D%3F%23%2B%C3%A9");
    }
}
