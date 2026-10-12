//! 12a: networks, from NetworkManager on the system bus (zbus, no nmcli): the menu bar's
//! network icon and menu (overlay "status" mode) and System Settings → Network.
//!
//! - `network_summary`: what the menu-bar icon shows (Ethernet, Wi-Fi with 0–3 bars, off, none).
//! - `network_state`: devices (Ethernet, Wi-Fi) with their IP, the networks in range (one per
//!   name, strongest first, the joined one and known ones marked) and the known Wi-Fi networks.
//! - `network_wifi` (on/off), `network_scan`, `network_join` (known: reconnect; new: saved as
//!   *your* connection, `connection.permissions = user:<you>`, which NetworkManager lets the
//!   active session add without an admin password), `network_disconnect`, `network_forget`.
//!
//! NetworkManager's change signals (debounced) become `kompact-network-changed`; pages fetch again.
//! A join reports how it went with `kompact-network-joined` ({ssid, ok, message}); a new network
//! that fails to connect (a wrong password) is removed again, so the next try asks anew.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Duration;

use serde::Serialize;
use serde_json::{json, Value};
use tauri::{AppHandle, Emitter};
use tracing::{info, warn};
use zbus::zvariant::{OwnedObjectPath, OwnedValue, Value as ZValue};

const NM: &str = "org.freedesktop.NetworkManager";
const NM_PATH: &str = "/org/freedesktop/NetworkManager";
const SETTINGS_PATH: &str = "/org/freedesktop/NetworkManager/Settings";

const DEVICE_ETHERNET: u32 = 1;
const DEVICE_WIFI: u32 = 2;
/// NMDeviceState
const STATE_UNAVAILABLE: u32 = 20;
const STATE_DISCONNECTED: u32 = 30;
const STATE_ACTIVATED: u32 = 100;
/// NM80211ApSecurityFlags
const KEY_MGMT_PSK: u32 = 0x100;
const KEY_MGMT_802_1X: u32 = 0x200;
const KEY_MGMT_SAE: u32 = 0x400;
const AP_FLAGS_PRIVACY: u32 = 0x1;

// ---- The pure part ----

/// How a network wants to be joined.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Security {
    Open,
    /// WPA/WPA2 Personal (`wpa-psk`).
    Psk,
    /// WPA3 Personal only (`sae`).
    Sae,
    /// Enterprise (802.1X) or WEP: not offered here.
    Unsupported,
}

pub fn security(flags: u32, wpa: u32, rsn: u32) -> Security {
    let both = wpa | rsn;
    if both & KEY_MGMT_802_1X != 0 {
        Security::Unsupported
    } else if both & KEY_MGMT_PSK != 0 {
        Security::Psk
    } else if both & KEY_MGMT_SAE != 0 {
        Security::Sae
    } else if flags & AP_FLAGS_PRIVACY != 0 {
        Security::Unsupported // WEP
    } else {
        Security::Open
    }
}

/// An access point as NetworkManager reports it.
#[derive(Debug, Clone, PartialEq)]
pub struct Ap {
    pub ssid: String,
    pub strength: u8,
    pub security: Security,
    pub active: bool,
}

/// A network in the list: one per name.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Net {
    pub ssid: String,
    pub strength: u8,
    pub bars: u8,
    pub security: Security,
    pub active: bool,
    pub known: bool,
}

/// 0–3 bars, as the menu bar draws them.
pub fn bars(strength: u8) -> u8 {
    match strength {
        0..=24 => 0,
        25..=49 => 1,
        50..=74 => 2,
        _ => 3,
    }
}

/// SSIDs are bytes; most are UTF-8. Hidden networks (empty) are left out.
pub fn ssid_text(bytes: &[u8]) -> Option<String> {
    let s = String::from_utf8_lossy(bytes).trim_end_matches('\0').to_string();
    (!s.trim().is_empty()).then_some(s)
}

/// One entry per name (the strongest of its access points), the joined one first, then known
/// ones, then by strength.
pub fn network_list(aps: &[Ap], known: &[String]) -> Vec<Net> {
    let mut by_name: Vec<Net> = Vec::new();
    for ap in aps {
        match by_name.iter_mut().find(|n| n.ssid == ap.ssid) {
            Some(n) => {
                n.active |= ap.active;
                if ap.strength > n.strength {
                    n.strength = ap.strength;
                    n.bars = bars(ap.strength);
                    n.security = ap.security;
                }
            }
            None => by_name.push(Net {
                ssid: ap.ssid.clone(),
                strength: ap.strength,
                bars: bars(ap.strength),
                security: ap.security,
                active: ap.active,
                known: known.contains(&ap.ssid),
            }),
        }
    }
    by_name.sort_by(|a, b| b.active.cmp(&a.active).then(b.known.cmp(&a.known)).then(b.strength.cmp(&a.strength)).then(a.ssid.cmp(&b.ssid)));
    by_name
}

/// What the menu-bar icon shows: `ethernet`, `wifi` (with bars), `connecting`, `off` (Wi-Fi
/// switched off, no cable) or `none`.
pub fn icon(ethernet_up: bool, wifi_up: Option<u8>, wifi_enabled: bool, connecting: bool) -> (&'static str, u8) {
    if ethernet_up {
        ("ethernet", 3)
    } else if let Some(strength) = wifi_up {
        ("wifi", bars(strength))
    } else if connecting {
        ("connecting", 0)
    } else if !wifi_enabled {
        ("off", 0)
    } else {
        ("none", 0)
    }
}

/// The key management NetworkManager wants for a new connection, or why it can't be joined.
pub fn key_mgmt(security: Security, password: Option<&str>) -> Result<Option<&'static str>, String> {
    match (security, password.filter(|p| !p.is_empty())) {
        (Security::Open, _) => Ok(None),
        (Security::Psk, Some(p)) if (8..=63).contains(&p.len()) => Ok(Some("wpa-psk")),
        (Security::Sae, Some(p)) if !p.is_empty() => Ok(Some("sae")),
        (Security::Psk, Some(_)) => Err("A Wi-Fi password is 8 to 63 characters.".into()),
        (Security::Psk | Security::Sae, None) => Err("This network needs a password.".into()),
        (Security::Unsupported, _) => Err("This network needs a sign-in (enterprise or WEP), which kompact can't do yet.".into()),
        (Security::Sae, Some(_)) => unreachable!(),
    }
}

// ---- NetworkManager ----

static CONN: Mutex<Option<zbus::Connection>> = Mutex::new(None);

async fn bus() -> Result<zbus::Connection, String> {
    if let Some(c) = CONN.lock().unwrap_or_else(|e| e.into_inner()).clone() {
        return Ok(c);
    }
    let c = zbus::Connection::system().await.map_err(|e| format!("system bus: {e}"))?;
    *CONN.lock().unwrap_or_else(|e| e.into_inner()) = Some(c.clone());
    Ok(c)
}

async fn proxy(path: &str, iface: &str) -> Result<zbus::Proxy<'static>, String> {
    let c = bus().await?;
    zbus::proxy::Builder::<zbus::Proxy>::new(&c)
        .destination(NM)
        .and_then(|b| b.path(path.to_string()))
        .and_then(|b| b.interface(iface.to_string()))
        .map_err(|e| e.to_string())?
        .cache_properties(zbus::proxy::CacheProperties::No)
        .build()
        .await
        .map_err(|e| format!("NetworkManager: {e}"))
}

async fn prop<T>(p: &zbus::Proxy<'_>, name: &str) -> Option<T>
where
    T: TryFrom<OwnedValue>,
    T::Error: Into<zbus::Error>,
{
    p.get_property::<T>(name).await.ok()
}

struct Device {
    path: OwnedObjectPath,
    iface: String,
    kind: u32,
    state: u32,
    ip4: Option<String>,
    connection: Option<String>,
}

async fn devices() -> Result<Vec<Device>, String> {
    let nm = proxy(NM_PATH, NM).await?;
    let paths: Vec<OwnedObjectPath> = nm.call("GetDevices", &()).await.map_err(|e| format!("NetworkManager: {e}"))?;
    let mut out = Vec::new();
    for path in paths {
        let d = proxy(path.as_str(), "org.freedesktop.NetworkManager.Device").await?;
        let kind: u32 = prop(&d, "DeviceType").await.unwrap_or(0);
        if kind != DEVICE_ETHERNET && kind != DEVICE_WIFI {
            continue;
        }
        let state: u32 = prop(&d, "State").await.unwrap_or(0);
        let iface: String = prop(&d, "Interface").await.unwrap_or_default();
        let mut ip4 = None;
        if let Some(cfg) = prop::<OwnedObjectPath>(&d, "Ip4Config").await.filter(|p| p.as_str() != "/") {
            if let Ok(c) = proxy(cfg.as_str(), "org.freedesktop.NetworkManager.IP4Config").await {
                let data: Vec<HashMap<String, OwnedValue>> = prop(&c, "AddressData").await.unwrap_or_default();
                ip4 = data.first().and_then(|a| a.get("address")).and_then(|v| String::try_from(v.clone()).ok());
            }
        }
        let mut connection = None;
        if let Some(ac) = prop::<OwnedObjectPath>(&d, "ActiveConnection").await.filter(|p| p.as_str() != "/") {
            if let Ok(a) = proxy(ac.as_str(), "org.freedesktop.NetworkManager.Connection.Active").await {
                connection = prop::<String>(&a, "Id").await;
            }
        }
        out.push(Device { path, iface, kind, state, ip4, connection });
    }
    Ok(out)
}

async fn access_points(dev: &Device) -> Vec<Ap> {
    let Ok(w) = proxy(dev.path.as_str(), "org.freedesktop.NetworkManager.Device.Wireless").await else { return Vec::new() };
    let active: Option<OwnedObjectPath> = prop(&w, "ActiveAccessPoint").await;
    let paths: Vec<OwnedObjectPath> = prop(&w, "AccessPoints").await.unwrap_or_default();
    let mut out = Vec::new();
    for path in paths {
        let Ok(a) = proxy(path.as_str(), "org.freedesktop.NetworkManager.AccessPoint").await else { continue };
        let Some(ssid) = prop::<Vec<u8>>(&a, "Ssid").await.as_deref().and_then(ssid_text) else { continue };
        let flags: u32 = prop(&a, "Flags").await.unwrap_or(0);
        let wpa: u32 = prop(&a, "WpaFlags").await.unwrap_or(0);
        let rsn: u32 = prop(&a, "RsnFlags").await.unwrap_or(0);
        let strength: u8 = prop(&a, "Strength").await.unwrap_or(0);
        out.push(Ap { ssid, strength, security: security(flags, wpa, rsn), active: active.as_ref() == Some(&path) });
    }
    out
}

/// A saved connection: its path, uuid, name, type and (Wi-Fi) network name.
struct Saved {
    path: OwnedObjectPath,
    uuid: String,
    id: String,
    kind: String,
    ssid: Option<String>,
}

fn setting_str(s: &HashMap<String, HashMap<String, OwnedValue>>, group: &str, key: &str) -> Option<String> {
    s.get(group)?.get(key).and_then(|v| String::try_from(v.clone()).ok())
}

async fn saved() -> Vec<Saved> {
    let Ok(settings) = proxy(SETTINGS_PATH, "org.freedesktop.NetworkManager.Settings").await else { return Vec::new() };
    let paths: Vec<OwnedObjectPath> = settings.call("ListConnections", &()).await.unwrap_or_default();
    let mut out = Vec::new();
    for path in paths {
        let Ok(c) = proxy(path.as_str(), "org.freedesktop.NetworkManager.Settings.Connection").await else { continue };
        let Ok(s) = c.call::<_, _, HashMap<String, HashMap<String, OwnedValue>>>("GetSettings", &()).await else { continue };
        let ssid = s
            .get("802-11-wireless")
            .and_then(|w| w.get("ssid"))
            .and_then(|v| Vec::<u8>::try_from(v.clone()).ok())
            .as_deref()
            .and_then(ssid_text);
        out.push(Saved {
            path,
            uuid: setting_str(&s, "connection", "uuid").unwrap_or_default(),
            id: setting_str(&s, "connection", "id").unwrap_or_default(),
            kind: setting_str(&s, "connection", "type").unwrap_or_default(),
            ssid,
        });
    }
    out
}

async fn wifi_enabled() -> (bool, bool) {
    match proxy(NM_PATH, NM).await {
        Ok(nm) => (prop(&nm, "WirelessEnabled").await.unwrap_or(false), prop(&nm, "WirelessHardwareEnabled").await.unwrap_or(true)),
        Err(_) => (false, false),
    }
}

/// The menu-bar icon: cheap (no network list).
#[tauri::command]
pub async fn network_summary() -> Result<Value, String> {
    let devs = devices().await?;
    let (enabled, _) = wifi_enabled().await;
    let ethernet_up = devs.iter().any(|d| d.kind == DEVICE_ETHERNET && d.state == STATE_ACTIVATED);
    let wifi = devs.iter().find(|d| d.kind == DEVICE_WIFI && d.state == STATE_ACTIVATED);
    let mut strength = None;
    if let Some(w) = wifi {
        strength = Some(access_points(w).await.iter().find(|a| a.active).map_or(100, |a| a.strength));
    }
    let connecting = devs.iter().any(|d| d.state > STATE_DISCONNECTED && d.state < STATE_ACTIVATED);
    let (kind, bars) = icon(ethernet_up, strength, enabled, connecting);
    let name = if ethernet_up {
        devs.iter().find(|d| d.kind == DEVICE_ETHERNET && d.state == STATE_ACTIVATED).and_then(|d| d.connection.clone())
    } else {
        wifi.and_then(|d| d.connection.clone())
    };
    Ok(json!({"icon": kind, "bars": bars, "name": name, "has_wifi": devs.iter().any(|d| d.kind == DEVICE_WIFI)}))
}

/// Everything the menu and the Network pane show.
#[tauri::command]
pub async fn network_state() -> Result<Value, String> {
    let devs = devices().await?;
    let (enabled, hardware) = wifi_enabled().await;
    let known = saved().await;
    let known_ssids: Vec<String> = known.iter().filter_map(|k| k.ssid.clone()).collect();
    let mut aps = Vec::new();
    for d in devs.iter().filter(|d| d.kind == DEVICE_WIFI) {
        aps.extend(access_points(d).await);
    }
    let devices: Vec<Value> = devs
        .iter()
        .map(|d| {
            json!({
                "iface": d.iface,
                "kind": if d.kind == DEVICE_WIFI { "wifi" } else { "ethernet" },
                "state": match d.state {
                    STATE_ACTIVATED => "connected",
                    s if s > STATE_DISCONNECTED => "connecting",
                    STATE_DISCONNECTED => "disconnected",
                    STATE_UNAVAILABLE => "unavailable",
                    _ => "unmanaged",
                },
                "ip4": d.ip4,
                "connection": d.connection,
            })
        })
        .collect();
    let known: Vec<Value> = known
        .iter()
        .filter(|k| k.kind == "802-11-wireless")
        .map(|k| json!({"uuid": k.uuid, "name": k.id, "ssid": k.ssid}))
        .collect();
    Ok(json!({
        "wifi_enabled": enabled,
        "wifi_hardware": hardware,
        "devices": devices,
        "networks": network_list(&aps, &known_ssids),
        "known": known,
    }))
}

#[tauri::command]
pub async fn network_wifi(on: bool) -> Result<(), String> {
    let nm = proxy(NM_PATH, NM).await?;
    nm.set_property("WirelessEnabled", on).await.map_err(|e| format!("Wi-Fi {}: {e}", if on { "on" } else { "off" }))?;
    info!("network: Wi-Fi {}", if on { "on" } else { "off" });
    Ok(())
}

#[tauri::command]
pub async fn network_scan() -> Result<(), String> {
    for d in devices().await?.iter().filter(|d| d.kind == DEVICE_WIFI) {
        let w = proxy(d.path.as_str(), "org.freedesktop.NetworkManager.Device.Wireless").await?;
        let opts: HashMap<&str, ZValue> = HashMap::new();
        // NetworkManager refuses scans that come too soon after the last one; that's fine.
        let _ = w.call::<_, _, ()>("RequestScan", &(opts,)).await;
    }
    Ok(())
}

/// Join `ssid`: a known one again, else a new connection of yours (`password` for secured ones).
#[tauri::command]
pub async fn network_join(app: AppHandle, ssid: String, password: Option<String>) -> Result<(), String> {
    let devs = devices().await?;
    let wifi = devs.iter().find(|d| d.kind == DEVICE_WIFI).ok_or("no Wi-Fi device")?;
    let nm = proxy(NM_PATH, NM).await?;
    let root = OwnedObjectPath::try_from("/").map_err(|e| e.to_string())?;
    let known = saved().await.into_iter().find(|k| k.ssid.as_deref() == Some(ssid.as_str()) && password.is_none());
    let (active, added): (OwnedObjectPath, Option<OwnedObjectPath>) = if let Some(k) = known {
        info!("network: joining {ssid} (known)");
        let a = nm
            .call("ActivateConnection", &(k.path.clone(), wifi.path.clone(), root.clone()))
            .await
            .map_err(|e| format!("Couldn't join {ssid}: {e}"))?;
        (a, None)
    } else {
        let aps = access_points(wifi).await;
        let sec = aps.iter().filter(|a| a.ssid == ssid).max_by_key(|a| a.strength).map_or(Security::Open, |a| a.security);
        let mgmt = key_mgmt(sec, password.as_deref())?;
        let user = std::env::var("USER").unwrap_or_default();
        let mut connection: HashMap<&str, ZValue> = HashMap::new();
        connection.insert("type", ZValue::from("802-11-wireless"));
        connection.insert("id", ZValue::from(ssid.clone()));
        if !user.is_empty() {
            connection.insert("permissions", ZValue::from(vec![format!("user:{user}:")]));
        }
        let mut wireless: HashMap<&str, ZValue> = HashMap::new();
        wireless.insert("ssid", ZValue::from(ssid.as_bytes().to_vec()));
        wireless.insert("mode", ZValue::from("infrastructure"));
        let mut settings: HashMap<&str, HashMap<&str, ZValue>> = HashMap::new();
        settings.insert("connection", connection);
        settings.insert("802-11-wireless", wireless);
        if let (Some(m), Some(p)) = (mgmt, password.as_deref()) {
            let mut security: HashMap<&str, ZValue> = HashMap::new();
            security.insert("key-mgmt", ZValue::from(m));
            security.insert("psk", ZValue::from(p.to_string()));
            settings.insert("802-11-wireless-security", security);
        }
        info!("network: joining {ssid} (new, {sec:?})");
        let (conn, active): (OwnedObjectPath, OwnedObjectPath) = nm
            .call("AddAndActivateConnection", &(settings, wifi.path.clone(), root.clone()))
            .await
            .map_err(|e| format!("Couldn't join {ssid}: {e}"))?;
        (active, Some(conn))
    };
    // How it goes, without holding up the page.
    tauri::async_runtime::spawn(async move {
        let result = watch_activation(&active).await;
        let (ok, message) = match result {
            Ok(()) => (true, format!("Joined {ssid}")),
            Err(e) => {
                if let Some(conn) = &added {
                    if let Ok(c) = proxy(conn.as_str(), "org.freedesktop.NetworkManager.Settings.Connection").await {
                        let _ = c.call::<_, _, ()>("Delete", &()).await;
                    }
                }
                (false, e)
            }
        };
        info!("network: {ssid}: {}", if ok { "joined".to_string() } else { message.clone() });
        let _ = app.emit("kompact-network-joined", json!({"ssid": ssid, "ok": ok, "message": message}));
    });
    Ok(())
}

/// Wait (at most 40 s) for an activation to finish.
async fn watch_activation(active: &OwnedObjectPath) -> Result<(), String> {
    for _ in 0..80 {
        tokio::time::sleep(Duration::from_millis(500)).await;
        let Ok(a) = proxy(active.as_str(), "org.freedesktop.NetworkManager.Connection.Active").await else {
            return Err("The connection went away (a wrong password?)".into());
        };
        match prop::<u32>(&a, "State").await {
            Some(2) => return Ok(()), // activated
            Some(3) | Some(4) => return Err("Couldn't connect (a wrong password?)".into()),
            None => return Err("Couldn't connect (a wrong password?)".into()),
            _ => {}
        }
    }
    Err("Timed out joining".into())
}

#[tauri::command]
pub async fn network_disconnect(iface: String) -> Result<(), String> {
    let d = devices().await?.into_iter().find(|d| d.iface == iface).ok_or_else(|| format!("no device {iface}"))?;
    let p = proxy(d.path.as_str(), "org.freedesktop.NetworkManager.Device").await?;
    p.call::<_, _, ()>("Disconnect", &()).await.map_err(|e| format!("Disconnect {iface}: {e}"))?;
    info!("network: disconnected {iface}");
    Ok(())
}

#[tauri::command]
pub async fn network_forget(uuid: String) -> Result<(), String> {
    let s = saved().await.into_iter().find(|k| k.uuid == uuid).ok_or("no such network")?;
    let c = proxy(s.path.as_str(), "org.freedesktop.NetworkManager.Settings.Connection").await?;
    c.call::<_, _, ()>("Delete", &()).await.map_err(|e| format!("Forget {}: {e}", s.id))?;
    info!("network: forgot {}", s.id);
    Ok(())
}

/// NetworkManager's signals → `kompact-network-changed` (at most every 700 ms).
pub fn start(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        use futures_util::StreamExt;
        let conn = match bus().await {
            Ok(c) => c,
            Err(e) => return warn!("network: {e}; no network status"),
        };
        let rule = match zbus::MatchRule::builder().msg_type(zbus::message::Type::Signal).sender(NM).map(|b| b.build()) {
            Ok(r) => r,
            Err(e) => return warn!("network: {e}"),
        };
        let mut stream = match zbus::MessageStream::for_match_rule(rule, &conn, None).await {
            Ok(s) => s,
            Err(e) => return warn!("network: can't watch NetworkManager: {e}"),
        };
        info!("network: watching NetworkManager");
        let mut last = std::time::Instant::now() - Duration::from_secs(1);
        let pending = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        while stream.next().await.is_some() {
            if last.elapsed() >= Duration::from_millis(700) {
                last = std::time::Instant::now();
                let _ = app.emit("kompact-network-changed", ());
            } else if !pending.swap(true, std::sync::atomic::Ordering::Relaxed) {
                // One more after the burst, so the last change isn't lost.
                let (app, pending) = (app.clone(), pending.clone());
                tauri::async_runtime::spawn(async move {
                    tokio::time::sleep(Duration::from_millis(700)).await;
                    pending.store(false, std::sync::atomic::Ordering::Relaxed);
                    let _ = app.emit("kompact-network-changed", ());
                });
            }
        }
        warn!("network: NetworkManager's signals stopped");
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ap(ssid: &str, strength: u8, active: bool) -> Ap {
        Ap { ssid: ssid.into(), strength, security: Security::Psk, active }
    }

    #[test]
    fn security_kinds() {
        assert_eq!(security(0, 0, 0), Security::Open);
        assert_eq!(security(1, 0, KEY_MGMT_PSK), Security::Psk);
        assert_eq!(security(1, KEY_MGMT_PSK, KEY_MGMT_PSK | KEY_MGMT_SAE), Security::Psk, "WPA2/WPA3 mixed: the password works");
        assert_eq!(security(1, 0, KEY_MGMT_SAE), Security::Sae);
        assert_eq!(security(1, 0, KEY_MGMT_802_1X), Security::Unsupported);
        assert_eq!(security(AP_FLAGS_PRIVACY, 0, 0), Security::Unsupported, "WEP");
    }

    #[test]
    fn one_entry_per_name() {
        let aps = [ap("Home", 40, false), ap("Cafe", 90, false), ap("Home", 70, true), ap("Neighbour", 55, false)];
        let list = network_list(&aps, &["Neighbour".into()]);
        let names: Vec<&str> = list.iter().map(|n| n.ssid.as_str()).collect();
        assert_eq!(names, ["Home", "Neighbour", "Cafe"], "joined, known, then by strength");
        assert_eq!((list[0].strength, list[0].bars, list[0].active), (70, 2, true));
        assert!(list[1].known && !list[2].known);
    }

    #[test]
    fn names_and_bars() {
        assert_eq!(ssid_text(b"Home\0"), Some("Home".into()));
        assert_eq!(ssid_text(b""), None, "hidden network");
        assert_eq!(ssid_text(&[0xff, b'x']), Some("\u{fffd}x".into()));
        assert_eq!([0, 24, 25, 50, 74, 75, 100].map(bars), [0, 0, 1, 2, 2, 3, 3]);
    }

    #[test]
    fn the_icon() {
        assert_eq!(icon(true, Some(30), true, false), ("ethernet", 3), "a cable wins");
        assert_eq!(icon(false, Some(60), true, false), ("wifi", 2));
        assert_eq!(icon(false, None, true, true), ("connecting", 0));
        assert_eq!(icon(false, None, false, false), ("off", 0));
        assert_eq!(icon(false, None, true, false), ("none", 0));
    }

    /// Read-only, against this machine's NetworkManager: `cargo test live_state -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn live_state() {
        let state = tauri::async_runtime::block_on(network_state()).expect("network state");
        println!("{}", serde_json::to_string_pretty(&state).unwrap());
        let summary = tauri::async_runtime::block_on(network_summary()).expect("summary");
        println!("{summary}");
    }

    #[test]
    fn joining_rules() {
        assert_eq!(key_mgmt(Security::Open, None), Ok(None));
        assert_eq!(key_mgmt(Security::Psk, Some("password1")), Ok(Some("wpa-psk")));
        assert!(key_mgmt(Security::Psk, Some("short")).is_err());
        assert!(key_mgmt(Security::Psk, None).is_err());
        assert_eq!(key_mgmt(Security::Sae, Some("x")), Ok(Some("sae")));
        assert!(key_mgmt(Security::Unsupported, Some("password1")).is_err());
    }
}
