//! 12c: sound, through PipeWire's own tools (no PulseAudio `pactl` here): `pw-dump` lists the
//! outputs and inputs, `wpctl` reads and sets the volume (its 0–1 scale, the one it shows) and
//! picks the default devices. For the menu bar's sound item and menu, System Settings → Sound,
//! and kompact's volume keys (`sound.volume-up` …, which come as shell keys), each of which shows
//! the level on screen (the `osd` window, `kompact-osd`).
//!
//! `pw-dump --monitor` prints whenever PipeWire changes; that (debounced) becomes
//! `kompact-sound-changed`, and pages fetch again.

use std::process::{Command, Stdio};
use std::sync::Mutex;
use std::time::Duration;

use serde::Serialize;
use serde_json::{json, Value};
use tauri::{AppHandle, Emitter, Manager};
use tracing::{info, warn};

/// One key press moves the volume this much (of 1).
pub const STEP: f32 = 0.0625;

// ---- The pure part ----

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Node {
    pub id: u32,
    pub name: String,
    /// What it's called (the device's description), and its own name for itself (the HDMI
    /// one's is the TV's).
    pub label: String,
    pub nick: Option<String>,
    pub output: bool,
}

/// Outputs (`Audio/Sink`) and inputs (`Audio/Source`) in a `pw-dump`.
pub fn nodes(dump: &Value) -> Vec<Node> {
    let mut out = Vec::new();
    for o in dump.as_array().into_iter().flatten() {
        let props = &o["info"]["props"];
        let output = match props["media.class"].as_str() {
            Some("Audio/Sink") => true,
            Some("Audio/Source") => false,
            _ => continue,
        };
        let Some(id) = o["id"].as_u64() else { continue };
        let name = props["node.name"].as_str().unwrap_or("").to_string();
        let label = props["node.description"].as_str().or(props["node.nick"].as_str()).unwrap_or(&name).to_string();
        let nick = props["node.nick"].as_str().filter(|n| !label.contains(n)).map(str::to_string);
        out.push(Node { id: id as u32, name, label, nick, output });
    }
    out.sort_by(|a, b| b.output.cmp(&a.output).then(a.label.cmp(&b.label)));
    out
}

/// 12d: the apps recording now (running `Stream/Input/Audio` nodes), for the menu bar's orange
/// microphone dot. Peak meters and monitors of outputs aren't microphones; `stream.monitor` ones
/// are left out.
pub fn recording(dump: &Value) -> Vec<String> {
    let mut apps: Vec<String> = Vec::new();
    for o in dump.as_array().into_iter().flatten() {
        let info = &o["info"];
        let props = &info["props"];
        if props["media.class"].as_str() != Some("Stream/Input/Audio") || info["state"].as_str() != Some("running") {
            continue;
        }
        if props["stream.monitor"].as_bool() == Some(true) || props["stream.capture.sink"].as_bool() == Some(true) {
            continue;
        }
        let name = props["application.name"].as_str().or(props["node.name"].as_str()).unwrap_or("An app").to_string();
        if !apps.contains(&name) {
            apps.push(name);
        }
    }
    apps
}

/// 13d: the apps playing sound now (running `Stream/Output/Audio` nodes), for the menu bar's
/// speaker shown When Active.
pub fn playing(dump: &Value) -> Vec<String> {
    let mut apps: Vec<String> = Vec::new();
    for o in dump.as_array().into_iter().flatten() {
        let info = &o["info"];
        let props = &info["props"];
        if props["media.class"].as_str() != Some("Stream/Output/Audio") || info["state"].as_str() != Some("running") {
            continue;
        }
        let name = props["application.name"].as_str().or(props["node.name"].as_str()).unwrap_or("An app").to_string();
        if !apps.contains(&name) {
            apps.push(name);
        }
    }
    apps
}

/// `wpctl get-volume`: "Volume: 0.40" or "Volume: 0.40 [MUTED]".
pub fn parse_volume(text: &str) -> Option<(f32, bool)> {
    let rest = text.trim().strip_prefix("Volume:")?.trim();
    let value: f32 = rest.split_whitespace().next()?.parse().ok()?;
    Some((value, rest.contains("[MUTED]")))
}

/// `wpctl inspect`: its first line, "id 57, type PipeWire:Interface:Node".
pub fn parse_inspect_id(text: &str) -> Option<u32> {
    text.lines().next()?.trim().strip_prefix("id ")?.split(',').next()?.trim().parse().ok()
}

/// The new volume after a key: up or down by `STEP`, kept within 0–1 (on a step, so the
/// on-screen level has whole segments).
pub fn stepped(volume: f32, up: bool) -> f32 {
    let steps = (volume / STEP).round();
    let next = if up { steps + 1.0 } else { steps - 1.0 };
    (next * STEP).clamp(0.0, 1.0)
}

/// The icon for a level: 0 (muted or silent) to 3 waves.
pub fn waves(volume: f32, muted: bool) -> u8 {
    if muted || volume <= 0.0 {
        0
    } else if volume < 0.34 {
        1
    } else if volume < 0.67 {
        2
    } else {
        3
    }
}

// ---- PipeWire ----

fn run(args: &[&str]) -> Result<String, String> {
    let out = Command::new(args[0]).args(&args[1..]).stderr(Stdio::piped()).output().map_err(|e| format!("{}: {e}", args[0]))?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
        return Err(if err.is_empty() { format!("{} failed", args.join(" ")) } else { err });
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// `@DEFAULT_AUDIO_SINK@` / `@DEFAULT_AUDIO_SOURCE@`.
fn target(output: bool) -> &'static str {
    if output { "@DEFAULT_AUDIO_SINK@" } else { "@DEFAULT_AUDIO_SOURCE@" }
}

fn volume_of(output: bool) -> Option<(f32, bool)> {
    run(&["wpctl", "get-volume", target(output)]).ok().as_deref().and_then(parse_volume)
}

fn default_id(output: bool) -> Option<u32> {
    run(&["wpctl", "inspect", target(output)]).ok().as_deref().and_then(parse_inspect_id)
}

fn state_now() -> Result<Value, String> {
    let dump: Value = serde_json::from_str(&run(&["pw-dump", "-N"])?).map_err(|e| format!("pw-dump: {e}"))?;
    let all = nodes(&dump);
    let side = |output: bool| {
        let devices: Vec<&Node> = all.iter().filter(|n| n.output == output).collect();
        // With no input chosen WirePlumber falls back to an output's monitor (not a microphone):
        // then there's no current input, and no input level to show.
        let current = default_id(output).filter(|id| devices.iter().any(|d| d.id == *id));
        let (volume, muted) = current.and_then(|_| volume_of(output)).unwrap_or((0.0, false));
        json!({
            "devices": devices,
            "current": current,
            "volume": volume,
            "muted": muted,
        })
    };
    let (out, inp) = (side(true), side(false));
    let waves = waves(out["volume"].as_f64().unwrap_or(0.0) as f32, out["muted"].as_bool().unwrap_or(false));
    Ok(json!({"output": out, "input": inp, "waves": waves}))
}

async fn blocking<T: Send + 'static>(f: impl FnOnce() -> Result<T, String> + Send + 'static) -> Result<T, String> {
    tauri::async_runtime::spawn_blocking(f).await.map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn sound_state() -> Result<Value, String> {
    blocking(state_now).await
}

/// The menu bar's speaker (the output's level) and (12d) who is recording.
#[tauri::command]
pub async fn sound_level() -> Result<Value, String> {
    blocking(|| {
        let dump = run(&["pw-dump", "-N"]).ok().and_then(|t| serde_json::from_str::<Value>(&t).ok());
        let recording = dump.as_ref().map(recording).unwrap_or_default();
        let playing = dump.as_ref().map(playing).unwrap_or_default();
        Ok(match volume_of(true) {
            Some((volume, muted)) => json!({"output": true, "volume": volume, "muted": muted, "waves": waves(volume, muted), "recording": recording, "playing": playing}),
            None => json!({"output": false, "recording": recording, "playing": playing}),
        })
    })
    .await
}

/// The output's (or input's) volume, 0–1; setting it also unmutes, as on macOS.
#[tauri::command]
pub async fn sound_volume(output: bool, volume: f32) -> Result<(), String> {
    let v = format!("{:.3}", volume.clamp(0.0, 1.0));
    blocking(move || {
        run(&["wpctl", "set-volume", target(output), &v])?;
        let _ = run(&["wpctl", "set-mute", target(output), "0"]);
        Ok(())
    })
    .await
}

#[tauri::command]
pub async fn sound_mute(output: bool, mute: bool) -> Result<(), String> {
    blocking(move || run(&["wpctl", "set-mute", target(output), if mute { "1" } else { "0" }]).map(|_| ())).await
}

/// Make `id` the default output or input.
#[tauri::command]
pub async fn sound_default(id: u32) -> Result<(), String> {
    blocking(move || {
        run(&["wpctl", "set-default", &id.to_string()])?;
        info!("sound: default device now {id}");
        Ok(())
    })
    .await
}

// ---- The volume keys and the on-screen level ----

/// What the on-screen level shows last (for the page when it loads).
static OSD: Mutex<Option<Value>> = Mutex::new(None);

/// A volume key from kompact: change it, then show the level.
pub fn key(app: &AppHandle, action: &str) {
    let (app, action) = (app.clone(), action.to_string());
    tauri::async_runtime::spawn_blocking(move || {
        let output = action != "mic-mute";
        if !output {
            // No microphone chosen: the fallback is an output's monitor, not a microphone.
            let inputs = run(&["pw-dump", "-N"]).ok().and_then(|t| serde_json::from_str::<Value>(&t).ok()).map(|d| nodes(&d)).unwrap_or_default();
            if !default_id(false).is_some_and(|id| inputs.iter().any(|n| !n.output && n.id == id)) {
                return info!("sound: mic-mute: no microphone chosen (Settings → Sound)");
            }
        }
        let Some((volume, muted)) = volume_of(output) else {
            return warn!("sound: no default {} (is PipeWire running?)", if output { "output" } else { "input" });
        };
        let (volume, muted) = match action.as_str() {
            "volume-up" | "volume-down" => {
                let v = stepped(volume, action == "volume-up");
                let _ = run(&["wpctl", "set-volume", target(true), &format!("{v:.4}")]);
                let _ = run(&["wpctl", "set-mute", target(true), "0"]);
                (v, false)
            }
            _ => {
                let _ = run(&["wpctl", "set-mute", target(output), "toggle"]);
                (volume, !muted)
            }
        };
        info!("sound: {action} → {:.0}%{}", volume * 100.0, if muted { " (muted)" } else { "" });
        let osd = json!({"kind": if output { "volume" } else { "mic" }, "volume": volume, "muted": muted, "waves": waves(volume, muted)});
        *OSD.lock().unwrap_or_else(|e| e.into_inner()) = Some(osd.clone());
        let handle = app.clone();
        let _ = app.run_on_main_thread(move || {
            if let Some(w) = handle.get_webview_window(OSD_LABEL) {
                let _ = w.show();
            }
        });
        let _ = app.emit("kompact-osd", osd);
        let _ = app.emit("kompact-sound-changed", ());
    });
}

pub const OSD_LABEL: &str = "osd";

#[tauri::command]
pub fn osd_state() -> Option<Value> {
    OSD.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

/// The page's timer ran out: hide the window (unmapped, it costs nothing).
#[tauri::command]
pub fn osd_hide(app: AppHandle) {
    if let Some(w) = app.get_webview_window(OSD_LABEL) {
        let _ = w.hide();
    }
}

/// `pw-dump --monitor` → `kompact-sound-changed` (at most every 500 ms).
pub fn start(app: AppHandle) {
    std::thread::Builder::new()
        .name("sound".into())
        .spawn(move || {
            use std::io::BufRead;
            let child = Command::new("pw-dump").args(["--monitor", "-N"]).stdout(Stdio::piped()).stderr(Stdio::null()).spawn();
            let mut child = match child {
                Ok(c) => c,
                Err(e) => return warn!("sound: no pw-dump ({e}); the sound item won't follow changes made elsewhere"),
            };
            info!("sound: watching PipeWire");
            let Some(stdout) = child.stdout.take() else { return };
            let mut last = std::time::Instant::now() - Duration::from_secs(1);
            let pending = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
            for line in std::io::BufReader::new(stdout).lines() {
                if line.is_err() {
                    break;
                }
                if last.elapsed() >= Duration::from_millis(500) {
                    last = std::time::Instant::now();
                    let _ = app.emit("kompact-sound-changed", ());
                } else if !pending.swap(true, std::sync::atomic::Ordering::Relaxed) {
                    let (app, pending) = (app.clone(), pending.clone());
                    std::thread::spawn(move || {
                        std::thread::sleep(Duration::from_millis(500));
                        pending.store(false, std::sync::atomic::Ordering::Relaxed);
                        let _ = app.emit("kompact-sound-changed", ());
                    });
                }
            }
            let _ = child.kill();
            warn!("sound: pw-dump stopped");
        })
        .expect("sound thread");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn devices_from_a_dump() {
        let dump = json!([
            {"id": 38, "info": {"props": {"media.class": "Audio/Sink", "node.name": "alsa_output.hdmi", "node.description": "GK208 HDMI/DP Audio Controller Digital Stereo (HDMI)", "node.nick": "2D HD LG TV"}}},
            {"id": 57, "info": {"props": {"media.class": "Audio/Sink", "node.name": "alsa_output.iec958", "node.description": "Starship/Matisse HD Audio Controller Digital Stereo (IEC958)", "node.nick": "ALC887-VD Digital"}}},
            {"id": 44, "info": {"props": {"media.class": "Audio/Source", "node.name": "alsa_input.analog", "node.description": "Starship/Matisse HD Audio Controller Analog Stereo"}}},
            {"id": 70, "info": {"props": {"media.class": "Stream/Output/Audio", "node.name": "firefox"}}},
            {"id": 1, "type": "PipeWire:Interface:Core"}
        ]);
        let n = nodes(&dump);
        assert_eq!(n.iter().map(|n| n.id).collect::<Vec<_>>(), [38, 57, 44], "outputs first, by name; streams left out");
        assert_eq!(n[0].nick.as_deref(), Some("2D HD LG TV"));
        assert!(n[0].output && !n[2].output);
    }

    #[test]
    fn who_records() {
        let dump = json!([
            {"id": 80, "info": {"state": "running", "props": {"media.class": "Stream/Input/Audio", "application.name": "Firefox"}}},
            {"id": 81, "info": {"state": "running", "props": {"media.class": "Stream/Input/Audio", "application.name": "Firefox"}}},
            {"id": 82, "info": {"state": "idle", "props": {"media.class": "Stream/Input/Audio", "application.name": "OBS"}}},
            {"id": 83, "info": {"state": "running", "props": {"media.class": "Stream/Input/Audio", "application.name": "pavucontrol", "stream.monitor": true}}},
            {"id": 84, "info": {"state": "running", "props": {"media.class": "Stream/Output/Audio", "application.name": "Spotify"}}}
        ]);
        assert_eq!(recording(&dump), ["Firefox"], "running microphone streams only, once per app");
        assert!(recording(&json!([])).is_empty());
        assert_eq!(playing(&dump), ["Spotify"], "running output streams only");
    }

    #[test]
    fn wpctl_output() {
        assert_eq!(parse_volume("Volume: 0.40\n"), Some((0.4, false)));
        assert_eq!(parse_volume("Volume: 0.25 [MUTED]"), Some((0.25, true)));
        assert_eq!(parse_volume("nothing"), None);
        assert_eq!(parse_inspect_id("id 57, type PipeWire:Interface:Node\n  * node.name = …"), Some(57));
        assert_eq!(parse_inspect_id(""), None);
    }

    /// Read-only, against this machine's PipeWire: `cargo test sound_live -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn sound_live() {
        println!("{}", serde_json::to_string_pretty(&state_now().expect("sound state")).unwrap());
    }

    #[test]
    fn keys_step() {
        assert_eq!(stepped(0.40, true), 0.4375);
        assert_eq!(stepped(0.0, false), 0.0);
        assert_eq!(stepped(1.0, true), 1.0);
        assert_eq!(stepped(0.97, true), 1.0);
        assert_eq!([waves(0.5, true), waves(0.0, false), waves(0.2, false), waves(0.5, false), waves(0.9, false)], [0, 0, 1, 2, 3]);
    }
}
