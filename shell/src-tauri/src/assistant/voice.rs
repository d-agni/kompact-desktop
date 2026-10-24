//! 19d, 19h: Siva hears you. Click the menu-bar item (19d-2: Ctrl+Super+V) and speak: `pw-record`
//! takes the chosen microphone (`sound::microphone`) as 16 kHz mono into memory (its stream is
//! called "Siva", so the menu bar's microphone dot names him) until you've finished (`Endpoint`:
//! 0.6 s of quiet after speech), then the audio goes to whisper.cpp's `whisper-cli` on its
//! stdin, at low priority, and the text comes back. Nothing is written to disk and nothing leaves
//! this PC: whisper runs on the CPU with a model fetched once by `shell/tools/get-voice-model.sh`
//! into `~/.local/share/kompact/whisper/`.
//!
//! The audio and text handling is pure and tested; the rest runs the two programs.

use std::io::Read;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
use std::time::Instant;

use serde::Serialize;
use tracing::{info, warn};

pub const RATE: usize = 16_000;
/// Longer than this and recording stops by itself (and `whisper-cli` gets a short audio context).
pub const MAX_SECONDS: usize = 12;
/// A peak below this is no sound at all (a muted or wrong device gives exact zeros; this PC's
/// analog microphone peaks at ~180 in a quiet room).
pub const SILENT_PEAK: i16 = 8;
/// Quiet speech is made louder before whisper, up to this peak and at most `MAX_GAIN` times.
pub const TARGET_PEAK: f64 = 24_000.0;
pub const MAX_GAIN: f64 = 20.0;
/// The models looked for, the most accurate first.
pub const MODELS: [&str; 3] = ["small.en", "base.en", "tiny.en"];
/// Words he's likely to hear, so whisper spells them our way.
const PROMPT: &str = "Siva, volume, mute, unmute, Firefox, Files, terminal, desktop, Do Not Disturb, dark mode, light mode, side by side, Bluetooth, Wi-Fi, screenshot.";
/// Threads for whisper (the Ryzen 3 3100 has 8; the desktop and the sound keep the rest).
const THREADS: &str = "4";

// ---- pure ----

/// A WAV file of 16-bit mono samples at `RATE`, for `whisper-cli -f -`.
pub fn wav(pcm: &[u8]) -> Vec<u8> {
    let data = pcm.len() as u32;
    let mut out = Vec::with_capacity(44 + pcm.len());
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes()); // fmt chunk size
    out.extend_from_slice(&1u16.to_le_bytes()); // PCM
    out.extend_from_slice(&1u16.to_le_bytes()); // mono
    out.extend_from_slice(&(RATE as u32).to_le_bytes());
    out.extend_from_slice(&(RATE as u32 * 2).to_le_bytes()); // bytes a second
    out.extend_from_slice(&2u16.to_le_bytes()); // bytes a frame
    out.extend_from_slice(&16u16.to_le_bytes()); // bits
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data.to_le_bytes());
    out.extend_from_slice(pcm);
    out
}

/// How long `pcm` (16-bit mono at `RATE`) is.
pub fn millis(pcm: &[u8]) -> usize {
    pcm.len() / 2 * 1000 / RATE
}

/// The loudness (RMS) of 16-bit little-endian samples.
pub fn loudness(pcm: &[u8]) -> f64 {
    let n = pcm.len() / 2;
    if n == 0 {
        return 0.0;
    }
    let sum: f64 = pcm.chunks_exact(2).map(|c| f64::from(i16::from_le_bytes([c[0], c[1]])).powi(2)).sum();
    (sum / n as f64).sqrt()
}

/// The loudest sample.
pub fn peak(pcm: &[u8]) -> i16 {
    pcm.chunks_exact(2).map(|c| i16::from_le_bytes([c[0], c[1]]).saturating_abs()).max().unwrap_or(0)
}

/// `pcm` made louder so its peak reaches `TARGET_PEAK` (never more than `MAX_GAIN` times, never
/// quieter), and the gain used.
pub fn louder(pcm: &[u8]) -> (Vec<u8>, f64) {
    let p = f64::from(peak(pcm).max(1));
    let gain = (TARGET_PEAK / p).clamp(1.0, MAX_GAIN);
    let out = pcm
        .chunks_exact(2)
        .flat_map(|c| ((f64::from(i16::from_le_bytes([c[0], c[1]])) * gain).round().clamp(-32768.0, 32767.0) as i16).to_le_bytes())
        .collect();
    (out, gain)
}

/// whisper's audio context for `ms` of audio: 1500 is 30 s; a short one is much faster (a 4 s
/// command: 1.8 → 0.75 s here), but below 512 it starts to mishear.
pub fn audio_ctx(ms: usize) -> usize {
    (ms * 1500 / 30_000 + 100).clamp(512, 1500)
}

/// What whisper wrote, tidied: its notes ("[BLANK_AUDIO]", "(music)") out, one line, no
/// trailing full stop. None if nothing is left.
pub fn tidy(out: &str) -> Option<String> {
    let mut text = String::new();
    // Inside [..] or (..), and inside *..*.
    let mut depth = 0usize;
    let mut starred = false;
    for c in out.chars() {
        match c {
            '[' | '(' => depth += 1,
            ']' | ')' => depth = depth.saturating_sub(1),
            '*' => starred = !starred,
            _ if depth > 0 || starred => {}
            c if c.is_whitespace() => text.push(' '),
            c => text.push(c),
        }
    }
    let text = unrepeated(&text.split_whitespace().collect::<Vec<_>>().join(" "));
    let text = text.trim_end_matches(['.', '!']).trim().to_string();
    let words = text.chars().any(char::is_alphanumeric);
    words.then_some(text)
}

/// whisper sometimes writes the same sentence twice ("Why you care? Why you care?"): once.
fn unrepeated(text: &str) -> String {
    let parts: Vec<&str> = text.split(['.', '?', '!']).map(str::trim).filter(|p| !p.is_empty()).collect();
    let key = |p: &str| p.to_lowercase().replace(',', "");
    if parts.len() >= 2 && parts.iter().all(|p| key(p) == key(parts[0])) {
        parts[0].to_string()
    } else {
        text.to_string()
    }
}

// ---- the programs ----

fn data_home() -> Option<PathBuf> {
    std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local").join("share")))
}

/// The model to use and its name, the most accurate one present.
pub fn model() -> Option<(&'static str, PathBuf)> {
    let dir = data_home()?.join("kompact").join("whisper");
    MODELS.into_iter().map(|m| (m, dir.join(format!("ggml-{m}.bin")))).find(|(_, p)| p.is_file())
}

fn on_path(program: &str) -> bool {
    std::env::var_os("PATH").is_some_and(|paths| std::env::split_paths(&paths).any(|d| d.join(program).is_file()))
}

#[derive(Debug, Clone, Serialize)]
pub struct VoiceState {
    /// Both whisper-cli and a model are here.
    pub ready: bool,
    pub whisper: bool,
    pub model: Option<&'static str>,
    /// What to do, when not ready.
    pub missing: Option<String>,
}

pub fn state() -> VoiceState {
    let whisper = on_path("whisper-cli");
    let model = model().map(|(m, _)| m);
    let missing = match (whisper, model.is_some()) {
        (true, true) => None,
        (false, true) => Some("Install whisper.cpp: sudo apt install whisper.cpp".into()),
        (true, false) => Some("Fetch the speech model: shell/tools/get-voice-model.sh".into()),
        (false, false) => Some("Run shell/tools/get-voice-model.sh, then sudo apt install whisper.cpp".into()),
    };
    VoiceState { ready: missing.is_none(), whisper, model, missing }
}

// ---- knowing when you've finished (pure) ----

/// 20 ms of audio, the step the end-of-speech detection works in.
pub const FRAME: usize = RATE / 50;
/// Speech is louder than this (RMS) and than `NOISE_TIMES` the room's noise.
pub const MIN_SPEECH: f64 = 40.0;
pub const NOISE_TIMES: f64 = 4.0;
/// Loud frames in a row that start speech (60 ms: a click or a knock doesn't).
pub const START_FRAMES: usize = 3;
/// Quiet after speech that ends it (0.6 s: pauses between words are shorter; was 0.8 s until
/// 2026-10-24, which felt slow).
pub const END_FRAMES: usize = 30;
/// Frames right after the microphone wakes that don't count (the "speak now" chime plays then;
/// they're still kept and heard).
pub const GRACE_FRAMES: usize = 15;
/// With no speech this long after the microphone gives sound, it gives up (6 s).
pub const NO_SPEECH_FRAMES: usize = 300;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// No sound yet (a Bluetooth microphone switching to headset mode gives exact zeros).
    Waking,
    Waiting,
    Speaking,
    /// Speech, then quiet: done.
    Done,
    NoSpeech,
}

/// Follows the loudness of each frame: the room's noise (which only quiet frames move), when
/// speech starts, and when it has ended.
#[derive(Debug, Clone, Default)]
pub struct Endpoint {
    noise: Option<f64>,
    frames: usize,
    loud_run: usize,
    quiet_run: usize,
    started: bool,
    loudest: f64,
}

impl Endpoint {
    pub fn threshold(&self) -> f64 {
        (self.noise.unwrap_or(0.0) * NOISE_TIMES).max(MIN_SPEECH)
    }

    pub fn started(&self) -> bool {
        self.started
    }

    /// The room's noise as learnt (RMS).
    pub fn noise(&self) -> f64 {
        self.noise.unwrap_or(0.0)
    }

    /// The loudest frame so far (for the log, when nothing counted as speech).
    pub fn loudest(&self) -> f64 {
        self.loudest
    }

    /// One frame's RMS.
    pub fn feed(&mut self, rms: f64) -> Verdict {
        if self.noise.is_none() && rms < 0.5 {
            return Verdict::Waking;
        }
        self.frames += 1;
        self.loudest = self.loudest.max(rms);
        let threshold = self.threshold();
        let noise = self.noise.get_or_insert(rms);
        if rms > threshold {
            self.loud_run += 1;
            self.quiet_run = 0;
            if self.loud_run >= START_FRAMES {
                self.started = true;
            }
        } else {
            // Down at once, up slowly, so a steady hum is learnt but speech never is.
            *noise = if rms < *noise { rms } else { *noise * 0.95 + rms * 0.05 };
            self.loud_run = 0;
            if self.started {
                self.quiet_run += 1;
            }
        }
        match () {
            _ if self.started && self.quiet_run >= END_FRAMES => Verdict::Done,
            _ if self.started => Verdict::Speaking,
            _ if self.frames >= NO_SPEECH_FRAMES => Verdict::NoSpeech,
            _ => Verdict::Waiting,
        }
    }
}

/// The RMS of each whole frame in `pcm`.
pub fn frame_levels(pcm: &[u8]) -> Vec<f64> {
    pcm.chunks_exact(FRAME * 2).map(loudness).collect()
}

/// A level 0–1 for the wave on screen (a square root, so quiet speech still moves it).
pub fn display_level(rms: f64) -> f64 {
    (rms / 6000.0).sqrt().min(1.0)
}

// ---- a listening session ----

/// The pw-record listening now (0: none), so `stop` and `cancel` can end it from elsewhere.
static LISTENING: AtomicI32 = AtomicI32::new(0);
/// Set by `stop` (you're done: use what was said) or `cancel` (throw it away).
static STOP: AtomicBool = AtomicBool::new(false);
static CANCEL: AtomicBool = AtomicBool::new(false);

pub fn listening() -> bool {
    LISTENING.load(Ordering::Relaxed) != 0
}

/// What was heard, or why nothing was.
pub enum Heard {
    Text(String),
    Nothing(String),
    Cancelled,
}

/// Listen on the chosen microphone until you've finished speaking (or `stop`/`cancel`), then
/// turn it into text. `level` gets the loudness (0–1) a few times a second, `ready` once the
/// microphone gives sound. Blocking: call off the main thread.
pub fn listen(mic: &crate::sound::Node, level: &dyn Fn(f64), ready: &dyn Fn()) -> Result<Heard, String> {
    if listening() {
        return Err("I'm already listening.".into());
    }
    STOP.store(false, Ordering::Relaxed);
    CANCEL.store(false, Ordering::Relaxed);
    let mut child = Command::new("pw-record")
        .args(["--raw", "--rate", "16000", "--channels", "1", "--format", "s16", "--media-role", "Communication", "--latency", "40ms"])
        .args(["--target", &mic.name])
        .args(["-P", "{ application.name = \"Siva\" node.description = \"Siva\" }", "-"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("I can't open the microphone ({e})."))?;
    LISTENING.store(child.id() as i32, Ordering::Relaxed);
    info!("voice: listening on {} ({})", mic.label, mic.name);
    let started = Instant::now();
    let mut out = child.stdout.take().ok_or("no pw-record output")?;
    let mut pcm: Vec<u8> = Vec::with_capacity(RATE * 2 * 6);
    let mut endpoint = Endpoint::default();
    let (mut verdict, mut woke, mut buf, mut done_upto, mut grace) = (Verdict::Waking, false, [0u8; 4096], 0usize, 0usize);
    let limit = RATE * 2 * MAX_SECONDS;
    loop {
        let n = match out.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => n,
        };
        pcm.extend_from_slice(&buf[..n]);
        // Whole frames not looked at yet.
        while pcm.len() >= done_upto + FRAME * 2 {
            let rms = loudness(&pcm[done_upto..done_upto + FRAME * 2]);
            done_upto += FRAME * 2;
            if (done_upto / (FRAME * 2)) % 4 == 0 {
                level(display_level(rms));
            }
            // A Bluetooth microphone gives exact zeros until it has switched to headset mode.
            if !woke {
                if rms < 0.5 {
                    continue;
                }
                woke = true;
                info!("voice: microphone ready after {} ms", started.elapsed().as_millis());
                ready();
            }
            if grace < GRACE_FRAMES {
                grace += 1;
                continue;
            }
            verdict = endpoint.feed(rms);
        }
        if matches!(verdict, Verdict::Done | Verdict::NoSpeech) || pcm.len() >= limit || STOP.load(Ordering::Relaxed) || CANCEL.load(Ordering::Relaxed) {
            break;
        }
    }
    // SIGINT lets pw-record end cleanly.
    unsafe {
        libc::kill(child.id() as i32, libc::SIGINT);
    }
    let _ = child.wait();
    LISTENING.store(0, Ordering::Relaxed);
    level(0.0);
    let ms = millis(&pcm);
    let why = if CANCEL.load(Ordering::Relaxed) {
        "cancelled"
    } else if STOP.load(Ordering::Relaxed) {
        "stopped"
    } else {
        match verdict {
            Verdict::Done => "speech ended",
            Verdict::NoSpeech => "no speech",
            _ if pcm.len() >= limit => "at the limit",
            _ => "the microphone went away",
        }
    };
    info!(
        "voice: microphone closed after {} ms ({why}; {ms} ms of audio, noise {:.0}, loudest {:.0}, speech above {:.0})",
        started.elapsed().as_millis(),
        endpoint.noise(),
        endpoint.loudest(),
        endpoint.threshold()
    );
    if CANCEL.load(Ordering::Relaxed) {
        return Ok(Heard::Cancelled);
    }
    if !woke || peak(&pcm) < SILENT_PEAK {
        return Ok(Heard::Nothing(format!("No sound came from “{}”. Is it muted, or is another microphone the right one (Settings → Sound)?", mic.label)));
    }
    if !endpoint.started() {
        return Ok(Heard::Nothing("I didn't hear anything.".into()));
    }
    let (top, loud) = (peak(&pcm), loudness(&pcm));
    let (pcm, gain) = louder(&pcm);
    info!("voice: {ms} ms from {}, level {loud:.0}, peak {top}, made {gain:.1}x louder", mic.label);
    Ok(match transcribe(&pcm)? {
        Some(t) => Heard::Text(t),
        None => Heard::Nothing("I couldn't make out any words.".into()),
    })
}

/// You've finished: use what was said so far.
pub fn stop() {
    STOP.store(true, Ordering::Relaxed);
}

/// Throw away what's being heard.
pub fn cancel() {
    CANCEL.store(true, Ordering::Relaxed);
    let pid = LISTENING.load(Ordering::Relaxed);
    if pid != 0 {
        // Ends the read even if the microphone has stopped sending.
        unsafe {
            libc::kill(pid, libc::SIGINT);
        }
    }
}

/// 16-bit mono audio at `RATE` to text, with `whisper-cli` at low priority (blocking).
pub fn transcribe(pcm: &[u8]) -> Result<Option<String>, String> {
    let ms = millis(pcm);
    let (name, model) = model().ok_or("The speech model has gone.")?;
    let started = Instant::now();
    let mut child = Command::new("nice")
        .args(["-n", "10", "whisper-cli", "-m"])
        .arg(&model)
        .args(["-t", THREADS, "-l", "en", "-np", "-nt", "-otxt", "-ac", &audio_ctx(ms).to_string(), "--prompt", PROMPT, "-f", "-"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("I can't start whisper ({e})."))?;
    let mut input = child.stdin.take().ok_or("no whisper input")?;
    let audio = wav(pcm);
    let writer = std::thread::spawn(move || {
        use std::io::Write;
        let _ = input.write_all(&audio);
    });
    let out = child.wait_with_output().map_err(|e| format!("whisper: {e}"))?;
    let _ = writer.join();
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        let last = err.lines().filter(|l| !l.trim().is_empty()).last().unwrap_or("").to_string();
        warn!("voice: whisper-cli failed ({}): {last}", out.status);
        return Err("Sorry, my ears failed: whisper didn't work.".into());
    }
    let text = tidy(&String::from_utf8_lossy(&out.stdout));
    info!("voice: {ms} ms of speech, heard in {} ms with {name}: {:?}", started.elapsed().as_millis(), text.as_deref().unwrap_or(""));
    Ok(text)
}

#[tauri::command]
pub fn voice_state() -> VoiceState {
    state()
}

/// The HUD's Done (or the menu-bar item clicked while listening).
#[tauri::command]
pub fn voice_stop() {
    stop();
}

#[tauri::command]
pub fn voice_cancel() {
    cancel();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(ms: usize, amplitude: i16) -> Vec<u8> {
        (0..RATE * ms / 1000).flat_map(|i| (if i % 2 == 0 { amplitude } else { -amplitude }).to_le_bytes()).collect()
    }

    #[test]
    fn wav_header() {
        let pcm = tone(500, 1000);
        let w = wav(&pcm);
        assert_eq!(&w[0..4], b"RIFF");
        assert_eq!(&w[8..16], b"WAVEfmt ");
        assert_eq!(u32::from_le_bytes(w[4..8].try_into().unwrap()) as usize, w.len() - 8);
        assert_eq!(u32::from_le_bytes(w[24..28].try_into().unwrap()), 16_000);
        assert_eq!(u32::from_le_bytes(w[40..44].try_into().unwrap()) as usize, pcm.len());
        assert_eq!(w.len(), 44 + pcm.len());
    }

    #[test]
    fn length_and_loudness() {
        assert_eq!(millis(&tone(1500, 1)), 1500);
        assert_eq!(loudness(&[]), 0.0);
        assert!((loudness(&tone(100, 3000)) - 3000.0).abs() < 0.01);
        assert_eq!(peak(&tone(100, -3000)), 3000);
        assert_eq!(peak(&[]), 0);
    }

    #[test]
    fn quiet_speech_made_louder() {
        let (out, gain) = louder(&tone(100, 1200));
        assert_eq!(gain, 20.0, "at most MAX_GAIN");
        assert_eq!(peak(&out), 24_000);
        let (out, gain) = louder(&tone(100, 6000));
        assert_eq!((gain, peak(&out)), (4.0, 24_000));
        let (out, gain) = louder(&tone(100, 30_000));
        assert_eq!((gain, peak(&out)), (1.0, 30_000), "never quieter");
        assert_eq!(louder(&tone(10, i16::MIN + 1)).0.len(), tone(10, 1).len());
    }

    #[test]
    fn audio_context() {
        assert_eq!(audio_ctx(2000), 512, "never below 512");
        assert_eq!(audio_ctx(12_000), 700);
        assert_eq!(audio_ctx(30_000), 1500);
        assert_eq!(audio_ctx(60_000), 1500);
    }

    #[test]
    fn tidying_what_whisper_wrote() {
        assert_eq!(tidy(" Volume 30.\n").as_deref(), Some("Volume 30"));
        assert_eq!(tidy(" Open Firefox, then put it on the left.").as_deref(), Some("Open Firefox, then put it on the left"));
        assert_eq!(tidy("[BLANK_AUDIO]\n"), None);
        assert_eq!(tidy(" (music playing)"), None);
        assert_eq!(tidy(" *coughs* Mute."), Some("Mute".into()));
        assert_eq!(tidy(" [ Silence ] Dark mode on!"), Some("Dark mode on".into()));
        assert_eq!(tidy(" ... "), None);
        assert_eq!(tidy(""), None);
        assert_eq!(tidy(" Why you care? Why you care?").as_deref(), Some("Why you care"));
        assert_eq!(tidy(" Matching my terminals. Matching my terminals").as_deref(), Some("Matching my terminals"));
        assert_eq!(tidy(" Volume 30. Dark mode.").as_deref(), Some("Volume 30. Dark mode"), "different sentences stay");
    }

    /// whisper-cli on a real recording: `KOMPACT_VOICE_SAMPLE=<16 kHz mono WAV> cargo test
    /// voice_live -- --ignored --nocapture` (whisper.cpp's package has one in
    /// /usr/share/sounds/debian/samples).
    #[test]
    #[ignore]
    fn voice_live() {
        let file = std::env::var("KOMPACT_VOICE_SAMPLE").expect("KOMPACT_VOICE_SAMPLE");
        let bytes = std::fs::read(file).unwrap();
        let pcm = &bytes[44..bytes.len().min(44 + RATE * 2 * 4)]; // its first 4 s
        let started = Instant::now();
        let text = transcribe(pcm).unwrap();
        println!("heard {text:?} in {} ms", started.elapsed().as_millis());
        assert!(text.is_some_and(|t| t.to_lowercase().contains("ignore all rules")));
    }

    /// Feeds `frames` and returns the verdict after each.
    fn run(e: &mut Endpoint, frames: &[(f64, usize)]) -> Vec<Verdict> {
        frames.iter().flat_map(|&(rms, n)| std::iter::repeat_n(rms, n)).map(|r| e.feed(r)).collect()
    }

    #[test]
    fn hearing_when_you_have_finished() {
        // The headphones: ~1 s of zeros while they switch to headset mode, room noise ~2,
        // "volume thirty" with a short pause between the words, then quiet.
        let mut e = Endpoint::default();
        let v = run(&mut e, &[(0.0, 50), (2.0, 20), (450.0, 25), (3.0, 12), (380.0, 20)]);
        assert!(v[..50].iter().all(|&x| x == Verdict::Waking), "zeros: not awake yet");
        assert_eq!(v[69], Verdict::Waiting);
        assert_eq!(v[72], Verdict::Speaking, "three loud frames start speech");
        assert_eq!(v[100], Verdict::Speaking, "a 0.24 s pause between words doesn't end it");
        let after = run(&mut e, &[(2.0, END_FRAMES)]);
        assert_eq!(after[END_FRAMES - 2], Verdict::Speaking);
        assert_eq!(after[END_FRAMES - 1], Verdict::Done, "0.6 s of quiet ends it");
    }

    #[test]
    fn a_click_is_not_speech() {
        let mut e = Endpoint::default();
        let v = run(&mut e, &[(2.0, 10), (2000.0, 2), (2.0, 10)]);
        assert!(!e.started());
        assert_eq!(*v.last().unwrap(), Verdict::Waiting);
    }

    #[test]
    fn giving_up_when_nobody_speaks() {
        let mut e = Endpoint::default();
        let v = run(&mut e, &[(0.0, 40), (3.0, NO_SPEECH_FRAMES)]);
        assert_eq!(v[40 + NO_SPEECH_FRAMES - 2], Verdict::Waiting);
        assert_eq!(*v.last().unwrap(), Verdict::NoSpeech, "6 s after the microphone woke");
    }

    #[test]
    fn a_hum_is_learnt_speech_is_not() {
        let mut e = Endpoint::default();
        run(&mut e, &[(30.0, 50)]);
        assert!((e.threshold() - 120.0).abs() < 1.0, "a steady hum of 30 needs speech above 120");
        run(&mut e, &[(30.0, 2), (900.0, 200)]);
        assert!(e.started());
        assert!(e.threshold() < 130.0, "two seconds of speech don't raise the noise");
        // The level shown on screen.
        assert_eq!(display_level(0.0), 0.0);
        assert!(display_level(150.0) > 0.15, "quiet speech still moves the wave");
        assert_eq!(display_level(60_000.0), 1.0);
        assert_eq!(frame_levels(&tone(100, 500)).len(), 5);
    }

    #[test]
    fn best_model_first() {
        assert_eq!(MODELS[0], "small.en");
        assert!(MODELS.contains(&"base.en"));
    }
}
