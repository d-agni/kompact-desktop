//! 19f: a small local language model for what the rules don't understand ("I can't hear
//! anything", "get the browser out of the way"). Qwen2.5-1.5B-Instruct (Q4_K_M, 1.1 GB, fetched
//! once by `shell/tools/get-assistant-model.sh` into `~/.local/share/kompact/llm/`) runs on the
//! CPU in Ubuntu's `llama-completion` (`sudo apt install llama.cpp-tools`), at low priority.
//!
//! It can only *propose*. It answers with calls like `place_window(window="files", where="left")`
//! (Python-like calls, which small models write far more reliably than JSON: measured 2026-10-24),
//! held by a grammar to the names in `tools::TOOLS` and at most `MAX_CALLS`; each call is read by
//! `read_answer`, checked by `tools::validate`, and goes through `policy` like a typed one (Tier
//! 2 still asks). It is given only your words and the tool list, never window titles,
//! notifications or anything else someone else could have written. The instructions are worked
//! through once and kept in a prompt cache (`~/.cache/kompact/llm/`), so a request costs only
//! its own words.
//!
//! The prompt, the grammar and reading the answer are pure and tested; the rest runs the program.

use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::Instant;

use serde_json::{json, Map, Value};
use tracing::{info, warn};

use super::rules::Step;
use super::tools::{self, Arg, TOOLS};

pub const MODEL: &str = "qwen2.5-1.5b-instruct-q4_k_m.gguf";
pub const MODEL_NAME: &str = "Qwen2.5 1.5B";
/// Calls one answer may hold (the model guessing at misheard words made five).
pub const MAX_CALLS: usize = 4;
/// Tokens it may write.
const PREDICT: &str = "80";
/// 4 of the Ryzen's 8 threads: 8 was slower (measured 2026-10-24: 15 tokens/s with 4).
const THREADS: &str = "4";
const CONTEXT: &str = "2048";

// ---- what it's told (pure) ----

fn quoted(words: &[&str]) -> String {
    words.iter().map(|w| format!("\"{w}\"")).collect::<Vec<_>>().join(", ")
}

/// One line per tool: `place_window(window, where) - Put a window …; where: "left", "right", …`.
fn tool_lines() -> String {
    TOOLS
        .iter()
        .map(|t| {
            let names: Vec<String> = t.params.iter().map(|p| if p.required { p.name.to_string() } else { format!("{}=…", p.name) }).collect();
            let mut line = format!("{}({}) - {}", t.name, names.join(", "), t.about.trim_end_matches('.'));
            for p in t.params {
                let detail = match p.arg {
                    Arg::Int { min, max } => format!("{} to {}", min, max),
                    Arg::Bool => "True or False".into(),
                    Arg::OneOf(words) => quoted(words),
                    Arg::Text { .. } => "a name".into(),
                };
                line.push_str(&format!("; {}: {detail}", p.name));
                if !p.about.is_empty() {
                    line.push_str(&format!(" ({})", p.about));
                }
            }
            line
        })
        .collect::<Vec<_>>()
        .join("\n")
}

const EXAMPLES: &str = r#"Examples:
make it louder -> change_volume(by=10)
it's way too loud -> change_volume(by=-20)
i can't hear anything -> mute(on=False); set_volume(percent=60)
get the browser out of the way -> minimize_window(window="firefox")
move files to the left -> place_window(window="files", where="left")
what song is this -> now_playing()
skip this one -> media(action="next")
it's too bright -> appearance(mode="dark")
order me a pizza -> none
the cat sat on all the -> none"#;

/// The instructions, the same every time (so the prompt cache can keep them), in Qwen's chat
/// format.
pub fn prefix() -> String {
    format!(
        "<|im_start|>system\nYou turn a person's request into calls to these desktop tools. Answer with the calls only, \
separated by \"; \", or \"none\". The request was often spoken, so words may be misheard: if it isn't clearly something \
these tools do, answer none. Window and app names are as the person said them (firefox, files, terminal, settings). \
Use as few calls as needed.\n\nTools:\n{}\n\n{EXAMPLES}<|im_end|>\n<|im_start|>user\n",
        tool_lines()
    )
}

/// The whole prompt for `text` (your words, tidied to one line).
pub fn prompt(text: &str) -> String {
    let words: String = text.chars().map(|c| if c.is_control() { ' ' } else { c }).take(300).collect();
    format!("{}{}<|im_end|>\n<|im_start|>assistant\n", prefix(), words.trim())
}

/// The grammar its answer must follow: `none`, or up to `MAX_CALLS` calls to tools that exist.
pub fn grammar() -> String {
    let names: Vec<String> = TOOLS.iter().map(|t| format!("\"{}\"", t.name)).collect();
    let more = "(\"; \" call)?".repeat(MAX_CALLS - 1);
    format!("root ::= \"none\" | call {more}\ncall ::= ({}) \"(\" args \")\"\nargs ::= [^()\\n;]*\n", names.join(" | "))
}

// ---- reading its answer (pure) ----

/// Splits at `sep` outside double quotes.
fn split_outside_quotes(s: &str, sep: char) -> Vec<String> {
    let (mut out, mut cur, mut quoted) = (Vec::new(), String::new(), false);
    for c in s.chars() {
        match c {
            '"' => {
                quoted = !quoted;
                cur.push(c);
            }
            c if c == sep && !quoted => out.push(std::mem::take(&mut cur)),
            c => cur.push(c),
        }
    }
    out.push(cur);
    out.into_iter().map(|p| p.trim().to_string()).filter(|p| !p.is_empty()).collect()
}

/// One value, as the parameter wants it.
fn value(arg: Arg, raw: &str) -> Option<Value> {
    let text = raw.trim().trim_matches(|c| c == '"' || c == '\'').trim();
    match arg {
        Arg::Int { .. } => text.parse::<i64>().ok().map(Value::from),
        Arg::Bool => match text.to_lowercase().as_str() {
            "true" | "on" | "yes" | "1" => Some(json!(true)),
            "false" | "off" | "no" | "0" => Some(json!(false)),
            _ => None,
        },
        Arg::OneOf(words) => {
            let t = text.to_lowercase().replace([' ', '_'], "-");
            words.iter().find(|w| **w == t).map(|w| json!(w))
        }
        // `all_files` → "all files".
        Arg::Text { .. } => (!text.is_empty()).then(|| json!(text.replace('_', " "))),
    }
}

/// `place_window(window="files", where=left)` → a step, checked.
fn call(c: &str) -> Result<Step, String> {
    let open = c.find('(').ok_or_else(|| format!("not a call: {c}"))?;
    let name = c[..open].trim();
    let tool = tools::find(name).ok_or_else(|| format!("no tool {name:?}"))?;
    let inner = c[open + 1..].trim_end().strip_suffix(')').ok_or_else(|| format!("not a call: {c}"))?;
    let mut args = Map::new();
    for (i, part) in split_outside_quotes(inner, ',').iter().enumerate() {
        let (param, raw) = match part.split_once('=') {
            Some((k, v)) if !k.contains('"') => (tool.params.iter().find(|p| p.name == k.trim()).ok_or_else(|| format!("{name}: no {k:?}"))?, v),
            _ => (tool.params.get(i).ok_or_else(|| format!("{name}: too many details"))?, part.as_str()),
        };
        let v = value(param.arg, raw).ok_or_else(|| format!("{name}: {} can't be {raw:?}", param.name))?;
        args.insert(param.name.to_string(), v);
    }
    let args = Value::Object(args);
    tool.validate(&args).map_err(|e| format!("{name}: {e}"))?;
    Ok(Step { tool: tool.name, args })
}

/// Its answer as steps (none: it can't). Anything off (an unknown tool, bad details, too many
/// calls) is refused here, before `policy` would.
pub fn read_answer(out: &str) -> Result<Vec<Step>, String> {
    let text = out.replace("[end of text]", "");
    let text = text.lines().next().unwrap_or("").trim().trim_end_matches(';').trim();
    if text.is_empty() || text.eq_ignore_ascii_case("none") {
        return Ok(Vec::new());
    }
    let calls = split_outside_quotes(text, ';');
    if calls.len() > MAX_CALLS {
        return Err(format!("{} calls", calls.len()));
    }
    calls.iter().map(|c| call(c)).collect()
}

/// Words that show a request is about a tool (any word of the request starting with one counts:
/// "loud" for "louder").
fn tool_words(tool: &str) -> &'static [&'static str] {
    match tool {
        "set_volume" | "change_volume" => &["volume", "loud", "quiet", "hear", "sound", "silent", "softer", "noise"],
        "mute" | "sound_status" => &["mute", "unmute", "silent", "quiet", "sound", "hear", "mic", "microphone", "audio", "volume"],
        "sound_output" => &["speaker", "headphone", "headset", "output", "sound", "audio", "earphone"],
        "media" | "now_playing" => &["play", "pause", "song", "music", "track", "skip", "next", "previous", "resume", "listen"],
        "open_app" => &["open", "start", "launch", "run"],
        "focus_window" => &["show", "bring", "switch", "focus", "back"],
        "minimize_window" => &["minimi", "hide", "away", "way"],
        "place_window" => &["left", "right", "maximi", "full", "big", "restore", "side", "half", "move", "put"],
        "close_window" => &["close", "quit", "exit", "kill"],
        "switch_desktop" => &["desktop", "workspace", "space"],
        "do_not_disturb" => &["disturb", "notification", "focus", "silence", "quiet"],
        "appearance" => &["dark", "light", "bright", "theme", "appearance", "mode", "night"],
        "keep_awake" => &["awake", "sleep", "lock", "caffein"],
        "screenshot" => &["screenshot", "capture", "screen", "shot", "picture"],
        "lock_screen" => &["lock"],
        "wifi" | "network_status" => &["wifi", "wi", "wireless", "internet", "network", "online", "connect"],
        "bluetooth" | "bluetooth_status" => &["bluetooth", "pair", "connect", "device"],
        "log_out" => &["log", "sign"],
        "power" => &["shut", "restart", "reboot", "sleep", "power", "suspend", "computer"],
        "list_windows" => &["window", "open", "running"],
        "system_status" => &["slow", "lag", "cpu", "memory", "ram", "busy", "computer", "fan", "hot", "freez", "stuck"],
        "disk_space" => &["disk", "space", "storage", "full", "drive"],
        "sound_check" => &["sound", "audio", "cut", "choppy", "crackl", "quality", "bad", "stutter", "phone"],
        _ => &[],
    }
}

/// 19f-2: whether a step the model proposed is about what was said: the request has one of its
/// tool's words, or the app / window it names. ("Maximate Firefox", misheard, came back as
/// "volume up 100": not about it.)
pub fn grounded(step: &Step, text: &str) -> bool {
    let said = super::rules::normalize(text);
    let words: Vec<&str> = said.split_whitespace().collect();
    let named = ["app", "window", "device"].iter().filter_map(|k| step.args.get(*k).and_then(Value::as_str)).any(|name| {
        let name = super::rules::normalize(name);
        !name.is_empty() && name.split_whitespace().all(|n| words.contains(&n))
    });
    named || tool_words(step.tool).iter().any(|k| words.iter().any(|w| w.starts_with(k)))
}

/// A short name for the prompt cache, changing whenever the instructions or the model do.
pub fn cache_key(prefix: &str, model_size: u64) -> String {
    // FNV-1a: no crate for a file name.
    let mut h: u64 = 0xcbf29ce484222325;
    for b in prefix.bytes().chain(model_size.to_le_bytes()) {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x100000001b3);
    }
    format!("prompt-{h:016x}.cache")
}

// ---- llama-completion ----

fn home_dir(var: &str, fallback: &str) -> Option<PathBuf> {
    std::env::var_os(var).map(PathBuf::from).or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(fallback)))
}

fn on_path(program: &str) -> bool {
    std::env::var_os("PATH").is_some_and(|paths| std::env::split_paths(&paths).any(|d| d.join(program).is_file()))
}

pub fn model_file() -> Option<PathBuf> {
    let f = home_dir("XDG_DATA_HOME", ".local/share")?.join("kompact").join("llm").join(MODEL);
    f.is_file().then_some(f)
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct ModelState {
    pub ready: bool,
    pub model: &'static str,
    pub missing: Option<String>,
}

pub fn state() -> ModelState {
    let missing = match (on_path("llama-completion"), model_file().is_some()) {
        (true, true) => None,
        // 19i: one command sets it up (the package and the model).
        _ => Some("Run “kompact-assistant-setup understanding” in a terminal (1.1 GB, once).".into()),
    };
    ModelState { ready: missing.is_none(), model: MODEL_NAME, missing }
}

/// Ask the model what `text` means as steps (blocking: off the main thread; ~1.5–2.5 s here).
pub fn ask(text: &str) -> Result<Vec<Step>, String> {
    let model = model_file().ok_or("the model isn't there")?;
    let size = std::fs::metadata(&model).map(|m| m.len()).unwrap_or(0);
    let cache_dir = home_dir("XDG_CACHE_HOME", ".cache").ok_or("no home folder")?.join("kompact").join("llm");
    std::fs::create_dir_all(&cache_dir).map_err(|e| e.to_string())?;
    let cache = cache_dir.join(cache_key(&prefix(), size));
    let fresh = !cache.is_file();
    if fresh {
        // Caches of older instructions or models (20 MB each).
        for old in std::fs::read_dir(&cache_dir).into_iter().flatten().flatten() {
            if old.file_name().to_string_lossy().starts_with("prompt-") {
                let _ = std::fs::remove_file(old.path());
            }
        }
    }
    let started = Instant::now();
    let mut cmd = Command::new("nice");
    cmd.args(["-n", "10", "llama-completion", "-m"])
        .arg(&model)
        // --no-repack: repacking the weights for AVX2 cost 0.8 s a run, for no faster answers.
        .args(["-no-cnv", "--no-display-prompt", "--simple-io", "--no-warmup", "--no-repack", "--temp", "0", "-t", THREADS, "-c", CONTEXT, "-n", PREDICT])
        .args(["--grammar", &grammar(), "-p", &prompt(text), "--prompt-cache"])
        .arg(&cache);
    // The first run writes the cache (the instructions and these words); later ones only read
    // it, so it keeps just what every request shares.
    if !fresh {
        cmd.arg("--prompt-cache-ro");
    }
    let out = cmd.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).output().map_err(|e| format!("can't start llama-completion: {e}"))?;
    let ms = started.elapsed().as_millis();
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        warn!("model: llama-completion failed ({}): {}", out.status, err.lines().filter(|l| !l.trim().is_empty()).last().unwrap_or(""));
        return Err("the model didn't run".into());
    }
    let answer = String::from_utf8_lossy(&out.stdout).replace("[end of text]", "");
    info!("model: {text:?} → {} in {ms} ms{}", answer.trim(), if fresh { " (prompt cache made)" } else { "" });
    read_answer(&answer)
}

/// Make the prompt cache in the background (the first run takes ~12 s here), half a minute
/// after the shell starts, so the first real question is answered in a second or two.
pub fn warm_up() {
    std::thread::spawn(|| {
        std::thread::sleep(std::time::Duration::from_secs(30));
        let Some(model) = model_file() else { return };
        if !state().ready {
            return;
        }
        let size = std::fs::metadata(&model).map(|m| m.len()).unwrap_or(0);
        let cached = home_dir("XDG_CACHE_HOME", ".cache").map(|d| d.join("kompact").join("llm").join(cache_key(&prefix(), size)).is_file());
        if cached == Some(false) {
            info!("model: making the prompt cache");
            let _ = ask("hello");
        }
    });
}

#[tauri::command]
pub fn model_state() -> ModelState {
    state()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn what_it_is_told() {
        let p = prefix();
        for t in TOOLS {
            assert!(p.contains(&format!("\n{}(", t.name)), "{} is listed", t.name);
        }
        assert!(p.contains("place_window(window, where) - Put a window on the left or right half, fill the screen, or restore its size; window: a name (its title or app); where: \"left\", \"right\", \"fill\", \"restore\""));
        assert!(p.contains("mute(on, device=…)"));
        // Your words come after everything shared, so the cache covers all of that.
        let q = prompt("make it louder\n");
        assert!(q.starts_with(&p));
        assert!(q.ends_with("make it louder<|im_end|>\n<|im_start|>assistant\n"));
        assert!(!prompt(&"x".repeat(1000)).contains(&"x".repeat(301)), "long requests are cut");
    }

    #[test]
    fn the_grammar() {
        let g = grammar();
        assert!(g.starts_with("root ::= \"none\" | call (\"; \" call)?(\"; \" call)?(\"; \" call)?\n"), "{g}");
        for t in TOOLS {
            assert!(g.contains(&format!("\"{}\"", t.name)));
        }
    }

    #[test]
    fn its_examples_are_answers_it_may_give() {
        for line in EXAMPLES.lines().skip(1) {
            let answer = line.split(" -> ").nth(1).unwrap();
            assert!(read_answer(answer).is_ok(), "{line}");
        }
    }

    #[test]
    fn reading_its_answer() {
        let tools = |a: &str| read_answer(a).unwrap().iter().map(|s| (s.tool, s.args.clone())).collect::<Vec<_>>();
        // As measured on the real model.
        assert_eq!(tools(r#"place_window(window="all_files", where="left") [end of text]"#), [("place_window", json!({"window": "all files", "where": "left"}))]);
        assert_eq!(tools("wifi(off)"), [("wifi", json!({"on": false}))]);
        assert_eq!(tools("switch_desktop(2)"), [("switch_desktop", json!({"number": 2}))]);
        assert_eq!(tools("mute(on=False); set_volume(percent=60)"), [("mute", json!({"on": false})), ("set_volume", json!({"percent": 60}))]);
        assert_eq!(tools("media(action=\"Play Pause\")"), [("media", json!({"action": "play-pause"}))]);
        assert_eq!(tools("now_playing()"), [("now_playing", json!({}))]);
        assert_eq!(tools(r#"focus_window(window="Notes, draft")"#), [("focus_window", json!({"window": "Notes, draft"}))], "a comma in quotes");
        assert_eq!(read_answer("none").unwrap(), vec![]);
        assert_eq!(read_answer(" None \n").unwrap(), vec![]);
        // Refused.
        assert!(read_answer("run_shell(cmd=\"rm -rf /\")").unwrap_err().contains("no tool"));
        assert!(read_answer("set_volume(percent=900)").is_err(), "out of range");
        assert!(read_answer("power(action=\"format-disk\")").is_err());
        assert!(read_answer("place_window(window=files, where=left, colour=red)").is_err());
        assert!(read_answer("Sure! I'll turn it up.").is_err());
        assert!(read_answer("sound_status(); now_playing(); list_windows(); log_out(); close_window(window=\"all\")").unwrap_err().contains("5 calls"));
    }

    #[test]
    fn about_what_was_said() {
        let st = |t: &'static str, a: Value| Step { tool: t, args: a };
        // From the 2026-10-24 log.
        assert!(!grounded(&st("change_volume", json!({"by": 100})), "Maximate Firefox"), "misheard: asks first");
        assert!(grounded(&st("open_app", json!({"app": "Firefox"})), "Mac to my Firefox"), "it names the app");
        assert!(grounded(&st("minimize_window", json!({"window": "firefox"})), "Get browser out of the way"));
        assert!(grounded(&st("close_window", json!({"window": "Firefox"})), "Close"));
        assert!(grounded(&st("change_volume", json!({"by": -20})), "it's way too loud"));
        assert!(grounded(&st("now_playing", json!({})), "what song is this"));
        assert!(!grounded(&st("power", json!({"action": "restart"})), "the cat sat on all"));
        assert!(!grounded(&st("place_window", json!({"window": "all files", "where": "left"})), "Me and all the devices"));
    }

    #[test]
    fn the_cache_follows_the_instructions() {
        assert_eq!(cache_key("a", 1), cache_key("a", 1));
        assert_ne!(cache_key("a", 1), cache_key("b", 1));
        assert_ne!(cache_key("a", 1), cache_key("a", 2), "another model");
    }

    /// The real model: `cargo test model_live -- --ignored --nocapture` (needs llama-completion
    /// and the model).
    #[test]
    #[ignore]
    fn model_live() {
        let asks = [
            "make it louder",
            "i can't hear anything",
            "move all files to left",
            "get the browser out of the way",
            "it's way too loud",
            "put the terminal on the right and firefox on the left",
            "shut down the computer",
            "what's playing right now",
            "turn off the wifi",
            "Me and all the devices",
            "Two points",
            "write me a poem",
        ];
        for a in asks {
            let t = std::time::Instant::now();
            let r = ask(a);
            println!("{a:?} ({} ms): {:?}", t.elapsed().as_millis(), r.map(|s| s.iter().map(super::super::rules::describe).collect::<Vec<_>>()));
        }
    }
}
