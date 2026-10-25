//! 20d-2: an optional cloud AI, off until you give it a key (Settings → Assistant → Cloud AI).
//! It takes the local model's place for what the rules don't know: understanding a request as
//! tool calls, and talking. The rules still do everything they know, on this PC, at once.
//!
//! It is held to exactly what the local model is: it is given only your words (and, for
//! talking, the last few things you and Siva said to each other), never window titles, files or
//! the screen; its calls are read by `model::read_answer`, checked by `tools::validate`, kept
//! only when about your words (`model::keep_grounded`) and go through `policy` (drastic ones
//! still need your click). Its talk is checked by `model::checked_talk`. When it fails (no
//! internet, a wrong key, a used-up quota) the local model answers instead.
//!
//! It is used only while Settings → Assistant → Internet is Always. The key lives in
//! `~/.config/kompact/assistant-cloud` (owner-only), goes to curl on its stdin (`web::post`)
//! and is never logged or shown again. Building the requests and reading the answers is pure
//! and tested.

use std::path::PathBuf;
use std::time::Instant;

use serde::Serialize;
use serde_json::{json, Value};
use tracing::info;

use super::rules::Step;
use super::web::{self, Provider, PROVIDERS};

/// The Messages API version sent to Anthropic.
const ANTHROPIC_VERSION: &str = "2026-10-21";
/// How long an answer may take.
const SECONDS: u32 = 20;

// ---- settings ----

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Prefs {
    pub on: bool,
    pub provider: String,
    /// Empty: the provider's own model.
    pub model: String,
    pub key: String,
}

impl Default for Prefs {
    fn default() -> Prefs {
        Prefs { on: false, provider: PROVIDERS[0].id.into(), model: String::new(), key: String::new() }
    }
}

impl Prefs {
    pub fn provider(&self) -> &'static Provider {
        web::provider(&self.provider).unwrap_or(&PROVIDERS[0])
    }
    pub fn model(&self) -> &str {
        if self.model.is_empty() {
            self.provider().model
        } else {
            &self.model
        }
    }
}

/// A key or a model name as typed: printable, no spaces, at most 300 characters.
pub fn clean_word(s: &str) -> String {
    s.trim().chars().filter(|c| !c.is_control() && !c.is_whitespace()).take(300).collect()
}

pub fn parse_prefs(text: &str) -> Prefs {
    let mut p = Prefs::default();
    for line in text.lines().map(str::trim) {
        if let Some(v) = line.strip_prefix("on=") {
            p.on = v.trim() == "true";
        } else if let Some(v) = line.strip_prefix("provider=") {
            if web::provider(v.trim()).is_some() {
                p.provider = v.trim().into();
            }
        } else if let Some(v) = line.strip_prefix("model=") {
            p.model = clean_word(v);
        } else if let Some(v) = line.strip_prefix("key=") {
            p.key = clean_word(v);
        }
    }
    p
}

pub fn prefs_text(p: &Prefs) -> String {
    format!("# Siva's cloud AI (written by System Settings → Assistant; keep it private)\non={}\nprovider={}\nmodel={}\nkey={}\n", p.on, p.provider, p.model, p.key)
}

fn prefs_file() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from).or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
    Some(base.join("kompact").join("assistant-cloud"))
}

pub fn load_prefs() -> Prefs {
    parse_prefs(&prefs_file().and_then(|f| std::fs::read_to_string(f).ok()).unwrap_or_default())
}

/// Saved readable by you alone (0600), as it holds the key.
pub fn save_prefs(p: &Prefs) -> Result<(), String> {
    use std::io::Write;
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
    let file = prefs_file().ok_or("no home folder")?;
    if let Some(dir) = file.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let mut f = std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(&file).map_err(|e| format!("can't save {}: {e}", file.display()))?;
    // An older file keeps its old mode on open.
    let _ = std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o600));
    f.write_all(prefs_text(p).as_bytes()).map_err(|e| format!("can't save {}: {e}", file.display()))
}

/// Why the cloud AI isn't used now, or `None` when it is.
pub fn not_used(p: &Prefs, internet: web::Mode) -> Option<&'static str> {
    if !p.on {
        Some("it's switched off")
    } else if p.key.is_empty() {
        Some("there's no key")
    } else if internet != web::Mode::Always {
        Some("Internet isn't set to Always")
    } else {
        None
    }
}

/// The cloud AI's settings, when it's to be used now.
pub fn active() -> Option<Prefs> {
    let p = load_prefs();
    not_used(&p, web::load_prefs().mode).is_none().then_some(p)
}

// ---- the requests (pure) ----

/// Anthropic's models that take `output_config.effort` (and its server-side fallbacks, for
/// the ones that have them).
fn takes_effort(model: &str) -> bool {
    ["claude-fable-5", "claude-mythos-5", "claude-opus-5", "claude-sonnet-5", "claude-haiku-5", "claude-opus-4-6", "claude-opus-4-7", "claude-opus-4-8", "claude-sonnet-4-6"].iter().any(|m| model.starts_with(m))
}

fn has_fallbacks(model: &str) -> bool {
    ["claude-fable-5-1", "claude-opus-5-5", "claude-opus-5", "claude-sonnet-5-5"].contains(&model)
}

/// The headers and the body asking `p`'s model, as `system`, about `turns` (who, what: "user"
/// and "assistant", the last one yours).
pub fn request(p: &Prefs, system: &str, turns: &[(&str, String)]) -> (Vec<String>, String) {
    let provider = p.provider();
    let model = p.model();
    let messages: Vec<Value> = turns.iter().map(|(role, text)| json!({"role": role, "content": text})).collect();
    if provider.anthropic {
        let mut headers = vec![format!("x-api-key: {}", p.key), format!("anthropic-version: {ANTHROPIC_VERSION}"), "content-type: application/json".to_string()];
        // Short answers; its thinking counts against max_tokens too.
        let mut body = json!({"model": model, "max_tokens": 4000, "system": system, "messages": messages});
        if takes_effort(model) {
            body["output_config"] = json!({"effort": "low"});
        }
        // A refusal on the model you chose is answered by another Claude model.
        if has_fallbacks(model) {
            headers.push("anthropic-beta: server-side-fallback-2026-07-01".into());
            body["fallbacks"] = json!("default");
        }
        (headers, body.to_string())
    } else {
        let headers = vec![format!("Authorization: Bearer {}", p.key), "Content-Type: application/json".to_string()];
        let mut all = vec![json!({"role": "system", "content": system})];
        all.extend(messages);
        (headers, json!({"model": model, "messages": all}).to_string())
    }
}

/// What a provider's error was, said for a person.
pub fn trouble(status: u16, body: &str) -> String {
    let detail = serde_json::from_str::<Value>(body).ok().and_then(|v| v.pointer("/error/message").and_then(Value::as_str).map(str::to_string)).unwrap_or_default();
    let said = match status {
        401 | 403 => "the cloud AI didn't accept the key",
        404 => "the cloud AI doesn't know that model",
        429 => "the cloud AI is busy, or its quota is used up",
        400 => "the cloud AI didn't accept the request",
        500..=599 => "the cloud AI is having trouble",
        _ => "the cloud AI didn't answer properly",
    };
    if detail.is_empty() {
        format!("{said} ({status})")
    } else {
        format!("{said} ({status}: {})", detail.chars().take(160).collect::<String>())
    }
}

/// The text of an answer.
pub fn answer_text(p: &Prefs, body: &str) -> Result<String, String> {
    let v: Value = serde_json::from_str(body).map_err(|_| "the cloud AI sent something I can't read".to_string())?;
    if p.provider().anthropic {
        if v["stop_reason"] == "refusal" {
            return Err("the cloud AI declined".into());
        }
        let text: Vec<&str> = v["content"].as_array().into_iter().flatten().filter(|b| b["type"] == "text").filter_map(|b| b["text"].as_str()).collect();
        let text = text.join("");
        (!text.trim().is_empty()).then_some(text).ok_or_else(|| "the cloud AI had nothing to say".into())
    } else {
        v.pointer("/choices/0/message/content").and_then(Value::as_str).filter(|t| !t.trim().is_empty()).map(str::to_string).ok_or_else(|| "the cloud AI had nothing to say".into())
    }
}

/// The calls in an answer: the first line that isn't empty, without code fences or quotes
/// around it (a big model sometimes adds them).
pub fn calls_line(answer: &str) -> String {
    let line = answer.lines().map(str::trim).find(|l| !l.is_empty() && !l.starts_with("```")).unwrap_or("");
    line.trim_matches('`').trim().to_string()
}

// ---- asking ----

fn send(p: &Prefs, system: &str, turns: &[(&str, String)]) -> Result<String, String> {
    let (headers, body) = request(p, system, turns);
    let started = Instant::now();
    let (status, text) = web::post(p.provider().url, &headers, &body, SECONDS)?;
    if status != 200 {
        let why = trouble(status, &text);
        info!("cloud: {} {}: {why}", p.provider().id, p.model());
        return Err(why);
    }
    let said = answer_text(p, &text)?;
    info!("cloud: {} {} answered in {} ms", p.provider().id, p.model(), started.elapsed().as_millis());
    Ok(said)
}

/// What `text` means as steps (blocking).
pub fn ask(p: &Prefs, text: &str) -> Result<Vec<Step>, String> {
    let words: String = text.chars().map(|c| if c.is_control() { ' ' } else { c }).take(300).collect();
    let system = format!("{}\n\nAnswer on one line, with nothing else.", super::model::instructions());
    let answer = send(p, &system, &[("user", words.trim().to_string())])?;
    info!("cloud: {text:?} → {}", calls_line(&answer));
    super::model::read_answer(&calls_line(&answer))
}

/// Siva's answer to `text`, given the last exchanges of talk (blocking).
pub fn talk(p: &Prefs, text: &str, history: &[(String, String)], now: &str) -> Result<String, String> {
    let clean = |t: &str| t.chars().map(|c| if c.is_control() { ' ' } else { c }).take(300).collect::<String>();
    let system = format!("{}\n\n{now}", super::model::PERSONA);
    let mut turns: Vec<(&str, String)> = Vec::new();
    for (you, siva) in history {
        turns.push(("user", clean(you)));
        turns.push(("assistant", clean(siva)));
    }
    turns.push(("user", clean(text).trim().to_string()));
    let answer = send(p, &system, &turns)?;
    info!("cloud: talk {text:?} → {:?}", answer.trim());
    super::model::checked_talk(&answer)
}

// ---- Settings → Assistant ----

#[derive(Debug, Serialize)]
pub struct CloudState {
    pub on: bool,
    pub provider: String,
    /// The model named in Settings (empty: the provider's own).
    pub model: String,
    pub has_key: bool,
    /// The key's last four characters, so you can tell which one it is.
    pub key_end: String,
    pub providers: &'static [Provider],
    /// Why it isn't used now (none: it is).
    pub not_used: Option<&'static str>,
}

fn cloud_state_of(p: &Prefs) -> CloudState {
    let n = p.key.chars().count();
    CloudState {
        on: p.on,
        provider: p.provider.clone(),
        model: p.model.clone(),
        has_key: !p.key.is_empty(),
        key_end: if n > 8 { p.key.chars().skip(n - 4).collect() } else { String::new() },
        providers: PROVIDERS,
        not_used: not_used(p, web::load_prefs().mode),
    }
}

#[tauri::command]
pub fn cloud_state() -> CloudState {
    cloud_state_of(&load_prefs())
}

/// `key`: a new key, "" to forget it, none to keep the one saved.
#[tauri::command]
pub fn set_cloud(on: bool, provider: String, model: String, key: Option<String>) -> Result<CloudState, String> {
    let old = load_prefs();
    if web::provider(&provider).is_none() {
        return Err(format!("{provider:?} isn't a cloud AI I know"));
    }
    // Another provider: the old key isn't for it.
    let key = match key {
        Some(k) => clean_word(&k),
        None if provider == old.provider => old.key.clone(),
        None => String::new(),
    };
    let p = Prefs { on, provider, model: clean_word(&model), key };
    save_prefs(&p)?;
    info!("cloud: {} {} {}{}", if p.on { "on" } else { "off" }, p.provider, p.model(), if p.key.is_empty() { ", no key" } else { "" });
    Ok(cloud_state_of(&p))
}

/// Settings' Test button: one short exchange with the cloud AI.
#[tauri::command]
pub async fn cloud_test() -> Result<String, String> {
    let p = load_prefs();
    if p.key.is_empty() {
        return Err("Enter a key first.".into());
    }
    if web::load_prefs().mode == web::Mode::Off {
        return Err("Internet is Off in Settings → Assistant.".into());
    }
    tauri::async_runtime::spawn_blocking(move || {
        let started = Instant::now();
        send(&p, "Answer in five words or fewer.", &[("user", "Say hello.".to_string())]).map(|said| format!("{} answered in {:.1} s: “{}”", p.provider().name, started.elapsed().as_secs_f32(), said.trim()))
    })
    .await
    .unwrap_or_else(|e| Err(e.to_string()))
    .map_err(|e| {
        let mut c = e.chars();
        c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str() + ".").unwrap_or_default()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn prefs(provider: &str, model: &str) -> Prefs {
        Prefs { on: true, provider: provider.into(), model: model.into(), key: "sk-test-1234567890".into() }
    }

    #[test]
    fn settings() {
        assert_eq!(parse_prefs(""), Prefs::default(), "off, by default");
        assert!(!Prefs::default().on);
        let p = prefs("groq", "");
        assert_eq!(parse_prefs(&prefs_text(&p)), p);
        assert_eq!(p.model(), "llama-3.3-70b-versatile", "the provider's own model");
        assert_eq!(parse_prefs("provider=elsewhere\nkey= a b\n"), Prefs { key: "ab".into(), ..Prefs::default() }, "a provider not in the list isn't taken");
        assert_eq!(not_used(&Prefs::default(), web::Mode::Always), Some("it's switched off"));
        assert_eq!(not_used(&Prefs { key: String::new(), ..p.clone() }, web::Mode::Always), Some("there's no key"));
        assert_eq!(not_used(&p, web::Mode::Ask), Some("Internet isn't set to Always"));
        assert_eq!(not_used(&p, web::Mode::Off), Some("Internet isn't set to Always"));
        assert_eq!(not_used(&p, web::Mode::Always), None);
        for provider in PROVIDERS {
            assert!(web::allowed(provider.url), "{} goes to one of HOSTS", provider.id);
        }
        let s = cloud_state_of(&p);
        assert_eq!(s.key_end, "7890");
        let json = serde_json::to_string(&s).unwrap();
        assert!(!json.contains("sk-test"), "the key isn't sent back to the page");
    }

    #[test]
    fn requests() {
        let (headers, body) = request(&prefs("anthropic", ""), "Be brief.", &[("user", "hi".into())]);
        assert!(headers.contains(&"x-api-key: sk-test-1234567890".to_string()));
        assert!(headers.contains(&"anthropic-version: 2026-10-21".to_string()));
        assert!(headers.contains(&"anthropic-beta: server-side-fallback-2026-07-01".to_string()));
        let v: Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["model"], "claude-opus-5-5");
        assert_eq!(v["system"], "Be brief.");
        assert_eq!(v["messages"], json!([{"role": "user", "content": "hi"}]));
        assert_eq!(v["output_config"]["effort"], "low");
        assert_eq!(v["fallbacks"], "default");
        assert!(v.get("temperature").is_none(), "2026-10-21 refuses temperature");
        let (headers, body) = request(&prefs("anthropic", "claude-haiku-4-5"), "x", &[("user", "hi".into())]);
        let v: Value = serde_json::from_str(&body).unwrap();
        assert!(v.get("output_config").is_none() && v.get("fallbacks").is_none());
        assert_eq!(headers.len(), 3);

        let (headers, body) = request(&prefs("gemini", ""), "Be brief.", &[("user", "hi".into()), ("assistant", "hello".into()), ("user", "more".into())]);
        assert_eq!(headers[0], "Authorization: Bearer sk-test-1234567890");
        let v: Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["model"], "gemini-2.5-flash");
        assert_eq!(v["messages"][0], json!({"role": "system", "content": "Be brief."}));
        assert_eq!(v["messages"][3], json!({"role": "user", "content": "more"}));
    }

    #[test]
    fn answers() {
        let a = prefs("anthropic", "");
        assert_eq!(answer_text(&a, r#"{"content":[{"type":"thinking","thinking":""},{"type":"text","text":"mute(on=True)"}],"stop_reason":"end_turn"}"#), Ok("mute(on=True)".into()));
        assert!(answer_text(&a, r#"{"content":[],"stop_reason":"refusal"}"#).is_err());
        let o = prefs("openai", "");
        assert_eq!(answer_text(&o, r#"{"choices":[{"message":{"role":"assistant","content":"Hi there!"}}]}"#), Ok("Hi there!".into()));
        assert!(answer_text(&o, "<html>").is_err());
        assert_eq!(trouble(401, r#"{"error":{"message":"invalid x-api-key"}}"#), "the cloud AI didn't accept the key (401: invalid x-api-key)");
        assert_eq!(trouble(429, ""), "the cloud AI is busy, or its quota is used up (429)");
        assert_eq!(calls_line("```\nchange_volume(by=10)\n```"), "change_volume(by=10)");
        assert_eq!(calls_line("\n`none`\nBecause…"), "none");
        let steps = super::super::model::read_answer(&calls_line("mute(on=False); set_volume(percent=60)")).unwrap();
        assert_eq!(steps.len(), 2);
    }

    #[test]
    fn key_stays_off_the_command_line() {
        let (headers, body) = request(&prefs("openai", ""), "say \"hi\" \\ there", &[("user", "x".into())]);
        let url = web::provider("openai").unwrap().url;
        let c = web::post_config(url, &headers, &body);
        assert!(c.starts_with(&format!("url = \"{url}\"\n")));
        assert!(c.contains("header = \"Authorization: Bearer sk-test-1234567890\"\n"));
        // The body's own escapes survive curl's quoting: \" → \\\", \\ → \\\\.
        assert!(c.contains(r#"say \\\"hi\\\" \\\\ there"#), "{c}");
        assert_eq!(web::config_quote("a\nb\u{7}c"), "\"a\\nbc\"");
    }
}
