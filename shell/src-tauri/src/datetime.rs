//! 12g: System Settings → Date & Time, and how the menu bar's clock looks.
//!
//! The time zone and automatic time (NTP) are systemd-timedated's (`org.freedesktop.timedate1` on
//! the system bus); changing them asks polkit, so the password dialog (12e) may show. The clock's
//! look is ours, kept in `~/.config/kompact/clock` (`24-hour`, `weekday`, `date`, `seconds`).
//! Both go to the pages as `kompact-clock` ({zone, prefs}); the clock formats in that zone
//! explicitly, since WebKit keeps the zone it started with when the system's changes.

use std::path::PathBuf;

use serde::Serialize;
use serde_json::{json, Value};
use tauri::{AppHandle, Emitter};
use tracing::{info, warn};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct ClockPrefs {
    /// 13:05, not 1:05 PM.
    pub hour24: bool,
    pub weekday: bool,
    /// "8 Oct".
    pub date: bool,
    pub seconds: bool,
}

impl Default for ClockPrefs {
    /// What the clock always showed: "Thu 8 Oct 22:30" (the locale's hour style; 24-hour here).
    fn default() -> Self {
        ClockPrefs { hour24: true, weekday: true, date: true, seconds: false }
    }
}

impl ClockPrefs {
    pub fn parse(text: &str) -> ClockPrefs {
        let mut p = ClockPrefs::default();
        for line in text.lines().map(str::trim).filter(|l| !l.starts_with('#')) {
            let Some((k, v)) = line.split_once('=') else { continue };
            let on = match v.trim() {
                "on" => true,
                "off" => false,
                _ => continue,
            };
            match k.trim() {
                "24-hour" => p.hour24 = on,
                "weekday" => p.weekday = on,
                "date" => p.date = on,
                "seconds" => p.seconds = on,
                _ => {}
            }
        }
        p
    }

    pub fn to_text(&self) -> String {
        let w = |b: bool| if b { "on" } else { "off" };
        format!(
            "# kompact menu-bar clock (written by System Settings → Date & Time)\n24-hour={}\nweekday={}\ndate={}\nseconds={}\n",
            w(self.hour24),
            w(self.weekday),
            w(self.date),
            w(self.seconds)
        )
    }
}

/// A time zone name timedated could know: `Area/City`, `UTC`; letters, digits, `/_+-`.
pub fn valid_zone(z: &str) -> bool {
    !z.is_empty() && z.len() <= 64 && !z.starts_with('/') && !z.contains("..") && z.bytes().all(|b| b.is_ascii_alphanumeric() || b"/_+-".contains(&b))
}

fn prefs_path() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
    Some(base.join("kompact").join("clock"))
}

fn load_prefs() -> ClockPrefs {
    prefs_path().and_then(|p| std::fs::read_to_string(p).ok()).map(|t| ClockPrefs::parse(&t)).unwrap_or_default()
}

async fn timedate() -> Result<zbus::Proxy<'static>, String> {
    let c = zbus::Connection::system().await.map_err(|e| format!("system bus: {e}"))?;
    zbus::proxy::Builder::<zbus::Proxy>::new(&c)
        .destination("org.freedesktop.timedate1")
        .and_then(|b| b.path("/org/freedesktop/timedate1"))
        .and_then(|b| b.interface("org.freedesktop.timedate1"))
        .map_err(|e| e.to_string())?
        .cache_properties(zbus::proxy::CacheProperties::No)
        .build()
        .await
        .map_err(|e| format!("timedated: {e}"))
}

/// The zone, automatic time and the clock's look.
#[tauri::command]
pub async fn datetime_state() -> Result<Value, String> {
    let t = timedate().await?;
    let zone: String = t.get_property("Timezone").await.unwrap_or_default();
    let ntp: bool = t.get_property("NTP").await.unwrap_or(false);
    let can_ntp: bool = t.get_property("CanNTP").await.unwrap_or(false);
    let synced: bool = t.get_property("NTPSynchronized").await.unwrap_or(false);
    Ok(json!({"zone": zone, "ntp": ntp, "can_ntp": can_ntp, "synced": synced, "clock": load_prefs()}))
}

async fn tell(app: &AppHandle) {
    match datetime_state().await {
        Ok(state) => {
            let _ = app.emit("kompact-clock", state);
        }
        Err(e) => warn!("date & time: {e}"),
    }
}

/// Every zone timedated knows (about 480).
#[tauri::command]
pub async fn datetime_zones() -> Result<Vec<String>, String> {
    let t = timedate().await?;
    t.call("ListTimezones", &()).await.map_err(|e| format!("timedated: {e}"))
}

/// Polkit asks first (interactive), with the password dialog if it wants one.
#[tauri::command]
pub async fn datetime_set_zone(app: AppHandle, zone: String) -> Result<(), String> {
    if !valid_zone(&zone) {
        return Err(format!("{zone:?} isn't a time zone"));
    }
    let t = timedate().await?;
    t.call::<_, _, ()>("SetTimezone", &(zone.as_str(), true)).await.map_err(|e| format!("Couldn't set the time zone: {}", short(&e)))?;
    info!("date & time: time zone now {zone}");
    tell(&app).await;
    Ok(())
}

#[tauri::command]
pub async fn datetime_set_ntp(app: AppHandle, on: bool) -> Result<(), String> {
    let t = timedate().await?;
    t.call::<_, _, ()>("SetNTP", &(on, true)).await.map_err(|e| format!("Couldn't change automatic time: {}", short(&e)))?;
    info!("date & time: automatic time {}", if on { "on" } else { "off" });
    tell(&app).await;
    Ok(())
}

#[tauri::command]
pub async fn set_clock_prefs(app: AppHandle, hour24: bool, weekday: bool, date: bool, seconds: bool) -> Result<(), String> {
    let p = ClockPrefs { hour24, weekday, date, seconds };
    let path = prefs_path().ok_or("no home directory")?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    std::fs::write(&path, p.to_text()).map_err(|e| format!("{}: {e}", path.display()))?;
    info!("date & time: clock {p:?}");
    tell(&app).await;
    Ok(())
}

fn short(e: &zbus::Error) -> String {
    match e {
        zbus::Error::MethodError(name, msg, _) if name.as_str().ends_with("InteractiveAuthorizationRequired") || name.as_str().contains("AccessDenied") => {
            msg.clone().unwrap_or_else(|| "not allowed".into())
        }
        zbus::Error::MethodError(_, Some(msg), _) => msg.clone(),
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clock_settings() {
        assert_eq!(ClockPrefs::parse(""), ClockPrefs::default());
        let p = ClockPrefs { hour24: false, weekday: false, date: true, seconds: true };
        assert_eq!(ClockPrefs::parse(&p.to_text()), p);
        assert_eq!(ClockPrefs::parse("seconds=maybe\nweekday=off").weekday, false);
    }

    #[test]
    fn zone_names() {
        assert!(valid_zone("Asia/Kolkata"));
        assert!(valid_zone("America/Argentina/Buenos_Aires"));
        assert!(valid_zone("Etc/GMT+5"));
        assert!(valid_zone("UTC"));
        assert!(!valid_zone("../etc/passwd"));
        assert!(!valid_zone("/Asia/Kolkata"));
        assert!(!valid_zone("Asia Kolkata"));
        assert!(!valid_zone(""));
    }
}
