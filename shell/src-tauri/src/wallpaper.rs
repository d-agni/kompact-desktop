//! 10d-2: the wallpaper, chosen in System Settings → Wallpaper and kept in
//! `~/.config/kompact/wallpaper` (`kind=gradient|color|image`, `color=#rrggbb`, `image=<path>`).
//! It changes at once: the wallpaper pages, the frosted menu bar / dock / Launchpad (vibrancy)
//! and the workspace overview all listen for `kompact-wallpaper-changed` and reload with the new
//! `version` in their URLs.
//!
//! `KOMPACT_WALLPAPER` (an image, or `none` for no wallpaper surface), if set, still wins for the
//! session and the pane says so; `KOMPACT_WALLPAPER_DEFAULT` is the picture used while there's no
//! settings file (`ktest-desktop.sh` passes the Raccoon one).
//!
//! `kompact-wallpaper://localhost/…` serves `/image`, `/blurred` (vibrancy), `/small` (the
//! overview) and `/preview/<n>` (picture <n> of the last `wallpaper_pictures`, for the pane). The
//! decoding runs on one worker thread, so 4K pictures neither stall GTK nor all decode at once.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{mpsc, Arc, Mutex};

use serde_json::{json, Value};
use tauri::{AppHandle, Emitter, State};
use tracing::{info, warn};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Choice {
    /// The page's CSS gradient (the default).
    Gradient,
    /// `#rrggbb`, lowercase.
    Color(String),
    Image(PathBuf),
}

pub fn valid_color(s: &str) -> bool {
    s.len() == 7 && s.starts_with('#') && s[1..].bytes().all(|b| b.is_ascii_hexdigit())
}

/// A file we can show: one of the image types we serve.
pub fn is_picture(path: &Path) -> bool {
    crate::config::image_mime(path).is_some()
}

/// The settings file; `None` if it says nothing usable.
pub fn parse(text: &str) -> Option<Choice> {
    let mut kind = None;
    let mut color = None;
    let mut image = None;
    for line in text.lines().map(str::trim).filter(|l| !l.starts_with('#')) {
        let Some((k, v)) = line.split_once('=') else { continue };
        match k.trim() {
            "kind" => kind = Some(v.trim().to_string()),
            "color" => color = Some(v.trim().to_ascii_lowercase()),
            "image" => image = Some(PathBuf::from(v.trim())),
            _ => {}
        }
    }
    match kind.as_deref()? {
        "gradient" => Some(Choice::Gradient),
        "color" => color.filter(|c| valid_color(c)).map(Choice::Color),
        "image" => image.filter(|p| p.is_absolute() && is_picture(p)).map(Choice::Image),
        _ => None,
    }
}

pub fn to_text(c: &Choice) -> String {
    let body = match c {
        Choice::Gradient => "kind=gradient\n".to_string(),
        Choice::Color(c) => format!("kind=color\ncolor={c}\n"),
        Choice::Image(p) => format!("kind=image\nimage={}\n", p.display()),
    };
    format!("# kompact wallpaper (written by System Settings → Wallpaper)\n{body}")
}

/// What to start with, and whether `KOMPACT_WALLPAPER` holds it (`locked`) or turns the wallpaper
/// off altogether (`off`).
#[derive(Debug, PartialEq, Eq)]
pub struct Start {
    pub choice: Choice,
    pub locked: bool,
    pub off: bool,
}

pub fn initial(env: Option<&str>, file: Option<&str>, default: Option<&str>) -> Start {
    match env.map(str::trim).filter(|v| !v.is_empty()) {
        Some(v) if v.eq_ignore_ascii_case("none") => Start { choice: Choice::Gradient, locked: true, off: true },
        Some(v) => Start { choice: Choice::Image(PathBuf::from(v)), locked: true, off: false },
        None => {
            let choice = file
                .and_then(parse)
                .or_else(|| default.map(str::trim).filter(|d| !d.is_empty()).map(|d| Choice::Image(PathBuf::from(d))))
                .unwrap_or(Choice::Gradient);
            Start { choice, locked: false, off: false }
        }
    }
}

/// Where pictures are offered from: `backgrounds` in every data directory (Ubuntu's are in
/// `/usr/share/backgrounds`), and `~/Pictures` with `~/Pictures/Wallpapers`.
pub fn picture_dirs(home: Option<&Path>, data_dirs: &[PathBuf]) -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = data_dirs.iter().map(|d| d.join("backgrounds")).collect();
    if let Some(h) = home {
        dirs.push(h.join("Pictures"));
        dirs.push(h.join("Pictures/Wallpapers"));
    }
    dirs.dedup();
    dirs
}

/// At most this many pictures in the pane.
pub const MAX_PICTURES: usize = 300;

/// The pictures in `dirs` (not their subfolders), by folder then name, at most `MAX_PICTURES`.
pub fn scan(dirs: &[PathBuf]) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for dir in dirs {
        let Ok(entries) = std::fs::read_dir(dir) else { continue };
        let mut files: Vec<PathBuf> = entries.flatten().map(|e| e.path()).filter(|p| p.is_file() && is_picture(p)).collect();
        files.sort();
        for f in files {
            if out.len() == MAX_PICTURES {
                return out;
            }
            if !out.contains(&f) {
                out.push(f);
            }
        }
    }
    out
}

fn config_path() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
    Some(base.join("kompact").join("wallpaper"))
}

/// Long side of the pane's previews.
const PREVIEW_SIZE: i32 = 320;

enum Job {
    Blurred(u64, PathBuf),
    Small(u64, PathBuf),
    Preview(PathBuf),
}

struct Inner {
    choice: Choice,
    locked: bool,
    off: bool,
    /// Bumped on every change; pages put it in their URLs.
    version: u64,
    blurred: Option<(u64, Option<Vec<u8>>)>,
    small: Option<(u64, Option<Vec<u8>>)>,
    pictures: Vec<PathBuf>,
    previews: HashMap<PathBuf, Option<Vec<u8>>>,
}

pub struct Wallpapers {
    inner: Arc<Mutex<Inner>>,
    jobs: Mutex<mpsc::Sender<(Job, tauri::UriSchemeResponder)>>,
}

fn respond(responder: tauri::UriSchemeResponder, found: Option<(Vec<u8>, &str)>) {
    let r = match found {
        Some((bytes, mime)) => tauri::http::Response::builder().header(tauri::http::header::CONTENT_TYPE, mime).body(bytes),
        None => tauri::http::Response::builder().status(404).body(Vec::new()),
    };
    responder.respond(r.unwrap_or_default());
}

/// 14d: the desktop's picture now, if it's one (the login screen gets a copy, `users::publish`).
pub fn current_image() -> Option<PathBuf> {
    let file = config_path().and_then(|p| std::fs::read_to_string(p).ok());
    let start = initial(
        std::env::var("KOMPACT_WALLPAPER").ok().as_deref(),
        file.as_deref(),
        std::env::var("KOMPACT_WALLPAPER_DEFAULT").ok().as_deref(),
    );
    match start.choice {
        Choice::Image(p) if p.is_file() && is_picture(&p) => Some(p),
        _ => None,
    }
}

impl Wallpapers {
    /// From the environment and the settings file; images that aren't there fall back to the
    /// gradient.
    pub fn load() -> Self {
        let file = config_path().and_then(|p| std::fs::read_to_string(p).ok());
        let mut start = initial(
            std::env::var("KOMPACT_WALLPAPER").ok().as_deref(),
            file.as_deref(),
            std::env::var("KOMPACT_WALLPAPER_DEFAULT").ok().as_deref(),
        );
        if let Choice::Image(p) = &start.choice {
            if !p.is_file() || !is_picture(p) {
                warn!("wallpaper: {} isn't a picture we can show, using the gradient", p.display());
                start.choice = Choice::Gradient;
            }
        }
        info!(
            "wallpaper: {}{}",
            describe(&start.choice),
            if start.off { " (off: KOMPACT_WALLPAPER=none)" } else if start.locked { " (KOMPACT_WALLPAPER, Settings can't change it)" } else { "" }
        );
        let inner = Arc::new(Mutex::new(Inner {
            choice: start.choice,
            locked: start.locked,
            off: start.off,
            version: 1,
            blurred: None,
            small: None,
            pictures: Vec::new(),
            previews: HashMap::new(),
        }));
        let (tx, rx) = mpsc::channel::<(Job, tauri::UriSchemeResponder)>();
        let worker = inner.clone();
        std::thread::Builder::new()
            .name("wallpaper".into())
            .spawn(move || {
                for (job, responder) in rx {
                    let found = run_job(&worker, job);
                    respond(responder, found);
                }
            })
            .expect("wallpaper worker thread");
        Wallpapers { inner, jobs: Mutex::new(tx) }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn is_off(&self) -> bool {
        self.lock().off
    }

    pub fn state(&self) -> Value {
        state_json(&self.lock())
    }

    /// `kompact-wallpaper://localhost<path>` (the query, `?v=…`, is only for the page's cache).
    pub fn serve(&self, path: &str, responder: tauri::UriSchemeResponder) {
        let job = {
            let inner = self.lock();
            let image = match &inner.choice {
                Choice::Image(p) => Some(p.clone()),
                _ => None,
            };
            match (path, image) {
                ("/image", Some(p)) => {
                    // Read here: no decoding, and the page wants it now.
                    let found = std::fs::read(&p).ok().map(|b| (b, crate::config::image_mime(&p).unwrap_or("application/octet-stream")));
                    if found.is_none() {
                        warn!("wallpaper: can't read {}", p.display());
                    }
                    drop(inner);
                    return respond(responder, found);
                }
                ("/blurred", Some(p)) => match &inner.blurred {
                    Some((v, bytes)) if *v == inner.version => {
                        let found = bytes.clone().map(|b| (b, "image/png"));
                        drop(inner);
                        return respond(responder, found);
                    }
                    _ => Job::Blurred(inner.version, p),
                },
                ("/small", Some(p)) => match &inner.small {
                    Some((v, bytes)) if *v == inner.version => {
                        let found = bytes.clone().map(|b| (b, "image/png"));
                        drop(inner);
                        return respond(responder, found);
                    }
                    _ => Job::Small(inner.version, p),
                },
                (p, _) if p.starts_with("/preview/") => {
                    let picture = p["/preview/".len()..].parse::<usize>().ok().and_then(|i| inner.pictures.get(i).cloned());
                    match picture {
                        Some(pic) => match inner.previews.get(&pic) {
                            Some(bytes) => {
                                let found = bytes.clone().map(|b| (b, "image/jpeg"));
                                drop(inner);
                                return respond(responder, found);
                            }
                            None => Job::Preview(pic),
                        },
                        None => {
                            drop(inner);
                            return respond(responder, None);
                        }
                    }
                }
                _ => {
                    drop(inner);
                    return respond(responder, None);
                }
            }
        };
        if let Err(mpsc::SendError((_, responder))) = self.jobs.lock().unwrap_or_else(|e| e.into_inner()).send((job, responder)) {
            respond(responder, None);
        }
    }
}

fn describe(c: &Choice) -> String {
    match c {
        Choice::Gradient => "the gradient".into(),
        Choice::Color(c) => format!("colour {c}"),
        Choice::Image(p) => format!("picture {}", p.display()),
    }
}

fn state_json(inner: &Inner) -> Value {
    let (kind, color, image) = match &inner.choice {
        Choice::Gradient => ("gradient", None, None),
        Choice::Color(c) => ("color", Some(c.clone()), None),
        Choice::Image(p) => ("image", None, Some(p.display().to_string())),
    };
    json!({"kind": kind, "color": color, "image": image, "version": inner.version, "locked": inner.locked, "off": inner.off})
}

fn run_job(inner: &Mutex<Inner>, job: Job) -> Option<(Vec<u8>, &'static str)> {
    match job {
        Job::Blurred(version, p) => {
            let bytes = crate::small_png(&p, crate::BLUR_RADIUS);
            let mut i = inner.lock().unwrap_or_else(|e| e.into_inner());
            if i.version == version {
                i.blurred = Some((version, bytes.clone()));
            }
            bytes.map(|b| (b, "image/png"))
        }
        Job::Small(version, p) => {
            let bytes = crate::small_png(&p, 0);
            let mut i = inner.lock().unwrap_or_else(|e| e.into_inner());
            if i.version == version {
                i.small = Some((version, bytes.clone()));
            }
            bytes.map(|b| (b, "image/png"))
        }
        Job::Preview(p) => {
            // Asked twice while queued: the first made it.
            if let Some(done) = inner.lock().unwrap_or_else(|e| e.into_inner()).previews.get(&p) {
                return done.clone().map(|b| (b, "image/jpeg"));
            }
            let bytes = preview_jpeg(&p);
            inner.lock().unwrap_or_else(|e| e.into_inner()).previews.insert(p, bytes.clone());
            bytes.map(|b| (b, "image/jpeg"))
        }
    }
}

fn preview_jpeg(file: &Path) -> Option<Vec<u8>> {
    use gtk::gdk_pixbuf::Pixbuf;
    let (_, w, h) = Pixbuf::file_info(file)?;
    let (sw, sh) = crate::blur::small_size(w, h, PREVIEW_SIZE);
    let pix = Pixbuf::from_file_at_scale(file, sw, sh, false).ok()?;
    // JPEG has no alpha.
    let pix = if pix.has_alpha() {
        let flat = Pixbuf::new(gtk::gdk_pixbuf::Colorspace::Rgb, false, 8, pix.width(), pix.height())?;
        flat.fill(0x202028ff);
        pix.composite(&flat, 0, 0, pix.width(), pix.height(), 0.0, 0.0, 1.0, 1.0, gtk::gdk_pixbuf::InterpType::Nearest, 255);
        flat
    } else {
        pix
    };
    match pix.save_to_bufferv("jpeg", &[("quality", "85")]) {
        Ok(b) => Some(b),
        Err(e) => {
            warn!("wallpaper: no preview of {}: {e}", file.display());
            None
        }
    }
}

/// Change it, save it and tell every page.
fn apply(app: &AppHandle, w: &Wallpapers, choice: Choice) -> Result<Value, String> {
    let state = {
        let mut inner = w.lock();
        if inner.locked {
            return Err("KOMPACT_WALLPAPER is set for this session".into());
        }
        if let Choice::Image(p) = &choice {
            if !p.is_file() || !is_picture(p) {
                return Err(format!("{} isn't a picture kompact can show", p.display()));
            }
        }
        inner.choice = choice.clone();
        inner.version += 1;
        inner.blurred = None;
        inner.small = None;
        state_json(&inner)
    };
    let path = config_path().ok_or("no home directory")?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    std::fs::write(&path, to_text(&choice)).map_err(|e| format!("{}: {e}", path.display()))?;
    info!("wallpaper: now {} (saved to {})", describe(&choice), path.display());
    let _ = app.emit("kompact-wallpaper-changed", state.clone());
    // 14d: the login screen's copy follows a blurred-desktop look.
    crate::users::publish_soon();
    Ok(state)
}

#[tauri::command]
pub fn wallpaper_state(w: State<'_, Wallpapers>) -> Value {
    w.state()
}

/// The pictures to offer (scanned again each time the pane asks); `/preview/<index>` shows one.
#[tauri::command]
pub fn wallpaper_pictures(w: State<'_, Wallpapers>) -> Vec<Value> {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let dirs = picture_dirs(home.as_deref(), &crate::apps::data_dirs(|k| std::env::var(k).ok()));
    let list = scan(&dirs);
    info!("wallpaper: {} pictures in {} folders", list.len(), dirs.iter().filter(|d| d.is_dir()).count());
    let out = list
        .iter()
        .enumerate()
        .map(|(i, p)| {
            let name = p.file_stem().and_then(|s| s.to_str()).unwrap_or("").replace(['_', '-'], " ");
            json!({"index": i, "name": name, "path": p.display().to_string()})
        })
        .collect();
    w.lock().pictures = list;
    out
}

/// `kind`: `gradient`, `color` (value `#rrggbb`) or `image` (value a path).
#[tauri::command]
pub fn set_wallpaper(app: AppHandle, w: State<'_, Wallpapers>, kind: String, value: Option<String>) -> Result<Value, String> {
    let choice = match (kind.as_str(), value) {
        ("gradient", _) => Choice::Gradient,
        ("color", Some(c)) if valid_color(&c) => Choice::Color(c.to_ascii_lowercase()),
        ("image", Some(p)) if Path::new(&p).is_absolute() => Choice::Image(PathBuf::from(p)),
        _ => return Err("not a wallpaper".into()),
    };
    apply(&app, &w, choice)
}

/// "Choose a Picture…": GTK's file chooser; the picture picked becomes the wallpaper.
#[tauri::command]
pub fn choose_wallpaper(app: AppHandle) {
    let handle = app.clone();
    let _ = app.run_on_main_thread(move || {
        use gtk::prelude::*;
        let dialog = gtk::FileChooserDialog::with_buttons(
            Some("Choose a Picture"),
            None::<&gtk::Window>,
            gtk::FileChooserAction::Open,
            &[("Cancel", gtk::ResponseType::Cancel), ("Choose", gtk::ResponseType::Accept)],
        );
        let filter = gtk::FileFilter::new();
        filter.set_name(Some("Pictures"));
        for ext in ["png", "jpg", "jpeg", "webp", "gif", "svg", "bmp"] {
            filter.add_pattern(&format!("*.{ext}"));
            filter.add_pattern(&format!("*.{}", ext.to_ascii_uppercase()));
        }
        dialog.add_filter(filter);
        if let Some(home) = std::env::var_os("HOME") {
            let _ = dialog.set_current_folder(PathBuf::from(home).join("Pictures"));
        }
        dialog.connect_response(move |d, response| {
            if response == gtk::ResponseType::Accept {
                if let Some(path) = d.filename() {
                    use tauri::Manager;
                    if let Err(e) = apply(&handle, &handle.state::<Wallpapers>(), Choice::Image(path)) {
                        warn!("wallpaper: {e}");
                        let _ = handle.emit("kompact-wallpaper-error", e);
                    }
                }
            }
            d.close();
        });
        info!("wallpaper: file chooser open");
        dialog.show_all();
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_round_trip() {
        for c in [Choice::Gradient, Choice::Color("#1e90ff".into()), Choice::Image("/usr/share/backgrounds/a.png".into())] {
            assert_eq!(parse(&to_text(&c)), Some(c));
        }
        assert_eq!(parse(""), None);
        assert_eq!(parse("kind=color\ncolor=blue"), None);
        assert_eq!(parse("kind=color\ncolor=#ABCDEF"), Some(Choice::Color("#abcdef".into())));
        assert_eq!(parse("kind=image\nimage=relative.png"), None);
        assert_eq!(parse("kind=image\nimage=/x/notes.txt"), None);
        assert_eq!(parse("kind=sparkles"), None);
    }

    #[test]
    fn colours() {
        assert!(valid_color("#000000"));
        assert!(valid_color("#a1B2c3"));
        assert!(!valid_color("000000"));
        assert!(!valid_color("#00000"));
        assert!(!valid_color("#00000g"));
    }

    #[test]
    fn what_to_start_with() {
        let file = to_text(&Choice::Color("#112233".into()));
        // The variable wins and locks it.
        assert_eq!(
            initial(Some("/p/x.png"), Some(&file), None),
            Start { choice: Choice::Image("/p/x.png".into()), locked: true, off: false }
        );
        assert_eq!(initial(Some("none"), Some(&file), None), Start { choice: Choice::Gradient, locked: true, off: true });
        // Else the file, else the default picture, else the gradient.
        assert_eq!(initial(None, Some(&file), Some("/d.png")).choice, Choice::Color("#112233".into()));
        assert_eq!(initial(Some(" "), None, Some("/d.png")), Start { choice: Choice::Image("/d.png".into()), locked: false, off: false });
        assert_eq!(initial(None, Some("junk"), None).choice, Choice::Gradient);
    }

    #[test]
    fn picture_folders_and_scan() {
        let dirs = picture_dirs(Some(Path::new("/h")), &[PathBuf::from("/usr/share")]);
        assert_eq!(dirs, [PathBuf::from("/usr/share/backgrounds"), PathBuf::from("/h/Pictures"), PathBuf::from("/h/Pictures/Wallpapers")]);
        let tmp = std::env::temp_dir().join(format!("kompact-wp-{}", std::process::id()));
        std::fs::create_dir_all(tmp.join("sub")).unwrap();
        for f in ["b.png", "a.JPG", "notes.txt", "sub/c.png"] {
            std::fs::write(tmp.join(f), b"x").unwrap();
        }
        let found = scan(&[tmp.clone(), tmp.clone(), tmp.join("missing")]);
        assert_eq!(found, [tmp.join("a.JPG"), tmp.join("b.png")]);
        std::fs::remove_dir_all(&tmp).unwrap();
    }
}
