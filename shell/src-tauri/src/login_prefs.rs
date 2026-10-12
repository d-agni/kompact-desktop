//! 14b: how your lock (and, from 14c, login) page looks, chosen in System Settings → Users &
//! Login and kept in `~/.config/kompact/login` (`background=`, `image=`, `message=`). Shared by
//! the shell (writes it) and `kompact-lock` (reads it at each lock); pure and tested. Your name
//! and picture are AccountsService's (system-wide, so the login page can show them too).

// Two binaries include this file (the shell writes, kompact-lock reads); each uses half.
#![allow(dead_code)]

use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Background {
    /// The desktop wallpaper, blurred (macOS's look; the default).
    DesktopBlur,
    /// The desktop wallpaper as it is.
    Desktop,
    /// A picture of its own (`image`).
    Image,
}

impl Background {
    pub fn parse(s: &str) -> Option<Background> {
        match s {
            "desktop-blur" => Some(Background::DesktopBlur),
            "desktop" => Some(Background::Desktop),
            "image" => Some(Background::Image),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Background::DesktopBlur => "desktop-blur",
            Background::Desktop => "desktop",
            Background::Image => "image",
        }
    }
}

/// Longest lock-screen message, in characters.
pub const MAX_MESSAGE: usize = 120;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoginPrefs {
    pub background: Background,
    pub image: Option<PathBuf>,
    /// Shown under the time ("If found, call …"); empty for none.
    pub message: String,
}

impl Default for LoginPrefs {
    fn default() -> Self {
        LoginPrefs { background: Background::DesktopBlur, image: None, message: String::new() }
    }
}

/// One line, no control characters, at most `MAX_MESSAGE` characters.
pub fn clean_message(m: &str) -> String {
    let one: String = m.chars().map(|c| if c.is_control() { ' ' } else { c }).collect();
    one.split_whitespace().collect::<Vec<_>>().join(" ").chars().take(MAX_MESSAGE).collect()
}

impl LoginPrefs {
    pub fn parse(text: &str) -> LoginPrefs {
        let mut p = LoginPrefs::default();
        for line in text.lines().filter(|l| !l.trim_start().starts_with('#')) {
            let Some((k, v)) = line.split_once('=') else { continue };
            match k.trim() {
                "background" => p.background = Background::parse(v.trim()).unwrap_or(p.background),
                "image" => p.image = Some(v.trim()).filter(|v| v.starts_with('/')).map(PathBuf::from),
                "message" => p.message = clean_message(v),
                _ => {}
            }
        }
        p
    }

    pub fn to_text(&self) -> String {
        format!(
            "# kompact lock and login page (written by System Settings → Users & Login)\nbackground={}\nimage={}\nmessage={}\n",
            self.background.name(),
            self.image.as_ref().map(|p| p.display().to_string()).unwrap_or_default(),
            self.message
        )
    }

    /// The picture behind the page and whether to blur it: its own image (if it's still there),
    /// else the desktop's wallpaper (`desktop`, `None` for a colour or gradient).
    pub fn backdrop(&self, desktop: Option<PathBuf>, exists: impl Fn(&Path) -> bool) -> (Option<PathBuf>, bool) {
        match self.background {
            Background::Image => match self.image.clone().filter(|p| exists(p)) {
                Some(p) => (Some(p), false),
                None => (desktop, true),
            },
            Background::Desktop => (desktop, false),
            Background::DesktopBlur => (desktop, true),
        }
    }
}

pub fn path(config_home: &Path) -> PathBuf {
    config_home.join("kompact").join("login")
}

/// 24-hour time, from the menu-bar clock's `~/.config/kompact/clock` (`24-hour=on|off`; on if
/// unset, as the menu bar).
pub fn hour24(clock: &str) -> bool {
    !clock.lines().any(|l| l.split_once('=').is_some_and(|(k, v)| k.trim() == "24-hour" && v.trim() == "off"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn login_prefs() {
        assert_eq!(LoginPrefs::parse(""), LoginPrefs::default());
        let p = LoginPrefs { background: Background::Image, image: Some("/home/u/Pictures/a b.jpg".into()), message: "If found, call 555-0100".into() };
        assert_eq!(LoginPrefs::parse(&p.to_text()), p);
        let q = LoginPrefs::parse("background=sparkly\nimage=relative.png\nmessage= two\nlines\n");
        assert_eq!(q.background, Background::DesktopBlur);
        assert_eq!(q.image, None);
        assert_eq!(q.message, "two");
        assert_eq!(clean_message("a\tb\u{7}c  d"), "a b c d");
        assert_eq!(clean_message(&"x".repeat(500)).chars().count(), MAX_MESSAGE);
    }

    #[test]
    fn backdrops() {
        let desk = Some(PathBuf::from("/usr/share/backgrounds/w.jpg"));
        let mut p = LoginPrefs::default();
        assert_eq!(p.backdrop(desk.clone(), |_| true), (desk.clone(), true));
        p.background = Background::Desktop;
        assert_eq!(p.backdrop(desk.clone(), |_| true), (desk.clone(), false));
        p.background = Background::Image;
        p.image = Some("/home/u/me.png".into());
        assert_eq!(p.backdrop(desk.clone(), |_| true), (Some("/home/u/me.png".into()), false));
        // Its image is gone: the blurred desktop.
        assert_eq!(p.backdrop(desk.clone(), |_| false), (desk, true));
        assert!(hour24(""));
        assert!(!hour24("# x\n24-hour=off\n"));
        assert!(hour24("24-hour=on\n"));
    }
}
