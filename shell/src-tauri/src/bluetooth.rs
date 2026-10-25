//! 12b: Bluetooth, from BlueZ on the system bus (zbus): the menu bar's Bluetooth item and menu
//! (overlay "status" mode) and System Settings → Bluetooth.
//!
//! - `bluetooth_state`: the adapter (on/off, name, looking for devices) and its devices: paired
//!   ones ("My Devices") and the others seen nearby, with their kind and battery if known.
//! - `bluetooth_power`, `bluetooth_discover` (on for at most `DISCOVERY` while the pane shows),
//!   `bluetooth_connect` / `bluetooth_disconnect`, `bluetooth_pair` (then trusted and
//!   connected), `bluetooth_forget`.
//! - A pairing agent (`org.bluez.Agent1`, "KeyboardDisplay", the default one): BlueZ asks it to
//!   confirm a passkey, for a PIN, to show a passkey to type on the device, or whether a device
//!   may pair. Each question becomes `kompact-bluetooth-ask` (and is kept for a page that opens
//!   later, `bluetooth_pending`); the answer comes back with `bluetooth_answer`. Unanswered
//!   questions are refused after `ASK_TIMEOUT`. A question that comes without the Settings
//!   window open (a phone pairing with this computer) opens it at the Bluetooth pane.
//!
//! BlueZ's signals (debounced) become `kompact-bluetooth-changed`; pages fetch again.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Duration;

use serde::Serialize;
use serde_json::{json, Value};
use tauri::{AppHandle, Emitter, Manager};
use tracing::{info, warn};
use zbus::zvariant::{ObjectPath, OwnedObjectPath, OwnedValue};

const BLUEZ: &str = "org.bluez";
const AGENT_PATH: &str = "/org/kompact/BluetoothAgent";
/// How long discovery runs before it's stopped on its own.
const DISCOVERY: Duration = Duration::from_secs(60);
/// How long a pairing question waits for an answer.
const ASK_TIMEOUT: Duration = Duration::from_secs(60);
/// fix/bluetooth: how long "Pair Again" waits for the device to show up in pairing mode.
const FIND_AGAIN: Duration = Duration::from_secs(30);
/// After a scan is paused, before connecting (a CSR dongle can't scan and page at once).
const SCAN_SETTLE: Duration = Duration::from_millis(400);

// ---- What went wrong (pure) ----

/// fix/bluetooth: a BlueZ failure, sorted by what to do about it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Problem {
    /// `br-connection-key-missing`: the device forgot this computer (paired with a phone since,
    /// or reset): only pairing again helps.
    KeyMissing,
    /// Page / connection timeouts: off, out of range, or busy with a phone.
    NoAnswer,
    /// It said no (wrong code, refused).
    Refused,
    /// `br-connection-profile-unavailable`: no audio profile to take it (the sound system).
    NoProfile,
    /// `InProgress`, `Busy`: something else is already connecting it.
    Busy,
    AlreadyPaired,
    Cancelled,
    Other,
}

/// What a BlueZ error is: its D-Bus name's last part (`Failed`, `AuthenticationFailed`, …) and
/// its message (`br-connection-key-missing`, `Page Timeout`, …).
pub fn classify(name: &str, message: &str) -> Problem {
    let m = message.to_lowercase();
    match name {
        _ if m.contains("key-missing") || m.contains("key missing") => Problem::KeyMissing,
        "AuthenticationCanceled" => Problem::Cancelled,
        "AuthenticationFailed" | "AuthenticationRejected" => Problem::Refused,
        "AuthenticationTimeout" | "ConnectionAttemptFailed" => Problem::NoAnswer,
        "AlreadyExists" => Problem::AlreadyPaired,
        "InProgress" | "Busy" => Problem::Busy,
        _ if m.contains("profile-unavailable") || m.contains("profile unavailable") => Problem::NoProfile,
        _ if m.contains("timeout") || m.contains("page-timeout") || m.contains("abort-by-local") || m.contains("host is down") || m.contains("connection-unknown") => Problem::NoAnswer,
        _ if m.contains("refused") || m.contains("rejected") || m.contains("not-allowed") => Problem::Refused,
        _ if m.contains("in progress") || m.contains("busy") => Problem::Busy,
        _ => Problem::Other,
    }
}

/// What to tell the person. `KeyMissing` starts with `key-missing:`, so the page offers "Pair
/// Again".
pub fn explain(problem: Problem, name: &str, message: &str) -> String {
    match problem {
        Problem::KeyMissing => format!("key-missing: {name} has forgotten this computer (paired with another device since, or reset). Put it in pairing mode and press Pair Again."),
        Problem::NoAnswer => format!("{name} didn't answer. Is it on and close by, and not connected to your phone? For pairing, put it in pairing mode first."),
        Problem::Refused => format!("{name} refused (a wrong code, or it doesn't accept this computer)."),
        Problem::NoProfile => format!("{name} connected, but nothing took its sound: the sound system may be restarting. Try Connect again."),
        Problem::Busy => format!("{name} is already being connected; wait a moment."),
        Problem::AlreadyPaired => format!("{name} is already paired."),
        Problem::Cancelled => "Cancelled.".into(),
        Problem::Other if message.is_empty() => format!("{name}: it didn't work."),
        Problem::Other => format!("{name}: {message}"),
    }
}

/// Worth one more try at once (the radio missed it).
pub fn retry_helps(p: Problem) -> bool {
    matches!(p, Problem::NoAnswer | Problem::NoProfile)
}

// ---- Reconnecting (pure) ----

/// The addresses of the devices to reconnect, one a line.
pub fn parse_remembered(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for l in text.lines().map(str::trim) {
        let ok = l.len() == 17 && l.split(':').count() == 6 && l.split(':').all(|p| p.len() == 2 && p.chars().all(|c| c.is_ascii_hexdigit()));
        if ok && !out.iter().any(|o| o.eq_ignore_ascii_case(l)) {
            out.push(l.to_uppercase());
        }
    }
    out
}

/// Of `devices`, the paired ones remembered and not connected now: the ones to reconnect.
pub fn to_reconnect<'a>(remembered: &[String], devices: &'a [Device]) -> Vec<&'a Device> {
    devices.iter().filter(|d| d.paired && !d.connected && remembered.iter().any(|r| r.eq_ignore_ascii_case(&d.address))).collect()
}

// ---- The pure part ----

/// What kind of device, from BlueZ's `Icon` (freedesktop icon names).
pub fn kind(icon: &str) -> &'static str {
    match icon {
        i if i.starts_with("audio-headset") || i.starts_with("audio-headphones") => "headphones",
        i if i.starts_with("audio") => "speaker",
        "input-keyboard" => "keyboard",
        "input-mouse" | "input-tablet" => "mouse",
        "input-gaming" => "gamepad",
        "phone" => "phone",
        "computer" => "computer",
        _ => "device",
    }
}

/// A passkey as the device shows it: six digits.
pub fn passkey_text(passkey: u32) -> String {
    format!("{:06}", passkey % 1_000_000)
}

/// A PIN typed in: 1–16 characters (legacy pairing); a passkey: up to six digits.
pub fn parse_answer(kind: &str, value: &str) -> Result<Answer, String> {
    let v = value.trim();
    match kind {
        "pin" if (1..=16).contains(&v.len()) => Ok(Answer::Pin(v.to_string())),
        "pin" => Err("A PIN is 1 to 16 characters.".into()),
        "passkey" => v.parse::<u32>().ok().filter(|n| *n <= 999_999).map(Answer::Passkey).ok_or_else(|| "A passkey is up to six digits.".into()),
        _ => Ok(Answer::Yes),
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Device {
    pub path: String,
    pub address: String,
    pub name: String,
    pub kind: &'static str,
    pub paired: bool,
    pub connected: bool,
    pub battery: Option<u8>,
    pub rssi: Option<i16>,
}

/// Paired ones first (connected first, then by name), then the others by signal strength; the
/// nameless ones nearby (only an address) are left out unless paired.
pub fn sort_devices(mut list: Vec<Device>) -> Vec<Device> {
    list.retain(|d| d.paired || d.name != d.address);
    list.sort_by(|a, b| {
        let by_name = || a.name.to_lowercase().cmp(&b.name.to_lowercase());
        match (a.paired, b.paired) {
            (true, true) => b.connected.cmp(&a.connected).then_with(by_name),
            (true, false) => std::cmp::Ordering::Less,
            (false, true) => std::cmp::Ordering::Greater,
            (false, false) => b.rssi.unwrap_or(i16::MIN).cmp(&a.rssi.unwrap_or(i16::MIN)).then_with(by_name),
        }
    });
    list
}

// ---- BlueZ ----

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
        .destination(BLUEZ)
        .and_then(|b| b.path(path.to_string()))
        .and_then(|b| b.interface(iface.to_string()))
        .map_err(|e| e.to_string())?
        .cache_properties(zbus::proxy::CacheProperties::No)
        .build()
        .await
        .map_err(|e| format!("BlueZ: {e}"))
}

type Objects = HashMap<OwnedObjectPath, HashMap<String, HashMap<String, OwnedValue>>>;

async fn objects() -> Result<Objects, String> {
    let om = proxy("/", "org.freedesktop.DBus.ObjectManager").await?;
    om.call("GetManagedObjects", &()).await.map_err(|e| format!("BlueZ isn't answering: {e}"))
}

fn get<T: TryFrom<OwnedValue>>(props: &HashMap<String, OwnedValue>, key: &str) -> Option<T> {
    props.get(key).and_then(|v| T::try_from(v.clone()).ok())
}

/// Whether the first adapter is on.
fn adapter_powered(objs: &Objects) -> Option<bool> {
    let path = adapter_of(objs)?;
    let a = objs.get(&OwnedObjectPath::try_from(path.as_str()).ok()?)?.get("org.bluez.Adapter1")?;
    get::<bool>(a, "Powered")
}

/// The first adapter's path.
fn adapter_of(objs: &Objects) -> Option<String> {
    let mut paths: Vec<String> = objs.iter().filter(|(_, i)| i.contains_key("org.bluez.Adapter1")).map(|(p, _)| p.to_string()).collect();
    paths.sort();
    paths.into_iter().next()
}

fn devices_of(objs: &Objects, adapter: &str) -> Vec<Device> {
    let mut out = Vec::new();
    for (path, ifaces) in objs {
        let Some(d) = ifaces.get("org.bluez.Device1") else { continue };
        if get::<OwnedObjectPath>(d, "Adapter").map(|a| a.to_string()).as_deref() != Some(adapter) {
            continue;
        }
        let address: String = get(d, "Address").unwrap_or_default();
        let name: String = get::<String>(d, "Alias").or_else(|| get(d, "Name")).unwrap_or_else(|| address.clone());
        out.push(Device {
            path: path.to_string(),
            name: if name.replace('-', ":") == address { address.clone() } else { name },
            address,
            kind: kind(&get::<String>(d, "Icon").unwrap_or_default()),
            paired: get(d, "Paired").unwrap_or(false),
            connected: get(d, "Connected").unwrap_or(false),
            battery: ifaces.get("org.bluez.Battery1").and_then(|b| get::<u8>(b, "Percentage")),
            rssi: get(d, "RSSI"),
        });
    }
    sort_devices(out)
}

#[tauri::command]
pub async fn bluetooth_state() -> Result<Value, String> {
    let objs = objects().await?;
    let Some(adapter) = adapter_of(&objs) else { return Ok(json!({"adapter": null})) };
    let a = &objs[&OwnedObjectPath::try_from(adapter.as_str()).map_err(|e| e.to_string())?]["org.bluez.Adapter1"];
    Ok(json!({
        "adapter": adapter,
        "name": get::<String>(a, "Alias").unwrap_or_default(),
        "powered": get::<bool>(a, "Powered").unwrap_or(false),
        "blocked": get::<String>(a, "PowerState").as_deref() == Some("off-blocked"),
        "discovering": get::<bool>(a, "Discovering").unwrap_or(false),
        "discoverable": get::<bool>(a, "Discoverable").unwrap_or(false),
        "devices": devices_of(&objs, &adapter),
    }))
}

async fn adapter() -> Result<String, String> {
    adapter_of(&objects().await?).ok_or_else(|| "no Bluetooth adapter".into())
}

#[tauri::command]
pub async fn bluetooth_power(on: bool) -> Result<(), String> {
    let a = proxy(&adapter().await?, "org.bluez.Adapter1").await?;
    a.set_property("Powered", on).await.map_err(|e| format!("Bluetooth {}: {e}", if on { "on" } else { "off" }))?;
    info!("bluetooth: {}", if on { "on" } else { "off" });
    Ok(())
}

/// Look for devices (and be seen, so a phone can pair with this computer), at most `DISCOVERY`;
/// or stop.
#[tauri::command]
pub async fn bluetooth_discover(on: bool) -> Result<(), String> {
    let path = adapter().await?;
    let a = proxy(&path, "org.bluez.Adapter1").await?;
    let discovering: bool = a.get_property("Discovering").await.unwrap_or(false);
    if on {
        let _ = a.set_property("Discoverable", true).await;
        if !discovering {
            a.call::<_, _, ()>("StartDiscovery", &()).await.map_err(|e| format!("Can't look for devices: {e}"))?;
            info!("bluetooth: looking for devices");
        }
        static GENERATION: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let generation = GENERATION.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
        tauri::async_runtime::spawn(async move {
            tokio::time::sleep(DISCOVERY).await;
            if GENERATION.load(std::sync::atomic::Ordering::Relaxed) == generation {
                stop_discovery(&path).await;
            }
        });
    } else {
        stop_discovery(&path).await;
    }
    Ok(())
}

async fn stop_discovery(adapter: &str) {
    let Ok(a) = proxy(adapter, "org.bluez.Adapter1").await else { return };
    let _ = a.set_property("Discoverable", false).await;
    if a.get_property::<bool>("Discovering").await.unwrap_or(false) {
        let _ = a.call::<_, _, ()>("StopDiscovery", &()).await;
        info!("bluetooth: stopped looking for devices");
    }
}

fn device_path(path: &str) -> Result<String, String> {
    // Only BlueZ's own device objects.
    (path.starts_with("/org/bluez/") && ObjectPath::try_from(path).is_ok()).then(|| path.to_string()).ok_or_else(|| "not a Bluetooth device".into())
}

/// A BlueZ error as a `Problem` and its message.
fn problem(e: &zbus::Error) -> (Problem, String) {
    match e {
        zbus::Error::MethodError(name, msg, _) => {
            let n = name.as_str().rsplit('.').next().unwrap_or("").to_string();
            let m = msg.clone().unwrap_or_default();
            (classify(&n, &m), if m.is_empty() { n } else { m })
        }
        other => (Problem::Other, other.to_string()),
    }
}

/// fix/bluetooth: stop looking for devices while connecting (a CSR 4.0 dongle, this PC's, can't
/// look and connect at once: the connection times out). Whether it was looking.
async fn pause_scan() -> bool {
    let Ok(path) = adapter().await else { return false };
    let Ok(a) = proxy(&path, "org.bluez.Adapter1").await else { return false };
    if !a.get_property::<bool>("Discovering").await.unwrap_or(false) {
        return false;
    }
    let _ = a.call::<_, _, ()>("StopDiscovery", &()).await;
    info!("bluetooth: looking paused while connecting");
    tokio::time::sleep(SCAN_SETTLE).await;
    true
}

async fn resume_scan(was: bool) {
    if !was {
        return;
    }
    let Ok(path) = adapter().await else { return };
    if let Ok(a) = proxy(&path, "org.bluez.Adapter1").await {
        if a.call::<_, _, ()>("StartDiscovery", &()).await.is_ok() {
            info!("bluetooth: looking again");
        }
    }
}

/// Call `method` on the device, once more if the radio missed it; the problem if it failed.
async fn call_device(d: &zbus::Proxy<'_>, method: &str, name: &str) -> Result<(), (Problem, String)> {
    let mut last = (Problem::Other, String::new());
    for attempt in 0..2 {
        match d.call::<_, _, ()>(method, &()).await {
            Ok(()) => return Ok(()),
            Err(e) => {
                last = problem(&e);
                warn!("bluetooth: {method} {name} failed ({:?}): {}", last.0, last.1);
                if attempt == 0 && retry_helps(last.0) {
                    tokio::time::sleep(Duration::from_secs(2)).await;
                    continue;
                }
                break;
            }
        }
    }
    Err(last)
}

async fn alias(d: &zbus::Proxy<'_>) -> String {
    d.get_property::<String>("Alias").await.unwrap_or_else(|_| "The device".into())
}

#[tauri::command]
pub async fn bluetooth_connect(path: String) -> Result<(), String> {
    let d = proxy(&device_path(&path)?, "org.bluez.Device1").await?;
    let name = alias(&d).await;
    info!("bluetooth: connecting {name}");
    let scanning = pause_scan().await;
    let r = call_device(&d, "Connect", &name).await;
    resume_scan(scanning).await;
    match r {
        Ok(()) => {
            info!("bluetooth: {name} connected");
            remember(&d, true).await;
            Ok(())
        }
        Err((p, m)) => Err(explain(p, &name, &m)),
    }
}

#[tauri::command]
pub async fn bluetooth_disconnect(path: String) -> Result<(), String> {
    let d = proxy(&device_path(&path)?, "org.bluez.Device1").await?;
    let name = alias(&d).await;
    d.call::<_, _, ()>("Disconnect", &()).await.map_err(|e| explain(problem(&e).0, &name, &problem(&e).1))?;
    // You disconnected it: don't reconnect it by yourself.
    remember(&d, false).await;
    info!("bluetooth: disconnected {name}");
    Ok(())
}

/// Pair (the agent asks what's needed), then trust and connect.
#[tauri::command]
pub async fn bluetooth_pair(path: String) -> Result<(), String> {
    let path = device_path(&path)?;
    let d = proxy(&path, "org.bluez.Device1").await?;
    let name = alias(&d).await;
    info!("bluetooth: pairing {name}");
    let scanning = pause_scan().await;
    let r = pair_and_connect(&d, &name).await;
    resume_scan(scanning && r.is_err()).await;
    r
}

async fn pair_and_connect(d: &zbus::Proxy<'_>, name: &str) -> Result<(), String> {
    match call_device(d, "Pair", name).await {
        Ok(()) | Err((Problem::AlreadyPaired, _)) => {}
        Err((p, m)) => return Err(explain(p, name, &m)),
    }
    let _ = d.set_property("Trusted", true).await;
    info!("bluetooth: {name} paired and trusted");
    match call_device(d, "Connect", name).await {
        Ok(()) => {
            info!("bluetooth: {name} connected");
            remember(d, true).await;
            Ok(())
        }
        Err((p, m)) => {
            info!("bluetooth: {name} paired, not connected: {m}");
            Err(format!("{name} is paired, but didn't connect yet: {}", explain(p, name, &m).trim_start_matches("key-missing: ")))
        }
    }
}

/// fix/bluetooth: "Pair Again" for a device that forgot this computer (key missing): forget it,
/// look for it again (it must be in pairing mode), then pair, trust and connect.
#[tauri::command]
pub async fn bluetooth_repair(path: String) -> Result<(), String> {
    let path = device_path(&path)?;
    let d = proxy(&path, "org.bluez.Device1").await?;
    let name = alias(&d).await;
    let address: String = d.get_property("Address").await.map_err(|e| e.to_string())?;
    info!("bluetooth: pairing {name} again");
    let adapter_path = adapter().await?;
    let a = proxy(&adapter_path, "org.bluez.Adapter1").await?;
    let p = OwnedObjectPath::try_from(path.as_str()).map_err(|e| e.to_string())?;
    a.call::<_, _, ()>("RemoveDevice", &(p,)).await.map_err(|e| format!("Couldn't forget {name}: {}", problem(&e).1))?;
    let _ = a.call::<_, _, ()>("StartDiscovery", &()).await;
    let started = std::time::Instant::now();
    let mut found = None;
    while started.elapsed() < FIND_AGAIN {
        tokio::time::sleep(Duration::from_millis(500)).await;
        if let Ok(objs) = objects().await {
            found = devices_of(&objs, &adapter_path).into_iter().find(|x| x.address.eq_ignore_ascii_case(&address)).map(|x| x.path);
            if found.is_some() {
                break;
            }
        }
    }
    let _ = a.call::<_, _, ()>("StopDiscovery", &()).await;
    let Some(found) = found else {
        return Err(format!("{name} didn't show up. Put it in pairing mode (usually: hold its button until the light flashes), then pair it from Nearby Devices."));
    };
    info!("bluetooth: found {name} again after {} ms", started.elapsed().as_millis());
    tokio::time::sleep(SCAN_SETTLE).await;
    let d = proxy(&found, "org.bluez.Device1").await?;
    pair_and_connect(&d, &name).await
}

#[tauri::command]
pub async fn bluetooth_forget(path: String) -> Result<(), String> {
    let path = device_path(&path)?;
    if let Ok(d) = proxy(&path, "org.bluez.Device1").await {
        remember(&d, false).await;
    }
    let a = proxy(&adapter().await?, "org.bluez.Adapter1").await?;
    let p = OwnedObjectPath::try_from(path.as_str()).map_err(|e| e.to_string())?;
    a.call::<_, _, ()>("RemoveDevice", &(p,)).await.map_err(|e| format!("Couldn't remove it: {}", problem(&e).1))?;
    info!("bluetooth: forgot {path}");
    Ok(())
}

// ---- Reconnecting ----

fn remembered_file() -> Option<std::path::PathBuf> {
    let base = std::env::var_os("XDG_STATE_HOME")
        .map(std::path::PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join(".local").join("state")))?;
    Some(base.join("kompact").join("bluetooth-reconnect"))
}

fn remembered() -> Vec<String> {
    remembered_file().and_then(|f| std::fs::read_to_string(f).ok()).map(|t| parse_remembered(&t)).unwrap_or_default()
}

/// Add the device to (or take it off) the ones to reconnect.
async fn remember(d: &zbus::Proxy<'_>, on: bool) {
    let Ok(address) = d.get_property::<String>("Address").await else { return };
    remember_address(&address, on);
}

fn remember_address(address: &str, on: bool) {
    let mut list = remembered();
    let had = list.iter().any(|a| a.eq_ignore_ascii_case(address));
    if on == had {
        return;
    }
    if on {
        list.push(address.to_uppercase());
    } else {
        list.retain(|a| !a.eq_ignore_ascii_case(address));
    }
    let Some(f) = remembered_file() else { return };
    if let Some(dir) = f.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = std::fs::write(&f, list.iter().map(|a| format!("{a}\n")).collect::<String>());
}

/// fix/bluetooth: connect the remembered devices that aren't (after login, a restart, or
/// Bluetooth switched on): a few tries each, as headsets often wake slowly.
async fn reconnect(why: &str) {
    let list = remembered();
    if list.is_empty() {
        return;
    }
    for attempt in 1..=3 {
        let Ok(objs) = objects().await else { return };
        let Some(adapter) = adapter_of(&objs) else { return };
        let devices = devices_of(&objs, &adapter);
        let todo: Vec<(String, String)> = to_reconnect(&list, &devices).into_iter().map(|d| (d.path.clone(), d.name.clone())).collect();
        if todo.is_empty() {
            return;
        }
        for (path, name) in todo {
            info!("bluetooth: reconnecting {name} ({why}, try {attempt})");
            let Ok(d) = proxy(&path, "org.bluez.Device1").await else { continue };
            match d.call::<_, _, ()>("Connect", &()).await {
                Ok(()) => info!("bluetooth: {name} reconnected"),
                Err(e) => {
                    let (p, m) = problem(&e);
                    info!("bluetooth: {name} didn't reconnect ({p:?}): {m}");
                    if p == Problem::KeyMissing {
                        // Only pairing again helps; the page says so on Connect.
                        remember_address(&devices.iter().find(|x| x.path == path).map(|x| x.address.clone()).unwrap_or_default(), false);
                    }
                }
            }
        }
        tokio::time::sleep(Duration::from_secs(5)).await;
    }
}

// ---- The pairing agent ----

#[derive(Debug, Clone, PartialEq)]
pub enum Answer {
    Yes,
    No,
    Pin(String),
    Passkey(u32),
}

#[derive(Debug, zbus::DBusError)]
#[zbus(prefix = "org.bluez.Error")]
enum AgentError {
    #[zbus(error)]
    ZBus(zbus::Error),
    Rejected(String),
    Canceled(String),
}

/// A question to the user: `kind` is confirm (passkey shown on both), pin, passkey, show
/// (type `code` on the device) or authorize.
#[derive(Debug, Clone, Serialize)]
struct Question {
    id: u64,
    kind: &'static str,
    device: String,
    code: Option<String>,
}

#[derive(Default)]
struct Asks {
    next: u64,
    open: Vec<(Question, Option<tokio::sync::oneshot::Sender<Answer>>)>,
}

static ASKS: Mutex<Option<Asks>> = Mutex::new(None);

fn with_asks<T>(f: impl FnOnce(&mut Asks) -> T) -> T {
    let mut g = ASKS.lock().unwrap_or_else(|e| e.into_inner());
    f(g.get_or_insert_with(Asks::default))
}

struct Agent {
    app: AppHandle,
}

impl Agent {
    async fn name(device: &OwnedObjectPath) -> String {
        match proxy(device.as_str(), "org.bluez.Device1").await {
            Ok(d) => d.get_property::<String>("Alias").await.unwrap_or_else(|_| device.to_string()),
            Err(_) => device.to_string(),
        }
    }

    /// Ask, and wait (at most `ASK_TIMEOUT`) for the answer.
    async fn ask(&self, kind: &'static str, device: &OwnedObjectPath, code: Option<String>) -> Answer {
        let (tx, rx) = tokio::sync::oneshot::channel();
        let q = self.post(kind, device, code, Some(tx)).await;
        let answer = match tokio::time::timeout(ASK_TIMEOUT, rx).await {
            Ok(Ok(a)) => a,
            _ => Answer::No,
        };
        self.done(q.id);
        info!("bluetooth: {kind} for {}: {}", q.device, if answer == Answer::No { "no" } else { "yes" });
        answer
    }

    /// Show a question (no answer waited for, when `tx` is `None`).
    async fn post(&self, kind: &'static str, device: &OwnedObjectPath, code: Option<String>, tx: Option<tokio::sync::oneshot::Sender<Answer>>) -> Question {
        let q = Question { id: 0, kind, device: Self::name(device).await, code };
        let q = with_asks(|a| {
            a.next += 1;
            let q = Question { id: a.next, ..q };
            a.open.push((q.clone(), tx));
            q
        });
        info!("bluetooth: asking ({kind}) about {}", q.device);
        let _ = self.app.emit("kompact-bluetooth-ask", &q);
        // Nobody to see it: open Settings at the Bluetooth pane (its page picks it up).
        if self.app.get_webview_window(crate::settings::LABEL).is_none() {
            crate::settings::open_at(&self.app, "bluetooth", "a Bluetooth device wants to pair");
        }
        q
    }

    fn done(&self, id: u64) {
        with_asks(|a| a.open.retain(|(q, _)| q.id != id));
        let _ = self.app.emit("kompact-bluetooth-asked", id);
    }

    fn done_shows(&self) {
        let ids: Vec<u64> = with_asks(|a| a.open.iter().filter(|(q, tx)| tx.is_none() && q.kind == "show").map(|(q, _)| q.id).collect());
        for id in ids {
            self.done(id);
        }
    }
}

#[zbus::interface(name = "org.bluez.Agent1")]
impl Agent {
    async fn release(&self) {
        info!("bluetooth: BlueZ released the agent");
    }

    async fn request_pin_code(&self, device: OwnedObjectPath) -> Result<String, AgentError> {
        match self.ask("pin", &device, None).await {
            Answer::Pin(p) => Ok(p),
            _ => Err(AgentError::Rejected("no PIN".into())),
        }
    }

    async fn display_pin_code(&self, device: OwnedObjectPath, pincode: String) -> Result<(), AgentError> {
        self.post("show", &device, Some(pincode), None).await;
        Ok(())
    }

    async fn request_passkey(&self, device: OwnedObjectPath) -> Result<u32, AgentError> {
        match self.ask("passkey", &device, None).await {
            Answer::Passkey(p) => Ok(p),
            _ => Err(AgentError::Rejected("no passkey".into())),
        }
    }

    async fn display_passkey(&self, device: OwnedObjectPath, passkey: u32, entered: u16) {
        // Called again as digits are typed on the device; one question is enough.
        if entered == 0 {
            self.post("show", &device, Some(passkey_text(passkey)), None).await;
        }
    }

    async fn request_confirmation(&self, device: OwnedObjectPath, passkey: u32) -> Result<(), AgentError> {
        match self.ask("confirm", &device, Some(passkey_text(passkey))).await {
            Answer::No => Err(AgentError::Rejected("not confirmed".into())),
            _ => Ok(()),
        }
    }

    async fn request_authorization(&self, device: OwnedObjectPath) -> Result<(), AgentError> {
        match self.ask("authorize", &device, None).await {
            Answer::No => Err(AgentError::Rejected("not allowed".into())),
            _ => Ok(()),
        }
    }

    async fn authorize_service(&self, device: OwnedObjectPath, uuid: String) -> Result<(), AgentError> {
        // Devices you paired may use their services; others may not.
        let paired = match proxy(device.as_str(), "org.bluez.Device1").await {
            Ok(d) => d.get_property::<bool>("Paired").await.unwrap_or(false),
            Err(_) => false,
        };
        if paired {
            Ok(())
        } else {
            info!("bluetooth: refused service {uuid} for unpaired {device}");
            Err(AgentError::Rejected("not paired".into()))
        }
    }

    async fn cancel(&self) {
        info!("bluetooth: BlueZ cancelled the question");
        let open: Vec<(u64, Option<tokio::sync::oneshot::Sender<Answer>>)> = with_asks(|a| a.open.drain(..).map(|(q, tx)| (q.id, tx)).collect());
        for (id, tx) in open {
            if let Some(tx) = tx {
                let _ = tx.send(Answer::No);
            }
            let _ = self.app.emit("kompact-bluetooth-asked", id);
        }
    }
}

/// Questions still waiting (for a page that opened after they came).
#[tauri::command]
pub fn bluetooth_pending() -> Vec<Value> {
    with_asks(|a| a.open.iter().map(|(q, _)| json!(q)).collect())
}

/// The user's answer: `yes` (with `value`: the PIN or passkey typed) or no.
#[tauri::command]
pub fn bluetooth_answer(app: AppHandle, id: u64, yes: bool, value: Option<String>) -> Result<(), String> {
    let (q, tx) = with_asks(|a| {
        let i = a.open.iter().position(|(q, _)| q.id == id)?;
        Some(a.open.remove(i))
    })
    .ok_or("that question has gone")?;
    let answer = if yes { parse_answer(q.kind, value.as_deref().unwrap_or(""))? } else { Answer::No };
    match tx {
        Some(tx) => {
            let _ = tx.send(answer);
        }
        None if !yes => {
            // "Show this code" dismissed: stop the pairing.
            tauri::async_runtime::spawn(async move {
                if let Ok(objs) = objects().await {
                    for (path, ifaces) in &objs {
                        if ifaces.contains_key("org.bluez.Device1") {
                            if let Ok(d) = proxy(path.as_str(), "org.bluez.Device1").await {
                                let _ = d.call::<_, _, ()>("CancelPairing", &()).await;
                            }
                        }
                    }
                }
            });
        }
        None => {}
    }
    let _ = app.emit("kompact-bluetooth-asked", id);
    Ok(())
}

/// Serve the agent, register it as the default, and turn BlueZ's signals into
/// `kompact-bluetooth-changed` (at most every 700 ms).
pub fn start(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        use futures_util::StreamExt;
        let conn = match bus().await {
            Ok(c) => c,
            Err(e) => return warn!("bluetooth: {e}; no Bluetooth"),
        };
        if objects().await.ok().and_then(|o| adapter_of(&o)).is_none() {
            info!("bluetooth: no adapter (BlueZ not running, or no Bluetooth here)");
        }
        match conn.object_server().at(AGENT_PATH, Agent { app: app.clone() }).await {
            Ok(_) => match proxy("/org/bluez", "org.bluez.AgentManager1").await {
                Ok(m) => {
                    let path = ObjectPath::try_from(AGENT_PATH).expect("agent path");
                    let registered = m.call::<_, _, ()>("RegisterAgent", &(&path, "KeyboardDisplay")).await;
                    let default = m.call::<_, _, ()>("RequestDefaultAgent", &(&path,)).await;
                    match (registered, default) {
                        (Ok(()), Ok(())) => info!("bluetooth: pairing agent registered (default)"),
                        (r, d) => warn!("bluetooth: pairing agent: register {r:?}, default {d:?}"),
                    }
                }
                Err(e) => warn!("bluetooth: no agent manager: {e}"),
            },
            Err(e) => warn!("bluetooth: can't serve the agent: {e}"),
        }
        let rule = match zbus::MatchRule::builder().msg_type(zbus::message::Type::Signal).sender(BLUEZ).map(|b| b.build()) {
            Ok(r) => r,
            Err(e) => return warn!("bluetooth: {e}"),
        };
        let mut stream = match zbus::MessageStream::for_match_rule(rule, &conn, None).await {
            Ok(s) => s,
            Err(e) => return warn!("bluetooth: can't watch BlueZ: {e}"),
        };
        info!("bluetooth: watching BlueZ");
        // fix/bluetooth: the devices you had connected, back (after login or a restart).
        tauri::async_runtime::spawn(async {
            tokio::time::sleep(Duration::from_secs(4)).await;
            reconnect("the shell started").await;
        });
        let agent = Agent { app: app.clone() };
        let mut powered = objects().await.ok().and_then(|o| adapter_powered(&o));
        let mut last = std::time::Instant::now() - Duration::from_secs(1);
        let pending = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        while stream.next().await.is_some() {
            // A pairing that ended takes its "type this code" question with it.
            agent.done_shows_if_paired().await;
            if last.elapsed() >= Duration::from_millis(700) {
                last = std::time::Instant::now();
                let _ = app.emit("kompact-bluetooth-changed", ());
                // fix/bluetooth: Bluetooth switched on: reconnect; and whatever connects (even
                // by itself) is reconnected next time.
                if let Ok(objs) = objects().await {
                    let now = adapter_powered(&objs);
                    if powered == Some(false) && now == Some(true) {
                        tauri::async_runtime::spawn(async {
                            tokio::time::sleep(Duration::from_secs(2)).await;
                            reconnect("Bluetooth switched on").await;
                        });
                    }
                    powered = now;
                    if let Some(adapter) = adapter_of(&objs) {
                        for d in devices_of(&objs, &adapter).iter().filter(|d| d.connected && d.paired) {
                            remember_address(&d.address, true);
                        }
                    }
                }
            } else if !pending.swap(true, std::sync::atomic::Ordering::Relaxed) {
                let (app, pending) = (app.clone(), pending.clone());
                tauri::async_runtime::spawn(async move {
                    tokio::time::sleep(Duration::from_millis(700)).await;
                    pending.store(false, std::sync::atomic::Ordering::Relaxed);
                    let _ = app.emit("kompact-bluetooth-changed", ());
                });
            }
        }
        warn!("bluetooth: BlueZ's signals stopped");
    });
}

impl Agent {
    /// "Type this code" questions go once their device is paired.
    async fn done_shows_if_paired(&self) {
        let shows: Vec<String> = with_asks(|a| a.open.iter().filter(|(q, tx)| tx.is_none() && q.kind == "show").map(|(q, _)| q.device.clone()).collect());
        if shows.is_empty() {
            return;
        }
        let Ok(objs) = objects().await else { return };
        let Some(adapter) = adapter_of(&objs) else { return };
        if devices_of(&objs, &adapter).iter().any(|d| d.paired && shows.contains(&d.name)) {
            self.done_shows();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dev(name: &str, paired: bool, connected: bool, rssi: Option<i16>) -> Device {
        Device { path: format!("/org/bluez/hci0/dev_{name}"), address: "AA:BB".into(), name: name.into(), kind: "device", paired, connected, battery: None, rssi }
    }

    #[test]
    fn kinds() {
        assert_eq!(kind("audio-headset"), "headphones");
        assert_eq!(kind("audio-headphones"), "headphones");
        assert_eq!(kind("audio-card"), "speaker");
        assert_eq!(kind("input-mouse"), "mouse");
        assert_eq!(kind("input-keyboard"), "keyboard");
        assert_eq!(kind("phone"), "phone");
        assert_eq!(kind(""), "device");
    }

    #[test]
    fn codes() {
        assert_eq!(passkey_text(42), "000042");
        assert_eq!(passkey_text(123456), "123456");
        assert_eq!(parse_answer("passkey", " 012345 "), Ok(Answer::Passkey(12345)));
        assert!(parse_answer("passkey", "1234567").is_err());
        assert!(parse_answer("passkey", "12a").is_err());
        assert_eq!(parse_answer("pin", "0000"), Ok(Answer::Pin("0000".into())));
        assert!(parse_answer("pin", "").is_err());
        assert_eq!(parse_answer("confirm", ""), Ok(Answer::Yes));
    }

    #[test]
    fn device_order() {
        let list = sort_devices(vec![
            dev("Zed", false, false, Some(-40)),
            dev("Mouse", true, false, None),
            dev("Buds", true, true, None),
            dev("Alpha", false, false, Some(-80)),
            dev("AA:BB", false, false, Some(-30)),
        ]);
        let names: Vec<&str> = list.iter().map(|d| d.name.as_str()).collect();
        assert_eq!(names, ["Buds", "Mouse", "Zed", "Alpha"], "paired (connected first), then nearby by signal; nameless dropped");
    }

    /// Read-only, against this machine's BlueZ: `cargo test bt_live -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn bt_live() {
        let state = tauri::async_runtime::block_on(bluetooth_state()).expect("bluetooth state");
        println!("{}", serde_json::to_string_pretty(&state).unwrap());
    }

    #[test]
    fn what_went_wrong() {
        // From this PC's logs, 2026-10-25.
        assert_eq!(classify("Failed", "br-connection-key-missing"), Problem::KeyMissing);
        assert_eq!(classify("Failed", "Page Timeout"), Problem::NoAnswer);
        assert_eq!(classify("Failed", "br-connection-page-timeout"), Problem::NoAnswer);
        assert_eq!(classify("AuthenticationTimeout", ""), Problem::NoAnswer);
        assert_eq!(classify("ConnectionAttemptFailed", "Page Timeout"), Problem::NoAnswer);
        assert_eq!(classify("Failed", "br-connection-profile-unavailable"), Problem::NoProfile);
        assert_eq!(classify("AuthenticationFailed", ""), Problem::Refused);
        assert_eq!(classify("Failed", "br-connection-refused"), Problem::Refused);
        assert_eq!(classify("InProgress", "In Progress"), Problem::Busy);
        assert_eq!(classify("AlreadyExists", "Already Exists"), Problem::AlreadyPaired);
        assert_eq!(classify("Failed", "something new"), Problem::Other);
        assert!(explain(Problem::KeyMissing, "OnePlus Nord Buds 2r", "").starts_with("key-missing: OnePlus Nord Buds 2r has forgotten this computer"), "the page offers Pair Again");
        assert!(explain(Problem::NoAnswer, "Buds", "").contains("pairing mode"));
        assert!(retry_helps(Problem::NoAnswer) && !retry_helps(Problem::KeyMissing) && !retry_helps(Problem::Refused));
    }

    #[test]
    fn reconnecting() {
        let list = parse_remembered("0b:ef:24:8f:e2:cb\nnot an address\n0B:EF:24:8F:E2:CB\nAA:BB:CC:DD:EE:FF\n");
        assert_eq!(list, ["0B:EF:24:8F:E2:CB", "AA:BB:CC:DD:EE:FF"], "valid, once, upper case");
        let d = |addr: &str, paired, connected| Device { path: format!("/org/bluez/hci0/dev_{addr}"), address: addr.into(), name: addr.into(), kind: "headphones", paired, connected, battery: None, rssi: None };
        let devices = vec![d("0B:EF:24:8F:E2:CB", true, false), d("AA:BB:CC:DD:EE:FF", true, true), d("11:22:33:44:55:66", true, false)];
        let todo: Vec<&str> = to_reconnect(&list, &devices).iter().map(|x| x.address.as_str()).collect();
        assert_eq!(todo, ["0B:EF:24:8F:E2:CB"], "remembered, paired, not connected; a device never connected here isn't");
    }

    #[test]
    fn only_bluez_devices() {
        assert!(device_path("/org/bluez/hci0/dev_AA_BB").is_ok());
        assert!(device_path("/org/freedesktop/NetworkManager").is_err());
        assert!(device_path("/org/bluez/bad path").is_err());
    }
}
