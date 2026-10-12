//! 14a: what the lock page shows, worked out from plain files (pure, tested): the user's name and
//! picture, the wallpaper and the light/dark look kompact uses, and how long to wait after wrong
//! passwords.

use std::path::{Path, PathBuf};
use std::time::Duration;

/// The full name from a passwd GECOS field ("Sam Lee,,," → "Sam Lee"), else the login name.
pub fn display_name(gecos: &str, login: &str) -> String {
    let name = gecos.split(',').next().unwrap_or("").trim();
    if name.is_empty() { login.to_string() } else { name.to_string() }
}

/// The user's picture: AccountsService's (what GNOME Settings sets), else `~/.face`, the first
/// that exists. `None`: the page draws the initials.
pub fn avatar(login: &str, home: &Path, exists: impl Fn(&Path) -> bool) -> Option<PathBuf> {
    [PathBuf::from(format!("/var/lib/AccountsService/icons/{login}")), home.join(".face"), home.join(".face.icon")]
        .into_iter()
        .find(|p| exists(p))
}

/// "SL" from "Sam Lee", "S" from "sam".
pub fn initials(name: &str) -> String {
    let words: Vec<&str> = name.split_whitespace().collect();
    let pick = |w: &str| w.chars().next().map(|c| c.to_uppercase().collect::<String>()).unwrap_or_default();
    match words.as_slice() {
        [] => "?".into(),
        [one] => pick(one),
        [first, .., last] => format!("{}{}", pick(first), pick(last)),
    }
}

/// The wallpaper image from the shell's `~/.config/kompact/wallpaper` (`kind=image`,
/// `image=<path>`); `None` for a colour or gradient (the page draws its own).
pub fn wallpaper_image(text: &str) -> Option<PathBuf> {
    let mut kind = "";
    let mut image = None;
    for line in text.lines().map(str::trim).filter(|l| !l.starts_with('#')) {
        match line.split_once('=') {
            Some(("kind", v)) => kind = v.trim(),
            Some(("image", v)) if v.trim().starts_with('/') => image = Some(PathBuf::from(v.trim())),
            _ => {}
        }
    }
    (kind == "image").then_some(image).flatten()
}

/// Light or dark, and the accent, from kompact's `~/.config/kompact/settings` (`appearance`,
/// `accent`); `auto` is light from 7:00 to 19:00, as kompact does it.
pub fn appearance(text: &str, hour: u32) -> (bool, String) {
    let mut appearance = "dark";
    let mut accent = "blue";
    for line in text.lines().map(str::trim) {
        match line.split_once('=') {
            Some(("appearance", v)) => appearance = v.trim(),
            Some(("accent", v)) => accent = v.trim(),
            _ => {}
        }
    }
    let dark = match appearance {
        "light" => false,
        "auto" => !(7..19).contains(&hour),
        _ => true,
    };
    let accent = if accent.bytes().all(|b| b.is_ascii_lowercase()) && !accent.is_empty() { accent } else { "blue" };
    (dark, accent.to_string())
}

/// After `failures` wrong passwords in a row, how long the field stays closed (PAM adds its own
/// ~3 s on each): nothing for the first 3, then 5 s, 15 s, 30 s, then a minute each.
pub fn wait_after(failures: u32) -> Duration {
    Duration::from_secs(match failures {
        0..=2 => 0,
        3 => 5,
        4 => 15,
        5 => 30,
        _ => 60,
    })
}

/// The PAM service to check the password with: kompact's own if installed (14d), else
/// swaylock's (it ships one), else `login`.
pub fn pam_service(exists: impl Fn(&Path) -> bool) -> &'static str {
    ["kompact", "swaylock"].into_iter().find(|s| exists(&Path::new("/etc/pam.d").join(s))).unwrap_or("login")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_and_pictures() {
        assert_eq!(display_name("Sam Lee,,,", "sam"), "Sam Lee");
        assert_eq!(display_name(",,,", "sam"), "sam");
        assert_eq!(display_name("", "u"), "u");
        assert_eq!(initials("Sam Lee"), "SL");
        assert_eq!(initials("sam"), "S");
        assert_eq!(initials("Ana Maria de Souza"), "AS");
        assert_eq!(initials(" "), "?");
        let home = Path::new("/home/u");
        assert_eq!(avatar("u", home, |p| p.ends_with(".face")), Some(PathBuf::from("/home/u/.face")));
        assert_eq!(avatar("u", home, |p| p.starts_with("/var")), Some(PathBuf::from("/var/lib/AccountsService/icons/u")));
        assert_eq!(avatar("u", home, |_| false), None);
    }

    #[test]
    fn wallpaper_and_look() {
        let w = "# x\nkind=image\nimage=/usr/share/backgrounds/a b.jpg\n";
        assert_eq!(wallpaper_image(w), Some(PathBuf::from("/usr/share/backgrounds/a b.jpg")));
        assert_eq!(wallpaper_image("kind=gradient\nimage=/x.png\n"), None);
        assert_eq!(wallpaper_image("kind=image\nimage=relative.png\n"), None);
        assert_eq!(appearance("appearance=light\naccent=pink\n", 12), (false, "pink".into()));
        assert_eq!(appearance("appearance=auto\n", 12), (false, "blue".into()));
        assert_eq!(appearance("appearance=auto\n", 22), (true, "blue".into()));
        assert_eq!(appearance("accent=\"><script>\n", 12), (true, "blue".into()));
        assert_eq!(appearance("", 12), (true, "blue".into()));
    }

    #[test]
    fn waits_and_service() {
        assert_eq!(wait_after(2), Duration::ZERO);
        assert_eq!(wait_after(3), Duration::from_secs(5));
        assert_eq!(wait_after(9), Duration::from_secs(60));
        assert_eq!(pam_service(|p| p.ends_with("swaylock")), "swaylock");
        assert_eq!(pam_service(|_| true), "kompact");
        assert_eq!(pam_service(|_| false), "login");
    }
}
