//! 19h-2: Siva speaks his replies. Piper (a local text-to-speech program, fetched once with its
//! British English "Alan" voice by `shell/tools/get-speaking-voice.sh` into
//! `~/.local/share/kompact/piper/`) turns the reply into audio in memory (~0.4 s for a sentence on
//! the Ryzen), `pw-play` plays it on the default output, and the HUD's wave and the menu bar's
//! bars follow its loudness (`kompact-assistant-level`, timed to the playback). Speaking stops at
//! once when you start talking again or close the HUD (`hush`).
//!
//! When: `~/.config/kompact/assistant` `speak=voice|always|never` (voice, the default: replies
//! to what you said, not to what you typed). The text and the timeline are pure and tested.

use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde::Serialize;
use tauri::{AppHandle, Emitter};
use tracing::{info, warn};

pub const VOICE: &str = "en_GB-alan-medium";
/// Piper's "medium" voices.
pub const RATE: usize = 22_050;
/// One step of the wave while speaking (80 ms, as while listening).
pub const STEP_MS: usize = 80;
/// The speakers (a Bluetooth headset most of all) play a little after the audio is handed over.
pub const LATENCY_MS: u64 = 150;
/// Longer replies are cut to their first sentences (the HUD shows all of it).
pub const MAX_CHARS: usize = 320;

// ---- when to speak (pure) ----

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Speak {
    /// Replies to what you said (not to what you typed).
    Voice,
    Always,
    Never,
}

impl Speak {
    pub fn parse(s: &str) -> Option<Speak> {
        match s.trim() {
            "voice" => Some(Speak::Voice),
            "always" => Some(Speak::Always),
            "never" => Some(Speak::Never),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Speak::Voice => "voice",
            Speak::Always => "always",
            Speak::Never => "never",
        }
    }

    /// Whether a reply is spoken, for a request you said (`spoken`) or typed.
    pub fn says(self, spoken: bool) -> bool {
        match self {
            Speak::Always => true,
            Speak::Voice => spoken,
            Speak::Never => false,
        }
    }
}

/// `speak=…` from the settings file; anything else keeps the default.
pub fn parse_prefs(text: &str) -> Speak {
    text.lines().filter_map(|l| l.trim().strip_prefix("speak=")).filter_map(Speak::parse).last().unwrap_or(Speak::Voice)
}

pub fn prefs_text(speak: Speak) -> String {
    format!("# kompact's assistant (written by System Settings → Assistant)\nspeak={}\n", speak.name())
}

// ---- what is said (pure) ----

/// The reply as Piper should read it: quotes and symbols it would spell out dropped, "%" as a
/// word, at most `MAX_CHARS` (whole sentences when it can). None when nothing's left to say.
pub fn speakable(text: &str) -> Option<String> {
    let mut out = String::new();
    for c in text.chars() {
        match c {
            '“' | '”' | '"' | '‘' => {}
            '’' => out.push('\''),
            '%' => out.push_str(" percent"),
            '→' => out.push_str(", then "),
            '…' => out.push_str("..."),
            '&' => out.push_str(" and "),
            c if c.is_alphanumeric() || c.is_whitespace() || ".,;:!?'-()/".contains(c) => out.push(c),
            _ => out.push(' '),
        }
    }
    let mut text = out.split_whitespace().collect::<Vec<_>>().join(" ").replace(" ,", ",");
    if text.chars().count() > MAX_CHARS {
        let cut: String = text.chars().take(MAX_CHARS).collect();
        text = match cut.rfind(['.', '!', '?']) {
            Some(i) if i > 40 => cut[..=i].to_string(),
            _ => format!("{}…", cut.trim_end()),
        };
    }
    text.chars().any(char::is_alphanumeric).then_some(text)
}

/// The loudness (0–1, as the wave shows it) of each `STEP_MS` of 16-bit audio at `RATE`.
pub fn timeline(pcm: &[u8]) -> Vec<f64> {
    let step = RATE * STEP_MS / 1000 * 2;
    pcm.chunks(step).map(|c| super::voice::display_level(super::voice::loudness(c) * 1.4)).collect()
}

// ---- Piper and pw-play ----

fn data_home() -> Option<PathBuf> {
    std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local").join("share")))
}

/// Piper and the voice, if both are here.
pub fn installed() -> Option<(PathBuf, PathBuf)> {
    let dir = data_home()?.join("kompact").join("piper");
    let piper = dir.join("piper").join("piper");
    let voice = dir.join("voices").join(format!("{VOICE}.onnx"));
    (piper.is_file() && voice.is_file() && voice.with_extension("onnx.json").is_file()).then_some((piper, voice))
}

fn prefs_file() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
    Some(base.join("kompact").join("assistant"))
}

pub fn speak_pref() -> Speak {
    prefs_file().and_then(|f| std::fs::read_to_string(f).ok()).map(|t| parse_prefs(&t)).unwrap_or(Speak::Voice)
}

/// Each `say` gets a number; a newer one (or `hush`) makes an older one stop.
static TURN: AtomicU64 = AtomicU64::new(0);
/// The pw-play speaking now (0: none).
static PLAYING: Mutex<u32> = Mutex::new(0);

/// 20c: replies waiting to be said (tasks finish at their own time; each waits its turn).
static QUEUE: Mutex<std::collections::VecDeque<String>> = Mutex::new(std::collections::VecDeque::new());
/// At most this many waiting; older ones are dropped (they're in the HUD anyway).
const QUEUED: usize = 3;
/// Whether the speaking thread is running.
static SPEAKER: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Stop speaking now, and forget what was waiting to be said.
pub fn hush() {
    QUEUE.lock().unwrap_or_else(|e| e.into_inner()).clear();
    TURN.fetch_add(1, Ordering::Relaxed);
    let pid = std::mem::take(&mut *PLAYING.lock().unwrap_or_else(|e| e.into_inner()));
    if pid != 0 {
        unsafe {
            libc::kill(pid as i32, libc::SIGTERM);
        }
        info!("speech: hushed");
    }
}

/// Say `text` (off the main thread; returns at once), after whatever is being said (20c). The
/// HUD hears `kompact-assistant` `speaking` and then `spoken`, and the wave follows.
pub fn say(app: &AppHandle, text: &str) {
    let Some(text) = speakable(text) else { return };
    if installed().is_none() {
        return;
    }
    {
        let mut q = QUEUE.lock().unwrap_or_else(|e| e.into_inner());
        q.push_back(text);
        while q.len() > QUEUED {
            q.pop_front();
        }
    }
    if SPEAKER.swap(true, Ordering::Relaxed) {
        return; // the speaking thread takes it next
    }
    let app = app.clone();
    std::thread::spawn(move || loop {
        loop {
            let next = QUEUE.lock().unwrap_or_else(|e| e.into_inner()).pop_front();
            let Some(text) = next else { break };
            let Some((piper, voice)) = installed() else { break };
            let turn = TURN.load(Ordering::Relaxed);
            if let Err(e) = speak(&app, &piper, &voice, &text, turn) {
                warn!("speech: {e}");
            }
            if TURN.load(Ordering::Relaxed) == turn {
                *PLAYING.lock().unwrap_or_else(|e| e.into_inner()) = 0;
            }
            let _ = app.emit("kompact-assistant-level", 0.0);
            let _ = app.emit("kompact-assistant", serde_json::json!({"state": "spoken", "detail": ""}));
        }
        SPEAKER.store(false, Ordering::Relaxed);
        // A reply queued between the last look and now: take it on, unless another thread did.
        if QUEUE.lock().unwrap_or_else(|e| e.into_inner()).is_empty() || SPEAKER.swap(true, Ordering::Relaxed) {
            break;
        }
    });
}

/// pw-play for 16-bit mono audio at `rate`, reading its stdin.
fn player(rate: usize) -> Result<std::process::Child, String> {
    Command::new("pw-play")
        .args(["--raw", "--rate", &rate.to_string(), "--channels", "1", "--format", "s16", "--media-role", "Assistant"])
        .args(["-P", "{ application.name = \"Siva\" node.description = \"Siva\" }", "-"])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("can't start pw-play: {e}"))
}

/// Piper's audio goes to pw-play as it comes (sentence by sentence: the first one is heard
/// while the rest is still being made), and the wave follows in time with it.
fn speak(app: &AppHandle, piper: &PathBuf, voice: &PathBuf, text: &str, turn: u64) -> Result<(), String> {
    use std::io::Read;
    let started = Instant::now();
    let mut child = Command::new(piper)
        .arg("--model")
        .arg(voice)
        .args(["--output_raw", "--sentence_silence", "0.15"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("can't start piper: {e}"))?;
    child.stdin.take().ok_or("no piper input")?.write_all(text.as_bytes()).map_err(|e| e.to_string())?;
    let mut from_piper = child.stdout.take().ok_or("no piper output")?;
    let mut play = player(RATE)?;
    *PLAYING.lock().unwrap_or_else(|e| e.into_inner()) = play.id();
    let mut to_play = play.stdin.take().ok_or("no pw-play input")?;
    // The wave: each step's level, paced by the clock from the first sound handed over.
    let (levels, paced) = std::sync::mpsc::channel::<f64>();
    let waver = {
        let app = app.clone();
        std::thread::spawn(move || {
            let mut begin = None;
            for (i, level) in paced.iter().enumerate() {
                let begin = *begin.get_or_insert_with(|| Instant::now() + Duration::from_millis(LATENCY_MS));
                if let Some(wait) = (begin + Duration::from_millis((i * STEP_MS) as u64)).checked_duration_since(Instant::now()) {
                    std::thread::sleep(wait);
                }
                if TURN.load(Ordering::Relaxed) != turn {
                    break;
                }
                let _ = app.emit("kompact-assistant-level", level);
            }
        })
    };
    let step = RATE * STEP_MS / 1000 * 2;
    let (mut pending, mut buf, mut total, mut first) = (Vec::with_capacity(step * 2), [0u8; 8192], 0usize, None);
    loop {
        let n = match from_piper.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => n,
        };
        if TURN.load(Ordering::Relaxed) != turn {
            break; // hushed
        }
        if first.is_none() {
            first = Some(started.elapsed().as_millis());
            let _ = app.emit("kompact-assistant", serde_json::json!({"state": "speaking", "detail": text}));
        }
        if to_play.write_all(&buf[..n]).is_err() {
            break; // pw-play was stopped
        }
        total += n;
        pending.extend_from_slice(&buf[..n]);
        while pending.len() >= step {
            let _ = levels.send(timeline(&pending[..step])[0]);
            pending.drain(..step);
        }
    }
    if !pending.is_empty() {
        let _ = levels.send(timeline(&pending)[0]);
    }
    drop(levels);
    drop(to_play);
    let _ = child.kill();
    let _ = child.wait();
    let _ = play.wait();
    let _ = waver.join();
    match first {
        Some(ms) => info!("speech: {:.1} s of speech, first sound after {ms} ms: {text:?}", total as f64 / 2.0 / RATE as f64),
        None if TURN.load(Ordering::Relaxed) == turn => return Err("piper made no sound".into()),
        None => {}
    }
    Ok(())
}

// ---- the "speak now" chime ----

/// Two soft rising notes (A5 then E6, 70 ms each with a smooth swell), quiet: the cue that the
/// microphone is on.
pub fn chime() -> Vec<u8> {
    let note = |hz: f64, ms: usize| -> Vec<i16> {
        let n = RATE * ms / 1000;
        (0..n)
            .map(|i| {
                let t = i as f64 / RATE as f64;
                let envelope = (std::f64::consts::PI * i as f64 / n as f64).sin().powi(2);
                (6000.0 * envelope * (2.0 * std::f64::consts::PI * hz * t).sin()) as i16
            })
            .collect()
    };
    [note(880.0, 70), note(1318.5, 90)].concat().into_iter().flat_map(i16::to_le_bytes).collect()
}

/// Play `chime` (returns at once).
pub fn play_chime() {
    std::thread::spawn(|| {
        let Ok(mut play) = player(RATE) else { return };
        if let Some(mut input) = play.stdin.take() {
            let _ = input.write_all(&chime());
        }
        let _ = play.wait();
    });
}

// ---- the page's commands ----

#[derive(Debug, Clone, Serialize)]
pub struct SpeechState {
    pub ready: bool,
    pub voice: &'static str,
    pub speak: Speak,
}

#[tauri::command]
pub fn speech_state() -> SpeechState {
    SpeechState { ready: installed().is_some(), voice: VOICE, speak: speak_pref() }
}

#[tauri::command]
pub fn set_speak(speak: String) -> Result<SpeechState, String> {
    let s = Speak::parse(&speak).ok_or_else(|| format!("{speak:?} isn't voice, always or never"))?;
    let file = prefs_file().ok_or("no home folder")?;
    if let Some(dir) = file.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    std::fs::write(&file, prefs_text(s)).map_err(|e| format!("can't save {}: {e}", file.display()))?;
    info!("speech: speak {}", s.name());
    if s == Speak::Never {
        hush();
    }
    Ok(speech_state())
}

#[tauri::command]
pub fn speech_hush() {
    hush();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn when_to_speak() {
        assert_eq!(parse_prefs(""), Speak::Voice, "by default: replies to what you said");
        assert_eq!(parse_prefs("speak=never\n"), Speak::Never);
        assert_eq!(parse_prefs("# x\nspeak=always\nspeak=bogus\n"), Speak::Always);
        assert_eq!(parse_prefs(&prefs_text(Speak::Never)), Speak::Never);
        assert!(Speak::Voice.says(true) && !Speak::Voice.says(false));
        assert!(Speak::Always.says(false));
        assert!(!Speak::Never.says(true));
    }

    #[test]
    fn what_is_read_aloud() {
        assert_eq!(speakable("Volume is at 30%.").as_deref(), Some("Volume is at 30 percent."));
        assert_eq!(speakable("I can't find a window for “files”.").as_deref(), Some("I can't find a window for files."));
        assert_eq!(speakable("Is another microphone right (Settings → Sound)?").as_deref(), Some("Is another microphone right (Settings, then Sound)?"));
        assert_eq!(speakable("✦ … ✦"), None);
        assert_eq!(speakable(""), None);
        let long = "This is one sentence that goes on for a while. ".repeat(12);
        let s = speakable(&long).unwrap();
        assert!(s.chars().count() <= MAX_CHARS && s.ends_with('.'), "cut at a sentence: {s}");
    }

    #[test]
    fn the_chime() {
        let c = chime();
        assert_eq!(c.len(), (RATE * 70 / 1000 + RATE * 90 / 1000) * 2, "70 + 90 ms");
        let samples: Vec<i16> = c.chunks_exact(2).map(|b| i16::from_le_bytes([b[0], b[1]])).collect();
        let peak = samples.iter().map(|s| s.unsigned_abs()).max().unwrap();
        assert!((5000..=6000).contains(&peak), "soft: {peak}");
        assert!(samples[0].abs() < 50 && samples.last().unwrap().abs() < 50, "no click at either end");
    }

    #[test]
    fn the_wave_follows_the_speech() {
        let step = RATE * STEP_MS / 1000;
        let quiet = vec![0u8; step * 2];
        let loud: Vec<u8> = (0..step).flat_map(|i| (if i % 2 == 0 { 8000i16 } else { -8000 }).to_le_bytes()).collect();
        let t = timeline(&[quiet.clone(), loud, quiet].concat());
        assert_eq!(t.len(), 3);
        assert_eq!(t[0], 0.0);
        assert!(t[1] > 0.9);
        assert_eq!(t[2], 0.0);
    }
}
