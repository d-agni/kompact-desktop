//! 14b: System Settings → Users & Login. Your name and picture are AccountsService's
//! (`org.freedesktop.Accounts` on the system bus; polkit lets you change your own without a
//! password, and `SetIconFile` copies the picture to `/var/lib/AccountsService/icons/`, where the
//! login page (14c) can read it). How the lock page looks is `login_prefs` in
//! `~/.config/kompact/login`, read by `kompact-lock` each time it locks.

use std::path::{Path, PathBuf};

use serde_json::{json, Value};
use tauri::{AppHandle, Emitter, Manager};
use tracing::{info, warn};

use crate::login_prefs::{self, Background, LoginPrefs};
use crate::overlay::Overlay;

/// AccountsService refuses pictures over 1 MiB.
const MAX_PICTURE: u64 = 1024 * 1024;
/// Longest full name taken.
const MAX_NAME: usize = 64;

/// `kompact-icon://localhost/<key>` names for the page.
const PICTURE_KEY: &str = "user-picture";
const LOGIN_IMAGE_KEY: &str = "user-login-image";

fn config_home() -> Option<PathBuf> {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
}

fn load_prefs() -> LoginPrefs {
    config_home().and_then(|c| std::fs::read_to_string(login_prefs::path(&c)).ok()).map(|t| LoginPrefs::parse(&t)).unwrap_or_default()
}

fn save_prefs(p: &LoginPrefs) -> Result<PathBuf, String> {
    let path = login_prefs::path(&config_home().ok_or("no home directory")?);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    std::fs::write(&path, p.to_text()).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(path)
}

/// A name AccountsService may take: one line, not empty, at most `MAX_NAME` characters.
pub fn valid_name(name: &str) -> Option<String> {
    let n = name.trim();
    (!n.is_empty() && n.chars().count() <= MAX_NAME && !n.contains(|c: char| c.is_control())).then(|| n.to_string())
}

async fn user_proxy() -> Result<zbus::Proxy<'static>, String> {
    let conn = zbus::Connection::system().await.map_err(|e| format!("system bus: {e}"))?;
    let accounts = zbus::proxy::Builder::<zbus::Proxy>::new(&conn)
        .destination("org.freedesktop.Accounts")
        .and_then(|b| b.path("/org/freedesktop/Accounts"))
        .and_then(|b| b.interface("org.freedesktop.Accounts"))
        .map_err(|e| e.to_string())?
        .build()
        .await
        .map_err(|e| format!("AccountsService: {e}"))?;
    // SAFETY: plain query.
    let uid = unsafe { libc::getuid() } as i64;
    let path: zbus::zvariant::OwnedObjectPath = accounts.call("FindUserById", &(uid,)).await.map_err(|e| format!("AccountsService doesn't know you: {e}"))?;
    zbus::proxy::Builder::<zbus::Proxy>::new(&conn)
        .destination("org.freedesktop.Accounts")
        .and_then(|b| b.path(path))
        .and_then(|b| b.interface("org.freedesktop.Accounts.User"))
        .map_err(|e| e.to_string())?
        .cache_properties(zbus::proxy::CacheProperties::No)
        .build()
        .await
        .map_err(|e| format!("AccountsService: {e}"))
}

fn image_like(p: &Path) -> bool {
    p.is_file() && std::fs::read(p).ok().is_some_and(|b| crate::config::sniff_mime(&b[..b.len().min(512)]).is_some())
}

/// Your login name, full name, picture and the lock page's look.
#[tauri::command]
pub async fn users_state(app: AppHandle) -> Result<Value, String> {
    let user = user_proxy().await?;
    let login: String = user.get_property("UserName").await.unwrap_or_default();
    let name: String = user.get_property("RealName").await.unwrap_or_default();
    let icon: String = user.get_property("IconFile").await.unwrap_or_default();
    // AccountsService's copy first (what it's set to may not exist, as `~/.face` here).
    let picture = [PathBuf::from(format!("/var/lib/AccountsService/icons/{login}")), PathBuf::from(&icon)].into_iter().find(|p| image_like(p));
    let overlay = app.state::<Overlay>();
    match &picture {
        Some(p) => overlay.add_icon(PICTURE_KEY.into(), p.clone()),
        None => overlay.remove_icon(PICTURE_KEY),
    }
    let prefs = load_prefs();
    let image = prefs.image.clone().filter(|p| p.is_file());
    match &image {
        Some(p) => overlay.add_icon(LOGIN_IMAGE_KEY.into(), p.clone()),
        None => overlay.remove_icon(LOGIN_IMAGE_KEY),
    }
    Ok(json!({
        "login": login,
        "name": if name.is_empty() { login.clone() } else { name },
        "picture": picture.is_some(),
        "background": prefs.background.name(),
        "image": image.map(|p| p.display().to_string()),
        "message": prefs.message,
    }))
}

fn changed(app: &AppHandle) {
    let _ = app.emit("kompact-users-changed", ());
    publish_soon();
}

/// 14d: where each user's look is published for the login screen (made by
/// `kompact/tools/install-login.sh`, sticky like /tmp: everyone may add their own files there).
/// The greeter only believes files the user owns.
pub const SHARED_DIR: &str = "/var/lib/kompact/login";

/// Largest picture copied for the login screen.
const MAX_PUBLISHED: u64 = 16 * 1024 * 1024;

/// What to publish: the conf for the greeter (`image=` the copy in `dir`, `background=` blurred
/// or not) and which file to copy where. Pictures the greeter can't show aren't copied.
pub fn published(prefs: &LoginPrefs, desktop: Option<PathBuf>, dir: &Path, login: &str, exists: impl Fn(&Path) -> bool) -> (LoginPrefs, Option<(PathBuf, PathBuf)>) {
    let (src, blur) = prefs.backdrop(desktop, &exists);
    let copy = src.and_then(|s| {
        let ext = s.extension()?.to_str()?.to_ascii_lowercase();
        let ext = if ext == "jpeg" { "jpg".to_string() } else { ext };
        ["png", "jpg", "webp"].contains(&ext.as_str()).then(|| (s.clone(), dir.join(format!("{login}.{ext}"))))
    });
    let out = LoginPrefs {
        background: if blur { Background::DesktopBlur } else { Background::Desktop },
        image: copy.as_ref().map(|(_, d)| d.clone()),
        message: prefs.message.clone(),
    };
    (out, copy)
}

fn login_name() -> Option<String> {
    // SAFETY: getpwuid's result is read at once.
    unsafe {
        let pw = libc::getpwuid(libc::getuid());
        (!pw.is_null() && !(*pw).pw_name.is_null()).then(|| std::ffi::CStr::from_ptr((*pw).pw_name).to_string_lossy().into_owned())
    }
}

/// Write `bytes` to `path` whole: a temporary file next to it, then a rename; readable by all.
fn write_whole(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let tmp = path.with_extension("kompact-tmp");
    std::fs::write(&tmp, bytes)?;
    std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o644))?;
    std::fs::rename(&tmp, path)
}

/// 14d: copy this user's lock-screen look to the shared folder for the login screen. Nothing
/// if the folder isn't there (not installed). Runs in the background (a picture can be large).
pub fn publish_soon() {
    std::thread::spawn(|| {
        if let Err(e) = publish() {
            warn!("users: login screen look not published: {e}");
        }
    });
}

fn publish() -> Result<(), String> {
    let dir = std::env::var_os("KOMPACT_LOGIN_SHARED").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(SHARED_DIR));
    if !dir.is_dir() {
        return Ok(());
    }
    let login = login_name().ok_or("can't tell who you are")?;
    let (conf, copy) = published(&load_prefs(), crate::wallpaper::current_image(), &dir, &login, |p| p.is_file());
    if let Some((src, dst)) = &copy {
        let size = std::fs::metadata(src).map_err(|e| format!("{}: {e}", src.display()))?.len();
        if size > MAX_PUBLISHED {
            return Err(format!("{} is {} MB; the login screen takes up to 16 MB", src.display(), size / (1024 * 1024)));
        }
        let bytes = std::fs::read(src).map_err(|e| format!("{}: {e}", src.display()))?;
        write_whole(dst, &bytes).map_err(|e| format!("{}: {e}", dst.display()))?;
    }
    // Earlier copies in another format go.
    for ext in ["png", "jpg", "webp"] {
        let old = dir.join(format!("{login}.{ext}"));
        if copy.as_ref().map(|(_, d)| d) != Some(&old) && old.exists() {
            let _ = std::fs::remove_file(&old);
        }
    }
    let path = dir.join(format!("{login}.conf"));
    write_whole(&path, conf.to_text().as_bytes()).map_err(|e| format!("{}: {e}", path.display()))?;
    info!("users: login screen look published to {} ({}{})", path.display(), conf.background.name(), if copy.is_some() { ", with its picture" } else { ", no picture" });
    Ok(())
}

#[tauri::command]
pub async fn set_real_name(app: AppHandle, name: String) -> Result<(), String> {
    let name = valid_name(&name).ok_or("A name needs 1 to 64 characters on one line.")?;
    let user = user_proxy().await?;
    user.call::<_, _, ()>("SetRealName", &(name.as_str(),)).await.map_err(|e| format!("Couldn't change your name: {e}"))?;
    info!("users: full name now {name:?}");
    changed(&app);
    Ok(())
}

/// The lock page's background (`desktop-blur`, `desktop`, `image`) and message.
#[tauri::command]
pub fn set_login_prefs(app: AppHandle, background: String, message: String) -> Result<(), String> {
    let mut p = load_prefs();
    p.background = Background::parse(&background).ok_or_else(|| format!("{background:?} isn't a background"))?;
    p.message = login_prefs::clean_message(&message);
    if p.background == Background::Image && p.image.is_none() {
        return Err("Choose a picture first.".into());
    }
    let path = save_prefs(&p)?;
    info!("users: lock page {} ({} message; saved to {})", p.background.name(), if p.message.is_empty() { "no" } else { "a" }, path.display());
    changed(&app);
    Ok(())
}

/// GTK's file chooser for a picture; `then` gets the file chosen.
fn choose_image(app: &AppHandle, title: &'static str, then: impl Fn(&AppHandle, PathBuf) + Send + 'static) {
    let handle = app.clone();
    let _ = app.run_on_main_thread(move || {
        use gtk::prelude::*;
        let dialog = gtk::FileChooserDialog::with_buttons(
            Some(title),
            None::<&gtk::Window>,
            gtk::FileChooserAction::Open,
            &[("Cancel", gtk::ResponseType::Cancel), ("Choose", gtk::ResponseType::Accept)],
        );
        let filter = gtk::FileFilter::new();
        filter.set_name(Some("Pictures"));
        for m in ["image/png", "image/jpeg", "image/webp", "image/gif", "image/bmp"] {
            filter.add_mime_type(m);
        }
        dialog.add_filter(filter);
        if let Some(pics) = std::env::var_os("HOME").map(|h| PathBuf::from(h).join("Pictures")).filter(|p| p.is_dir()) {
            let _ = dialog.set_current_folder(pics);
        }
        dialog.connect_response(move |d, response| {
            if response == gtk::ResponseType::Accept {
                if let Some(path) = d.filename() {
                    then(&handle, path);
                }
            }
            d.close();
        });
        dialog.show_all();
    });
}

/// "Edit…" under the picture: AccountsService copies the file for every page that shows you.
#[tauri::command]
pub fn choose_user_picture(app: AppHandle) {
    choose_image(&app, "Choose Your Picture", |app, path| {
        let app = app.clone();
        tauri::async_runtime::spawn(async move {
            let result = async {
                let size = std::fs::metadata(&path).map_err(|e| format!("{}: {e}", path.display()))?.len();
                if size > MAX_PICTURE {
                    return Err(format!("That picture is {} KB; it can be 1024 KB at most.", size / 1024));
                }
                let user = user_proxy().await?;
                let p = path.display().to_string();
                user.call::<_, _, ()>("SetIconFile", &(p.as_str(),)).await.map_err(|e| format!("Couldn't set your picture: {e}"))
            }
            .await;
            match result {
                Ok(()) => info!("users: picture now {}", path.display()),
                Err(e) => {
                    warn!("users: {e}");
                    let _ = app.emit("kompact-users-error", e);
                }
            }
            changed(&app);
        });
    });
}

/// "Choose…" for a lock-page picture of its own.
#[tauri::command]
pub fn choose_login_image(app: AppHandle) {
    choose_image(&app, "Choose a Lock Screen Picture", |app, path| {
        let mut p = load_prefs();
        p.background = Background::Image;
        p.image = Some(path.clone());
        match save_prefs(&p) {
            Ok(saved) => info!("users: lock page picture {} (saved to {})", path.display(), saved.display()),
            Err(e) => {
                warn!("users: {e}");
                let _ = app.emit("kompact-users-error", e);
            }
        }
        changed(app);
    });
}

/// "Show Lock Screen": lock now, to see it.
#[tauri::command]
pub fn preview_lock(app: AppHandle) -> Result<(), String> {
    app.state::<crate::compositor::Compositor>().send_line_pub("lock", "lock\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn publishing() {
        let dir = Path::new("/var/lib/kompact/login");
        let desk = Some(PathBuf::from("/usr/share/backgrounds/w.JPEG"));
        let mut p = LoginPrefs { message: "hi".into(), ..LoginPrefs::default() };
        let (conf, copy) = published(&p, desk.clone(), dir, "sam", |_| true);
        assert_eq!(copy, Some((PathBuf::from("/usr/share/backgrounds/w.JPEG"), dir.join("sam.jpg"))));
        assert_eq!((conf.background, conf.image.clone(), conf.message.as_str()), (Background::DesktopBlur, Some(dir.join("sam.jpg")), "hi"));
        // Its own picture, sharp.
        p.background = Background::Image;
        p.image = Some("/home/u/me.png".into());
        let (conf, copy) = published(&p, desk.clone(), dir, "sam", |_| true);
        assert_eq!(copy.map(|c| c.1), Some(dir.join("sam.png")));
        assert_eq!(conf.background, Background::Desktop);
        // A gradient desktop: no picture.
        p.background = Background::DesktopBlur;
        let (conf, copy) = published(&p, None, dir, "sam", |_| true);
        assert_eq!((copy, conf.image), (None, None));
        // A picture type the login screen doesn't take.
        let (_, copy) = published(&p, Some("/x/w.bmp".into()), dir, "sam", |_| true);
        assert_eq!(copy, None);
    }

    #[test]
    fn names() {
        assert_eq!(valid_name("  Sam Lee "), Some("Sam Lee".into()));
        assert_eq!(valid_name(""), None);
        assert_eq!(valid_name("a\nb"), None);
        assert_eq!(valid_name(&"é".repeat(64)).map(|n| n.chars().count()), Some(64));
        assert_eq!(valid_name(&"é".repeat(65)), None);
    }
}
