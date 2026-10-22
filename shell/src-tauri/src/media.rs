//! 18a: Now Playing in the Control Center, from the apps' MPRIS players on the session bus
//! (`org.mpris.MediaPlayer2.*`: Firefox for YouTube, mpv, Spotify, a Bluetooth phone through
//! BlueZ's `mpris-proxy`). The card shows the player that's playing (else the last one paused),
//! its title, artist and cover, and ⏮ ⏯ ⏭.
//!
//! Covers: a `file://` one (Firefox writes its covers to a file) is served as
//! `kompact-art://localhost/<n>` from the list of covers the players named last, never any other
//! file; an `https://` one is passed to the page as it is.
//!
//! The players' `PropertiesChanged` and players coming and going (`NameOwnerChanged`) become
//! `kompact-media-changed` (at most every 700 ms), and the page fetches again.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::Duration;

use serde::Serialize;
use tauri::{AppHandle, Emitter};
use tracing::{info, warn};
use zbus::zvariant::OwnedValue;

pub const ART_SCHEME: &str = "kompact-art";
const PREFIX: &str = "org.mpris.MediaPlayer2.";
const PATH: &str = "/org/mpris/MediaPlayer2";

// ---- The pure part ----

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Player {
    /// Its bus name, to send it ⏯ etc.
    pub bus: String,
    /// What it calls itself ("Mozilla Firefox"), else from its bus name.
    pub name: String,
    /// "Playing", "Paused" or "Stopped".
    pub status: String,
    pub title: String,
    pub artist: String,
    /// For the page: a `kompact-art://` or `https://` URL.
    pub art: Option<String>,
    pub can_previous: bool,
    pub can_next: bool,
    pub can_play_pause: bool,
}

/// A name from the bus name: `org.mpris.MediaPlayer2.firefox.instance_1_42` → "firefox".
pub fn name_from_bus(bus: &str) -> String {
    let rest = bus.strip_prefix(PREFIX).unwrap_or(bus);
    rest.split('.').next().filter(|s| !s.is_empty()).unwrap_or("Media").to_string()
}

/// Where a player's cover is.
#[derive(Debug, PartialEq)]
pub enum Art {
    File(PathBuf),
    Web(String),
    None,
}

pub fn art_source(url: &str) -> Art {
    if let Some(path) = url.strip_prefix("file://") {
        match crate::config::percent_decode(path) {
            Some(p) if p.starts_with('/') => Art::File(PathBuf::from(p)),
            _ => Art::None,
        }
    } else if url.starts_with("https://") {
        Art::Web(url.to_string())
    } else {
        Art::None
    }
}

/// The players in the order shown: playing ones first, then paused, then the rest; one with
/// nothing to show (no title, stopped) is left out. Ties keep the bus's order.
pub fn order(mut players: Vec<Player>) -> Vec<Player> {
    players.retain(|p| !(p.title.is_empty() && p.status == "Stopped"));
    let rank = |p: &Player| match p.status.as_str() {
        "Playing" => 0,
        "Paused" => 1,
        _ => 2,
    };
    players.sort_by_key(rank);
    players
}

// ---- The session bus ----

static CONN: Mutex<Option<zbus::Connection>> = Mutex::new(None);

async fn bus() -> Result<zbus::Connection, String> {
    if let Some(c) = CONN.lock().unwrap_or_else(|e| e.into_inner()).clone() {
        return Ok(c);
    }
    let c = zbus::Connection::session().await.map_err(|e| format!("session bus: {e}"))?;
    *CONN.lock().unwrap_or_else(|e| e.into_inner()) = Some(c.clone());
    Ok(c)
}

async fn proxy(bus_name: &str, iface: &str) -> Result<zbus::Proxy<'static>, String> {
    let c = bus().await?;
    zbus::proxy::Builder::<zbus::Proxy>::new(&c)
        .destination(bus_name.to_string())
        .and_then(|b| b.path(PATH))
        .and_then(|b| b.interface(iface.to_string()))
        .map_err(|e| e.to_string())?
        .cache_properties(zbus::proxy::CacheProperties::No)
        .build()
        .await
        .map_err(|e| format!("{bus_name}: {e}"))
}

/// The covers named last, by their number in `kompact-art://localhost/<n>`.
static ART: Mutex<Vec<PathBuf>> = Mutex::new(Vec::new());

/// The cover file for `kompact-art://localhost/<n>`, if a player named it.
pub fn art_file(path: &str) -> Option<PathBuf> {
    let n: usize = path.trim_start_matches('/').parse().ok()?;
    ART.lock().unwrap_or_else(|e| e.into_inner()).get(n).cloned()
}

fn meta_str(meta: &HashMap<String, OwnedValue>, key: &str) -> String {
    meta.get(key).and_then(|v| String::try_from(v.try_clone().ok()?).ok()).unwrap_or_default()
}

fn meta_list(meta: &HashMap<String, OwnedValue>, key: &str) -> Vec<String> {
    meta.get(key).and_then(|v| Vec::<String>::try_from(v.try_clone().ok()?).ok()).unwrap_or_default()
}

async fn read_player(bus_name: &str, art: &mut Vec<PathBuf>) -> Option<Player> {
    let p = proxy(bus_name, "org.mpris.MediaPlayer2.Player").await.ok()?;
    let status: String = p.get_property("PlaybackStatus").await.ok()?;
    let meta: HashMap<String, OwnedValue> = p.get_property("Metadata").await.unwrap_or_default();
    let flag = |name: &'static str| {
        let p = &p;
        async move { p.get_property::<bool>(name).await.unwrap_or(false) }
    };
    let (can_previous, can_next, can_play, can_pause) = (flag("CanGoPrevious").await, flag("CanGoNext").await, flag("CanPlay").await, flag("CanPause").await);
    let name = match proxy(bus_name, "org.mpris.MediaPlayer2").await {
        Ok(root) => root.get_property::<String>("Identity").await.ok().filter(|s| !s.is_empty()),
        Err(_) => None,
    }
    .unwrap_or_else(|| name_from_bus(bus_name));
    let art_url = match art_source(&meta_str(&meta, "mpris:artUrl")) {
        Art::File(path) => {
            art.push(path);
            // The number changes with the list; the length makes the URL new when it does.
            Some(format!("{ART_SCHEME}://localhost/{}?v={}", art.len() - 1, meta_str(&meta, "mpris:artUrl").len()))
        }
        Art::Web(url) => Some(url),
        Art::None => None,
    };
    Some(Player {
        bus: bus_name.to_string(),
        name,
        status,
        title: meta_str(&meta, "xesam:title"),
        artist: meta_list(&meta, "xesam:artist").join(", "),
        art: art_url,
        can_previous,
        can_next,
        can_play_pause: can_play || can_pause,
    })
}

async fn players() -> Result<Vec<Player>, String> {
    let c = bus().await?;
    let dbus = zbus::fdo::DBusProxy::new(&c).await.map_err(|e| e.to_string())?;
    let names = dbus.list_names().await.map_err(|e| e.to_string())?;
    let mut art = Vec::new();
    let mut out = Vec::new();
    for n in names.iter().map(|n| n.as_str()).filter(|n| n.starts_with(PREFIX)) {
        if let Some(p) = read_player(n, &mut art).await {
            out.push(p);
        }
    }
    *ART.lock().unwrap_or_else(|e| e.into_inner()) = art;
    Ok(order(out))
}

#[tauri::command]
pub async fn media_state() -> Result<Vec<Player>, String> {
    players().await
}

/// ⏮ ⏯ ⏭: `action` is `previous`, `play-pause` or `next`.
#[tauri::command]
pub async fn media_control(bus: String, action: String) -> Result<(), String> {
    if !bus.starts_with(PREFIX) {
        return Err(format!("{bus} isn't a media player"));
    }
    let method = match action.as_str() {
        "previous" => "Previous",
        "play-pause" => "PlayPause",
        "next" => "Next",
        _ => return Err(format!("unknown media action {action:?}")),
    };
    let p = proxy(&bus, "org.mpris.MediaPlayer2.Player").await?;
    p.call_method(method, &()).await.map_err(|e| format!("{method}: {e}"))?;
    info!("media: {method} → {}", name_from_bus(&bus));
    Ok(())
}

/// The players' changes → `kompact-media-changed` (at most every 700 ms).
pub fn start(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        use futures_util::StreamExt;
        let conn = match bus().await {
            Ok(c) => c,
            Err(e) => return warn!("media: {e}; no Now Playing"),
        };
        let changed = zbus::MatchRule::builder()
            .msg_type(zbus::message::Type::Signal)
            .interface("org.freedesktop.DBus.Properties")
            .and_then(|b| b.member("PropertiesChanged"))
            .and_then(|b| b.path(PATH))
            .map(|b| b.build());
        let owners = zbus::MatchRule::builder()
            .msg_type(zbus::message::Type::Signal)
            .sender("org.freedesktop.DBus")
            .and_then(|b| b.member("NameOwnerChanged"))
            .and_then(|b| b.arg0ns("org.mpris.MediaPlayer2"))
            .map(|b| b.build());
        let (Ok(changed), Ok(owners)) = (changed, owners) else { return warn!("media: bad match rule") };
        let (Ok(a), Ok(b)) = (
            zbus::MessageStream::for_match_rule(changed, &conn, None).await,
            zbus::MessageStream::for_match_rule(owners, &conn, None).await,
        ) else {
            return warn!("media: can't watch the media players");
        };
        info!("media: watching MPRIS players");
        let mut stream = futures_util::stream::select(a, b);
        let mut last = std::time::Instant::now() - Duration::from_secs(1);
        let pending = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        while stream.next().await.is_some() {
            if last.elapsed() >= Duration::from_millis(700) {
                last = std::time::Instant::now();
                let _ = app.emit("kompact-media-changed", ());
            } else if !pending.swap(true, std::sync::atomic::Ordering::Relaxed) {
                // One more after the burst, so the last change isn't lost.
                let (app, pending) = (app.clone(), pending.clone());
                tauri::async_runtime::spawn(async move {
                    tokio::time::sleep(Duration::from_millis(700)).await;
                    pending.store(false, std::sync::atomic::Ordering::Relaxed);
                    let _ = app.emit("kompact-media-changed", ());
                });
            }
        }
        warn!("media: the session bus's signals stopped");
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn player(bus: &str, status: &str, title: &str) -> Player {
        Player {
            bus: bus.into(),
            name: name_from_bus(bus),
            status: status.into(),
            title: title.into(),
            artist: String::new(),
            art: None,
            can_previous: true,
            can_next: true,
            can_play_pause: true,
        }
    }

    /// Read-only, against this session's players: `cargo test media_live -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn media_live() {
        let list = tauri::async_runtime::block_on(players()).expect("media players");
        println!("{}", serde_json::to_string_pretty(&list).unwrap());
    }

    #[test]
    fn names_from_buses() {
        assert_eq!(name_from_bus("org.mpris.MediaPlayer2.firefox.instance_1_42"), "firefox");
        assert_eq!(name_from_bus("org.mpris.MediaPlayer2.mpv"), "mpv");
        assert_eq!(name_from_bus("org.mpris.MediaPlayer2."), "Media");
    }

    #[test]
    fn covers() {
        assert_eq!(art_source("file:///tmp/firefox-mpris/cover%201.png"), Art::File("/tmp/firefox-mpris/cover 1.png".into()));
        assert_eq!(art_source("https://i.ytimg.com/vi/x/hq.jpg"), Art::Web("https://i.ytimg.com/vi/x/hq.jpg".into()));
        // Plain http, relative files and other schemes aren't shown.
        assert_eq!(art_source("http://example.com/a.jpg"), Art::None);
        assert_eq!(art_source("file://relative.png"), Art::None);
        assert_eq!(art_source(""), Art::None);
    }

    #[test]
    fn art_only_from_the_list() {
        *ART.lock().unwrap() = vec!["/tmp/cover.png".into()];
        assert_eq!(art_file("/0"), Some(PathBuf::from("/tmp/cover.png")));
        assert_eq!(art_file("/1"), None);
        assert_eq!(art_file("/../../etc/passwd"), None);
    }

    #[test]
    fn playing_first() {
        let list = order(vec![
            player("org.mpris.MediaPlayer2.a", "Paused", "Song A"),
            player("org.mpris.MediaPlayer2.b", "Stopped", ""),
            player("org.mpris.MediaPlayer2.c", "Playing", "Video C"),
            player("org.mpris.MediaPlayer2.d", "Stopped", "Old D"),
        ]);
        let titles: Vec<&str> = list.iter().map(|p| p.title.as_str()).collect();
        assert_eq!(titles, ["Video C", "Song A", "Old D"], "stopped with nothing to show is left out");
    }
}
