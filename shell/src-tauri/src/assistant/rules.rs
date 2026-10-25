//! 19c: built-in rules, offline: what Siva understands from a typed (later spoken, 19d) request,
//! as tool steps for `policy`. English first; several requests in one go ("volume 30 and dark
//! mode", "open files then put it on the left"). Window and app names stay text here; `run`
//! matches them to real windows and apps.
//!
//! Pure. Anything not understood is `Parsed::Unknown`, and Siva says so (a local model may take
//! those later, 19f).

use serde_json::{json, Value};

#[derive(Debug, Clone, PartialEq)]
pub struct Step {
    pub tool: &'static str,
    pub args: Value,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Parsed {
    Steps(Vec<Step>),
    Hello,
    Help,
    /// 19k: small talk, answered at once.
    Talk(&'static str),
    /// Not understood: the part that wasn't.
    Unknown(String),
}

fn step(tool: &'static str, args: Value) -> Step {
    Step { tool, args }
}

/// Lower case, `'` and `’` dropped ("what's" → "whats"), other punctuation a space, words single
/// spaced, polite and addressing words off the front.
pub fn normalize(text: &str) -> String {
    let mapped: String = text
        .to_lowercase()
        .chars()
        .filter(|c| *c != '\'' && *c != '’')
        .map(|c| if c.is_alphanumeric() || c == '%' || c == '-' || c == '+' { c } else { ' ' })
        .collect();
    let mut words: Vec<&str> = mapped.split_whitespace().collect();
    loop {
        let before = words.len();
        for lead in [&["hey"][..], &["siva"], &["please"], &["can", "you"], &["could", "you"], &["would", "you"], &["i", "want", "to"], &["i", "want"]] {
            if words.starts_with(lead) {
                words.drain(..lead.len());
            }
        }
        if words.last() == Some(&"please") {
            words.pop();
        }
        if words.len() == before {
            break;
        }
    }
    words.join(" ")
}

const NUMBERS: &[(&str, i64)] = &[
    ("zero", 0), ("five", 5), ("ten", 10), ("fifteen", 15), ("twenty", 20), ("twenty-five", 25), ("thirty", 30),
    ("forty", 40), ("fifty", 50), ("half", 50), ("sixty", 60), ("seventy", 70), ("seventy-five", 75), ("eighty", 80),
    ("ninety", 90), ("hundred", 100), ("max", 100), ("full", 100), ("one", 1), ("two", 2), ("three", 3), ("four", 4),
    ("six", 6), ("seven", 7), ("eight", 8), ("nine", 9),
];

fn number(word: &str) -> Option<i64> {
    let w = word.trim_end_matches('%');
    w.parse().ok().or_else(|| NUMBERS.iter().find(|(n, _)| *n == w).map(|(_, v)| *v))
}

fn first_number(words: &[&str]) -> Option<i64> {
    words.iter().find_map(|w| number(w))
}

fn has(words: &[&str], any: &[&str]) -> bool {
    words.iter().any(|w| any.contains(w))
}

/// On or off from the words ("turn off wifi", "wifi on", "disable bluetooth"); `None` if neither.
fn switch(words: &[&str]) -> Option<bool> {
    if has(words, &["off", "disable", "stop", "deactivate", "end", "no"]) {
        Some(false)
    } else if has(words, &["on", "enable", "start", "activate"]) {
        Some(true)
    } else {
        None
    }
}

/// The words after a verb, without "the"/"a"/"my" in front: the name of a window or app.
fn name_after(words: &[&str], skip: usize) -> Option<String> {
    let rest: Vec<&str> = words[skip.min(words.len())..].iter().copied().skip_while(|w| ["the", "a", "an", "my", "app", "window"].contains(w)).collect();
    let rest: Vec<&str> = rest.into_iter().filter(|w| !["window", "app", "application"].contains(w)).collect();
    (!rest.is_empty()).then(|| rest.join(" "))
}

/// Where a "put X on the left" goes, and X.
fn placement(words: &[&str]) -> Option<(String, &'static str)> {
    let to = if has(words, &["left"]) {
        "left"
    } else if has(words, &["right"]) {
        "right"
    } else {
        return None;
    };
    let verbs = ["put", "move", "snap", "place", "send", "show"];
    // 19k: "close the firefox on the left" is closing; "on the left" says which one.
    const OTHER: [&str; 14] = ["close", "quit", "exit", "kill", "minimize", "minimise", "hide", "maximize", "maximise", "focus", "switch", "bring", "open", "restore"];
    if words.first().is_some_and(|w| OTHER.contains(w)) {
        return None;
    }
    let skip = usize::from(words.first().is_some_and(|w| verbs.contains(w)));
    let name: Vec<&str> = words[skip..]
        .iter()
        .copied()
        .take_while(|w| !["to", "on", "at", "left", "right", "into", "in"].contains(w))
        .filter(|w| !["the", "a", "my", "window", "app"].contains(w))
        .collect();
    (!name.is_empty()).then(|| (name.join(" "), to))
}

/// One request, already split from the others.
fn clause(c: &str) -> Option<Vec<Step>> {
    let w: Vec<&str> = c.split_whitespace().collect();
    let first = *w.first()?;

    // "firefox and terminal side by side" (kept whole by `split`)
    if let Some(i) = c.find(" side by side") {
        let pair = c[..i].trim_start_matches("put ").trim_start_matches("show ").trim_start_matches("place ");
        let (a, b) = pair.split_once(" and ")?;
        return Some(vec![step("place_window", json!({"window": a.trim(), "where": "left"})), step("place_window", json!({"window": b.trim(), "where": "right"}))]);
    }

    // ---- reading ----
    // 19k: the time and the date (from this PC's clock).
    if (has(&w, &["time"]) && has(&w, &["what", "whats", "tell"])) || c == "time" || c == "time please" || (has(&w, &["date", "day"]) && has(&w, &["what", "whats", "todays", "today"])) {
        return Some(vec![step("date_time", json!({}))]);
    }
    // 19g: why things are as they are.
    if has(&w, &["slow", "laggy", "lagging", "lag", "sluggish", "cpu", "processor", "ram", "memory"]) || c.contains("system status") || c.contains("how is my computer") || c.contains("hogging") {
        return Some(vec![step("system_status", json!({}))]);
    }
    if c.contains("disk") || c.contains("storage") || c.contains("space left") || c.contains("free space") || c.contains("how much space") {
        return Some(vec![step("disk_space", json!({}))]);
    }
    if has(&w, &["sound", "audio", "music", "headset", "headphones"])
        && (has(&w, &["bad", "cutting", "cuts", "choppy", "crackling", "crackles", "stutters", "stuttering", "quality", "terrible", "awful", "tinny", "muffled", "breaking"]) || c.contains("like a phone"))
        || c == "sound check"
    {
        return Some(vec![step("sound_check", json!({}))]);
    }
    if c.contains("playing") && has(&w, &["what", "whats", "now"]) || c == "now playing" {
        return Some(vec![step("now_playing", json!({}))]);
    }
    if (has(&w, &["what", "whats", "how"]) && has(&w, &["volume", "loud"])) || c == "sound status" {
        return Some(vec![step("sound_status", json!({}))]);
    }
    // 19l: "how many firefox windows are open?"
    if c.starts_with("how many ") && (has(&w, &["windows", "window", "open", "running"])) {
        let name: Vec<&str> = w[2..].iter().copied().filter(|x| !["windows", "window", "are", "is", "open", "opened", "running", "there", "do", "i", "have", "of", "the", "alone"].contains(x)).collect();
        return Some(vec![if name.is_empty() { step("list_windows", json!({})) } else { step("list_windows", json!({"app": name.join(" ")})) }]);
    }
    if has(&w, &["windows"]) && has(&w, &["what", "whats", "which", "list", "open", "show"]) {
        return Some(vec![step("list_windows", json!({}))]);
    }
    if (has(&w, &["online", "connected", "internet"]) && has(&w, &["am", "are", "is"])) || c == "network status" || c == "wifi status" {
        return Some(vec![step("network_status", json!({}))]);
    }
    if has(&w, &["bluetooth"]) && (has(&w, &["status", "is"]) && switch(&w).is_none() || c.ends_with("status")) {
        return Some(vec![step("bluetooth_status", json!({}))]);
    }

    // ---- sound ----
    if has(&w, &["volume", "sound", "louder", "quieter", "softer"]) && !has(&w, &["output", "speaker", "headphones"]) || first == "louder" || first == "quieter" {
        if has(&w, &["unmute"]) {
            return Some(vec![step("mute", json!({"on": false}))]);
        }
        if has(&w, &["mute"]) || c == "sound off" {
            return Some(vec![step("mute", json!({"on": true}))]);
        }
        if let Some(n) = first_number(&w) {
            if (0..=100).contains(&n) {
                return Some(vec![step("set_volume", json!({"percent": n}))]);
            }
        }
        if has(&w, &["up", "louder", "increase", "raise"]) {
            return Some(vec![step("change_volume", json!({"by": 10}))]);
        }
        if has(&w, &["down", "quieter", "softer", "lower", "decrease", "reduce"]) {
            return Some(vec![step("change_volume", json!({"by": -10}))]);
        }
    }
    let mic = has(&w, &["mic", "microphone"]);
    match first {
        "mute" => return Some(vec![step("mute", if mic { json!({"on": true, "device": "microphone"}) } else { json!({"on": true}) })]),
        "unmute" => return Some(vec![step("mute", if mic { json!({"on": false, "device": "microphone"}) } else { json!({"on": false}) })]),
        _ => {}
    }

    // ---- media ----
    let music = w.len() == 1 || has(&w, &["music", "song", "video", "track", "it", "media", "playback", "youtube"]);
    if music {
        match first {
            "play" | "pause" | "resume" | "stop" => return Some(vec![step("media", json!({"action": "play-pause"}))]),
            "next" | "skip" => return Some(vec![step("media", json!({"action": "next"}))]),
            "previous" | "back" | "last" => return Some(vec![step("media", json!({"action": "previous"}))]),
            _ => {}
        }
    }

    // ---- desktops ----
    if has(&w, &["desktop", "workspace", "space"]) {
        if let Some(n) = first_number(&w).filter(|n| (1..=9).contains(n)) {
            return Some(vec![step("switch_desktop", json!({"number": n}))]);
        }
    }

    // ---- switches ----
    if c.contains("do not disturb") || has(&w, &["dnd"]) || c.contains("silence notifications") {
        let on = if c.contains("silence") { Some(true) } else { switch(&w) };
        return Some(vec![step("do_not_disturb", json!({"on": on.unwrap_or(true)}))]);
    }
    if c.contains("dark mode") || c.contains("dark theme") || c.contains("light mode") || c.contains("light theme") || c.contains("auto mode") || c.contains("automatic appearance") {
        let mode = if c.contains("auto") {
            "auto"
        } else if c.contains("light") || (c.contains("dark") && switch(&w) == Some(false)) {
            "light"
        } else {
            "dark"
        };
        return Some(vec![step("appearance", json!({"mode": mode}))]);
    }
    if c.contains("awake") || first == "caffeinate" {
        let on = !(has(&w, &["stop", "off", "dont", "no"]) || c.contains("let the screen sleep"));
        return Some(vec![step("keep_awake", json!({"on": on}))]);
    }
    if has(&w, &["wifi", "wi-fi", "wireless"]) {
        return switch(&w).map(|on| vec![step("wifi", json!({"on": on}))]);
    }
    if has(&w, &["bluetooth"]) {
        return switch(&w).map(|on| vec![step("bluetooth", json!({"on": on}))]);
    }

    // ---- screen ----
    if has(&w, &["screenshot", "screenshots", "screengrab"]) || c.contains("screen shot") {
        let of = if has(&w, &["area", "region", "part", "selection", "select"]) { "area" } else { "screen" };
        return Some(vec![step("screenshot", json!({"of": of}))]);
    }
    if first == "lock" && w.len() <= 3 {
        return Some(vec![step("lock_screen", json!({}))]);
    }

    // ---- session and power ----
    if c == "log out" || c == "logout" || c == "sign out" || c == "log me out" {
        return Some(vec![step("log_out", json!({}))]);
    }
    if c.starts_with("shut down") || c.starts_with("shutdown") || c.starts_with("power off") || c == "turn off the computer" || c == "turn off computer" {
        return Some(vec![step("power", json!({"action": "power-off"}))]);
    }
    if first == "restart" || first == "reboot" {
        if w.len() == 1 || has(&w, &["computer", "pc", "system", "machine"]) {
            return Some(vec![step("power", json!({"action": "restart"}))]);
        }
    }
    if ["sleep", "suspend", "go to sleep"].contains(&c) || c == "put the computer to sleep" {
        return Some(vec![step("power", json!({"action": "sleep"}))]);
    }

    // ---- windows ----
    if let Some((name, to)) = placement(&w) {
        return Some(vec![step("place_window", json!({"window": name, "where": to}))]);
    }
    // 19l: a bare "maximize" ("open firefox and maximize", "maximize the window") means the app
    // just named: "it".
    let named = || name_after(&w, 1).unwrap_or_else(|| "it".into());
    match first {
        "maximize" | "maximise" | "fill" | "enlarge" => return Some(vec![step("place_window", json!({"window": named(), "where": "fill"}))]),
        "restore" | "unmaximize" | "unmaximise" => return Some(vec![step("place_window", json!({"window": named(), "where": "restore"}))]),
        "minimize" | "minimise" | "hide" => return Some(vec![step("minimize_window", json!({"window": named()}))]),
        "close" | "quit" | "exit" => return Some(vec![step("close_window", json!({"window": named()}))]),
        // "focus" alone is the focus routine, not a window.
        "focus" => return name_after(&w, 1).map(|n| vec![step("focus_window", json!({"window": n}))]),
        _ => {}
    }
    // 19l: "switch window", "swap windows", "the other window", "previous window": Alt+Tab's one.
    if ["switch window", "switch windows", "swap window", "swap windows", "other window", "the other window", "next window", "previous window", "go back", "switch back"].contains(&c) {
        return Some(vec![step("focus_window", json!({"window": "other one"}))]);
    }
    // 19l: "bring terminal up".
    if let Some(name) = c.strip_prefix("bring ").and_then(|r| r.strip_suffix(" up")) {
        let n = name_after(&name.split_whitespace().collect::<Vec<_>>(), 0)?;
        return Some(vec![step("focus_window", json!({"window": n}))]);
    }
    if c.starts_with("switch to ") || c.starts_with("go to ") || c.starts_with("bring up ") || c.starts_with("show me ") {
        return name_after(&w, 2).map(|n| vec![step("focus_window", json!({"window": n}))]);
    }
    // 19j: "bring firefox back".
    if let Some(name) = c.strip_prefix("bring ").and_then(|r| r.strip_suffix(" back")) {
        let n = name_after(&name.split_whitespace().collect::<Vec<_>>(), 0)?;
        return Some(vec![step("focus_window", json!({"window": n}))]);
    }
    if c.starts_with("make ") && (c.ends_with(" bigger") || c.ends_with(" full screen") || c.ends_with(" fullscreen")) {
        let name = c.trim_start_matches("make ").trim_end_matches(" bigger").trim_end_matches(" full screen").trim_end_matches(" fullscreen");
        let n = name_after(&name.split_whitespace().collect::<Vec<_>>(), 0)?;
        return Some(vec![step("place_window", json!({"window": n, "where": "fill"}))]);
    }

    // ---- apps ----
    if ["open", "launch", "start", "run"].contains(&first) {
        return name_after(&w, 1).map(|n| vec![step("open_app", json!({"app": n}))]);
    }
    None
}

/// Split into requests at "and then", "then", "and", "," and ";", keeping "x and y side by side"
/// whole: the piece with "side by side" takes the one before it back ("open terminal and put
/// terminal and firefox side by side" is two requests, not one).
fn split(c: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for part in c.split(" and then ").flat_map(|p| p.split(" then ")) {
        let start = out.len();
        for piece in part.split(" and ").map(str::trim).filter(|p| !p.is_empty()) {
            if piece.contains("side by side") && out.len() > start {
                let before = out.pop().unwrap_or_default();
                out.push(format!("{before} and {piece}"));
            } else {
                out.push(piece.to_string());
            }
        }
    }
    out
}

/// 19k: what a person says in passing, and Siva's answer (anything longer goes to the model).
fn small_talk(c: &str) -> Option<&'static str> {
    let is = |list: &[&str]| list.contains(&c);
    Some(if is(&["how are you", "how are you doing", "how are you today", "hows it going", "how do you do", "you ok", "are you ok"]) {
        "I'm good, thanks for asking! What can I do for you?"
    } else if is(&["thank you", "thanks", "thank you so much", "thanks a lot", "cheers", "thanks siva", "thank you siva", "nice", "great", "awesome", "perfect", "cool", "good job", "well done"]) {
        "Happy to help."
    } else if is(&["who are you", "whats your name", "what is your name", "what are you", "who made you"]) {
        "I'm Siva, the assistant built into your desktop. I run right here on this PC, and nothing you say leaves it."
    } else if is(&["good night", "goodnight", "night"]) {
        "Good night! Sleep well."
    } else if is(&["bye", "goodbye", "see you", "see you later", "thats all", "that is all", "nothing", "never mind"]) {
        "Okay. Call me any time."
    } else if is(&["are you there", "you there", "are you listening", "can you hear me"]) {
        "I'm here, and I can hear you."
    } else {
        return None;
    })
}

/// 19e: a step as one line of a plan ("Open Firefox", "Put Terminal on the left").
pub fn describe(s: &Step) -> String {
    let text = |k: &str| s.args.get(k).and_then(Value::as_str).unwrap_or("").to_string();
    let on = s.args.get("on").and_then(Value::as_bool).unwrap_or(true);
    let int = |k: &str| s.args.get(k).and_then(Value::as_i64).unwrap_or(0);
    let title = |t: String| {
        let mut c = t.chars();
        c.next().map(|f| f.to_uppercase().chain(c).collect()).unwrap_or_default()
    };
    let switch = |what: &str| format!("{what} {}", if on { "on" } else { "off" });
    match s.tool {
        "list_windows" => "Look at the open windows".into(),
        "sound_status" => "Check the sound".into(),
        "network_status" => "Check the network".into(),
        "bluetooth_status" => "Check Bluetooth".into(),
        "now_playing" => "See what's playing".into(),
        "system_status" => "Check how busy the computer is".into(),
        "date_time" => "Check the time".into(),
        "disk_space" => "Check the disk space".into(),
        "sound_check" => "Check the sound quality".into(),
        "set_volume" => format!("Volume {}%", int("percent")),
        "change_volume" if int("by") < 0 => format!("Volume down {}", -int("by")),
        "change_volume" => format!("Volume up {}", int("by")),
        "mute" if text("device") == "microphone" => if on { "Mute the microphone".into() } else { "Unmute the microphone".into() },
        "mute" => if on { "Mute the sound".into() } else { "Unmute the sound".into() },
        "sound_output" => format!("Sound to {}", text("device")),
        "media" => match text("action").as_str() {
            "next" => "Next track".into(),
            "previous" => "Previous track".into(),
            _ => "Play or pause".into(),
        },
        "open_app" => format!("Open {}", title(text("app"))),
        "focus_window" => format!("Bring up {}", title(text("window"))),
        "minimize_window" => format!("Minimize {}", title(text("window"))),
        "place_window" => match text("where").as_str() {
            "left" => format!("Put {} on the left", title(text("window"))),
            "right" => format!("Put {} on the right", title(text("window"))),
            "fill" => format!("Maximize {}", title(text("window"))),
            _ => format!("Restore {}", title(text("window"))),
        },
        "switch_desktop" => format!("Go to desktop {}", int("number")),
        "do_not_disturb" => switch("Do Not Disturb"),
        "appearance" => format!("{} appearance", title(text("mode"))),
        "keep_awake" => switch("Keep Awake"),
        "screenshot" if text("of") == "area" => "Screenshot of an area".into(),
        "screenshot" => "Screenshot".into(),
        "lock_screen" => "Lock the screen".into(),
        "close_window" => format!("Close {}", title(text("window"))),
        "wifi" => switch("Wi-Fi"),
        "bluetooth" => switch("Bluetooth"),
        "log_out" => "Log out".into(),
        "power" => title(text("action").replace('-', " ")),
        other => other.replace('_', " "),
    }
}

/// 19d: a spoken answer to a waiting card ("cancel", "yes"), or None if it's a request. Only
/// the whole utterance counts: "stop the music" is a request. What a spoken yes may do is
/// `policy::confirmed`'s to say (nothing).
pub fn spoken_answer(text: &str) -> Option<super::policy::Spoken> {
    use super::policy::Spoken;
    const NO: [&str; 13] = ["cancel", "stop", "no", "nope", "never mind", "nevermind", "dont", "do not", "dont do it", "abort", "forget it", "no thanks", "cancel that"];
    const YES: [&str; 10] = ["yes", "yeah", "yep", "confirm", "do it", "ok", "okay", "go ahead", "sure", "yes please"];
    let t = normalize(text);
    if NO.contains(&t.as_str()) {
        Some(Spoken::No)
    } else if YES.contains(&t.as_str()) {
        Some(Spoken::Yes)
    } else {
        None
    }
}

/// 19h-2: what was heard, as `parse` should get it. whisper puts commas where you paused
/// ("Volume, 100"), which `parse` reads as two requests: if it doesn't understand the text, the
/// same without commas is tried.
///
/// 19f-2: then the same with near misses of the words below put right ("Maximate Firefox" →
/// "maximize firefox"), but only if that is understood.
pub fn for_voice(text: &str) -> String {
    let plain = text.replace(',', " ");
    let unknown = |t: &str| matches!(parse(t), Parsed::Unknown(_));
    if !unknown(text) {
        return text.to_string();
    }
    [plain.clone(), corrected(text), corrected(&plain)].into_iter().find(|t| !unknown(t)).unwrap_or_else(|| text.to_string())
}

/// The words commands start with or lean on, for `corrected`.
const COMMAND_WORDS: [&str; 26] = [
    "maximize", "minimize", "restore", "volume", "unmute", "mute", "louder", "quieter", "close", "launch", "start",
    "switch", "desktop", "screenshot", "bluetooth", "disturb", "appearance", "awake", "terminal", "firefox", "files",
    "settings", "previous", "next", "pause", "music",
];

/// Edit distance (Levenshtein).
fn distance(a: &str, b: &str) -> usize {
    let b: Vec<char> = b.chars().collect();
    let mut row: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.chars().enumerate() {
        let mut prev = row[0];
        row[0] = i + 1;
        for j in 0..b.len() {
            let here = row[j + 1];
            row[j + 1] = (prev + usize::from(ca != b[j])).min(row[j] + 1).min(here + 1);
            prev = here;
        }
    }
    row[b.len()]
}

/// `text` with each word of 5 letters or more that is a near miss of exactly one command word
/// put right (1 letter off for 5, 2 for longer).
fn corrected(text: &str) -> String {
    normalize(text)
        .split_whitespace()
        .map(|w| {
            let n = w.chars().count();
            if n < 5 || COMMAND_WORDS.contains(&w) {
                return w.to_string();
            }
            let most = if n == 5 { 1 } else { 2 };
            let near: Vec<&str> = COMMAND_WORDS.iter().copied().filter(|c| distance(w, c) <= most).collect();
            if near.len() == 1 { near[0].to_string() } else { w.to_string() }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

pub fn parse(text: &str) -> Parsed {
    // 19k: ", which is on the left" describes what came before it: not a request of its own.
    let lower = text.to_lowercase();
    let glued = [", which", ", that is", ", the one"].iter().fold(lower, |t, c| t.replace(c, &c[1..]));
    // Commas and semicolons separate requests too: mark them before punctuation goes.
    let marked = glued.replace([',', ';'], " and ");
    let c = normalize(&marked);
    if c.is_empty() {
        return Parsed::Unknown(String::new());
    }
    if ["hi", "hello", "vanakkam", "hey", "hi siva", "hello siva", "good morning", "good evening"].contains(&c.as_str()) {
        return Parsed::Hello;
    }
    if c == "help" || c.contains("what can you do") || c == "what do you do" {
        return Parsed::Help;
    }
    let said = normalize(text);
    if let Some(answer) = small_talk(said.strip_suffix(" siva").unwrap_or(&said)) {
        return Parsed::Talk(answer);
    }
    let mut steps = Vec::new();
    for part in split(&c) {
        match clause(&part) {
            Some(s) => steps.extend(s),
            None => return Parsed::Unknown(part),
        }
    }
    resolve_it(&mut steps);
    if steps.is_empty() {
        Parsed::Unknown(c)
    } else {
        Parsed::Steps(steps)
    }
}

/// 19l: each request in `text`, as steps or (the ones the rules don't know) its words; None if
/// it's a greeting, help or small talk, or empty. For the local model to get only the part it
/// must work out.
pub fn clauses(text: &str) -> Option<Vec<Result<Vec<Step>, String>>> {
    if !matches!(parse(text), Parsed::Unknown(_) | Parsed::Steps(_)) {
        return None;
    }
    let lower = text.to_lowercase();
    let glued = [", which", ", that is", ", the one"].iter().fold(lower, |t, c| t.replace(c, &c[1..]));
    let c = normalize(&glued.replace([',', ';'], " and "));
    let parts: Vec<Result<Vec<Step>, String>> = split(&c).into_iter().map(|p| clause(&p).ok_or(p)).collect();
    (!parts.is_empty()).then_some(parts)
}

/// 19l: a remark, not a request ("there are 2 firefox open", "i like this song"): talked about,
/// never acted on.
pub fn is_remark(text: &str) -> bool {
    let c = normalize(text);
    const OPENINGS: [&str; 16] = [
        "there are ", "there is ", "theres ", "i have ", "ive ", "it is ", "its ", "this is ", "that is ", "thats ", "i think ", "i like ",
        "i love ", "i hate ", "i am ", "im ",
    ];
    OPENINGS.iter().any(|o| c.starts_with(o)) && !c.contains(" please")
}

/// "open files then put it on the left": "it" is the app just opened (the window just named).
pub fn resolve_it(steps: &mut [Step]) {
    let mut last: Option<String> = None;
    for s in steps.iter_mut() {
        if let Some(name) = s.args.get("app").and_then(Value::as_str) {
            last = Some(name.to_string());
        }
        if s.args.get("window").and_then(Value::as_str) == Some("it") {
            if let Some(name) = &last {
                s.args["window"] = json!(name);
            }
        }
        if let Some(name) = s.args.get("window").and_then(Value::as_str) {
            if name != "it" {
                last = Some(name.to_string());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn one(text: &str) -> (String, Value) {
        match parse(text) {
            Parsed::Steps(s) if s.len() == 1 => (s[0].tool.to_string(), s[0].args.clone()),
            other => panic!("{text:?} → {other:?}"),
        }
    }

    fn tools(text: &str) -> Vec<&'static str> {
        match parse(text) {
            Parsed::Steps(s) => s.iter().map(|s| s.tool).collect(),
            other => panic!("{text:?} → {other:?}"),
        }
    }

    #[test]
    fn tidying() {
        assert_eq!(normalize("Hey Siva, please set the Volume to 30%!"), "set the volume to 30%");
        assert_eq!(normalize("What's playing?"), "whats playing");
        assert_eq!(normalize("  Could you   open Firefox please "), "open firefox");
    }

    #[test]
    fn sound() {
        assert_eq!(one("volume 30"), ("set_volume".into(), json!({"percent": 30})));
        assert_eq!(one("Set the volume to 75%"), ("set_volume".into(), json!({"percent": 75})));
        assert_eq!(one("volume to half"), ("set_volume".into(), json!({"percent": 50})));
        assert_eq!(one("turn the volume up"), ("change_volume".into(), json!({"by": 10})));
        assert_eq!(one("louder"), ("change_volume".into(), json!({"by": 10})));
        assert_eq!(one("make it quieter"), ("change_volume".into(), json!({"by": -10})));
        assert_eq!(one("mute"), ("mute".into(), json!({"on": true})));
        assert_eq!(one("unmute the sound"), ("mute".into(), json!({"on": false})));
        assert_eq!(one("mute the microphone"), ("mute".into(), json!({"on": true, "device": "microphone"})));
        assert_eq!(one("what's the volume"), ("sound_status".into(), json!({})));
    }

    #[test]
    fn media() {
        assert_eq!(one("pause"), ("media".into(), json!({"action": "play-pause"})));
        assert_eq!(one("play the music"), ("media".into(), json!({"action": "play-pause"})));
        assert_eq!(one("next song"), ("media".into(), json!({"action": "next"})));
        assert_eq!(one("previous track"), ("media".into(), json!({"action": "previous"})));
        assert_eq!(one("what's playing"), ("now_playing".into(), json!({})));
    }

    #[test]
    fn windows_and_apps() {
        assert_eq!(one("open firefox"), ("open_app".into(), json!({"app": "firefox"})));
        assert_eq!(one("launch the terminal"), ("open_app".into(), json!({"app": "terminal"})));
        assert_eq!(one("put firefox on the left"), ("place_window".into(), json!({"window": "firefox", "where": "left"})));
        assert_eq!(one("move the terminal window to the right side"), ("place_window".into(), json!({"window": "terminal", "where": "right"})));
        assert_eq!(one("maximize gedit"), ("place_window".into(), json!({"window": "gedit", "where": "fill"})));
        assert_eq!(one("make files full screen"), ("place_window".into(), json!({"window": "files", "where": "fill"})));
        assert_eq!(one("restore firefox"), ("place_window".into(), json!({"window": "firefox", "where": "restore"})));
        assert_eq!(one("minimize the terminal"), ("minimize_window".into(), json!({"window": "terminal"})));
        assert_eq!(one("close gedit"), ("close_window".into(), json!({"window": "gedit"})));
        assert_eq!(one("switch to firefox"), ("focus_window".into(), json!({"window": "firefox"})));
        assert_eq!(one("which windows are open"), ("list_windows".into(), json!({})));
        let pair = parse("put firefox and terminal side by side");
        assert_eq!(
            pair,
            Parsed::Steps(vec![
                step("place_window", json!({"window": "firefox", "where": "left"})),
                step("place_window", json!({"window": "terminal", "where": "right"})),
            ])
        );
    }

    #[test]
    fn switches_and_desktops() {
        assert_eq!(one("dark mode"), ("appearance".into(), json!({"mode": "dark"})));
        assert_eq!(one("turn off dark mode"), ("appearance".into(), json!({"mode": "light"})));
        assert_eq!(one("light mode"), ("appearance".into(), json!({"mode": "light"})));
        assert_eq!(one("do not disturb"), ("do_not_disturb".into(), json!({"on": true})));
        assert_eq!(one("turn off do not disturb"), ("do_not_disturb".into(), json!({"on": false})));
        assert_eq!(one("keep the screen awake"), ("keep_awake".into(), json!({"on": true})));
        assert_eq!(one("stop keeping awake"), ("keep_awake".into(), json!({"on": false})));
        assert_eq!(one("turn wifi off"), ("wifi".into(), json!({"on": false})));
        assert_eq!(one("enable bluetooth"), ("bluetooth".into(), json!({"on": true})));
        assert_eq!(one("go to desktop 2"), ("switch_desktop".into(), json!({"number": 2})));
        assert_eq!(one("workspace three"), ("switch_desktop".into(), json!({"number": 3})));
        assert_eq!(one("take a screenshot"), ("screenshot".into(), json!({"of": "screen"})));
        assert_eq!(one("screenshot of an area"), ("screenshot".into(), json!({"of": "area"})));
        assert_eq!(one("lock the screen"), ("lock_screen".into(), json!({})));
        assert_eq!(one("shut down"), ("power".into(), json!({"action": "power-off"})));
        assert_eq!(one("restart the computer"), ("power".into(), json!({"action": "restart"})));
        assert_eq!(one("log out"), ("log_out".into(), json!({})));
        // "wifi" alone says neither on nor off.
        assert!(matches!(parse("wifi"), Parsed::Unknown(_)));
    }

    #[test]
    fn several_at_once() {
        assert_eq!(tools("volume 30 and dark mode"), ["set_volume", "appearance"]);
        assert_eq!(tools("mute, lock the screen"), ["mute", "lock_screen"]);
        let p = parse("open files then put it on the left");
        assert_eq!(
            p,
            Parsed::Steps(vec![step("open_app", json!({"app": "files"})), step("place_window", json!({"window": "files", "where": "left"}))])
        );
        // One part not understood: say which, don't do half.
        assert_eq!(parse("volume 30 and bake a cake"), Parsed::Unknown("bake a cake".into()));
    }

    #[test]
    fn talk() {
        assert_eq!(parse("Vanakkam!"), Parsed::Hello);
        assert_eq!(parse("hello siva"), Parsed::Hello);
        assert_eq!(parse("what can you do?"), Parsed::Help);
        assert_eq!(parse(""), Parsed::Unknown(String::new()));
        assert!(matches!(parse("delete all my files"), Parsed::Unknown(_)));
        assert!(matches!(parse("run sudo rm -rf"), Parsed::Steps(_)), "becomes open_app \"sudo rm -rf\", which matches no app");
    }

    #[test]
    fn side_by_side_among_others() {
        let tools = |t: &str| match parse(t) {
            Parsed::Steps(steps) => steps.iter().map(|s| (s.tool, s.args["window"].as_str().unwrap_or("").to_string())).collect::<Vec<_>>(),
            other => panic!("{t}: {other:?}"),
        };
        let p = |w: &str| ("place_window", w.to_string());
        assert_eq!(tools("open terminal and put terminal and firefox side by side"), [("open_app", String::new()), p("terminal"), p("firefox")]);
        assert_eq!(tools("put firefox and files side by side and do not disturb"), [p("firefox"), p("files"), ("do_not_disturb", String::new())]);
        assert_eq!(tools("firefox and files side by side"), [p("firefox"), p("files")]);
    }

    #[test]
    fn plan_lines() {
        let lines = |t: &str| match parse(t) {
            Parsed::Steps(steps) => steps.iter().map(describe).collect::<Vec<_>>(),
            other => panic!("{other:?}"),
        };
        assert_eq!(lines("open firefox then put it on the left"), ["Open Firefox", "Put Firefox on the left"]);
        assert_eq!(lines("volume 30, dark mode, do not disturb"), ["Volume 30%", "Dark appearance", "Do Not Disturb on"]);
        assert_eq!(lines("restart"), ["Restart"]);
        assert_eq!(lines("power off"), ["Power off"]);
        // Every tool has its own line (none falls through to its bare name).
        for t in super::super::tools::TOOLS {
            let line = describe(&step(t.name, json!({})));
            assert_ne!(line, t.name.replace('_', " "), "{}", t.name);
        }
    }

    #[test]
    fn the_gaps_from_the_log() {
        // 2026-10-24: all of these went to the model, and three came back wrong.
        let tools = |t: &str| match parse(t) {
            Parsed::Steps(s) => s.iter().map(|s| (s.tool, s.args.clone())).collect::<Vec<_>>(),
            other => panic!("{t}: {other:?}"),
        };
        assert_eq!(tools("Open Firefox and maximize"), [("open_app", json!({"app": "firefox"})), ("place_window", json!({"window": "firefox", "where": "fill"}))]);
        assert_eq!(tools("Open Firefox and then maximize the window"), [("open_app", json!({"app": "firefox"})), ("place_window", json!({"window": "firefox", "where": "fill"}))]);
        assert_eq!(tools("Bring terminal up"), [("focus_window", json!({"window": "terminal"}))]);
        assert_eq!(tools("How many Firefox windows are open?"), [("list_windows", json!({"app": "firefox"}))]);
        assert_eq!(tools("how many windows are open"), [("list_windows", json!({}))]);
        assert_eq!(tools("Switch window"), [("focus_window", json!({"window": "other one"}))]);
        assert_eq!(tools("Close last Firefox"), [("close_window", json!({"window": "last firefox"}))]);
        assert!(is_remark("There are 2 Firefox open"));
        assert!(is_remark("I like this song"));
        assert!(!is_remark("open firefox"));
        // Only the part the rules don't know goes to the model.
        let parts = clauses("open firefox and make it sparkle").unwrap();
        assert_eq!(parts.len(), 2);
        assert!(parts[0].is_ok());
        assert_eq!(parts[1].as_ref().unwrap_err(), "make it sparkle");
        assert!(clauses("hello").is_none());
    }

    #[test]
    fn talking() {
        assert!(matches!(parse("How are you?"), Parsed::Talk(t) if t.starts_with("I'm good")));
        assert!(matches!(parse("Thanks, Siva"), Parsed::Talk(_)));
        assert!(matches!(parse("what's your name"), Parsed::Talk(t) if t.contains("Siva")));
        assert_eq!(one("what time is it"), ("date_time".into(), json!({})));
        assert_eq!(one("what's the date today"), ("date_time".into(), json!({})));
        assert_eq!(one("what day is it"), ("date_time".into(), json!({})));
        assert!(matches!(parse("tell me about the moon"), Parsed::Unknown(_)), "the model's to answer");
    }

    #[test]
    fn describing_windows() {
        // The user's own requests, 2026-10-24.
        assert_eq!(one("Close the Firefox, which is put on the left side"), ("close_window".into(), json!({"window": "firefox which is put on the left side"})));
        assert_eq!(one("Close Firefox, which is in the left side"), ("close_window".into(), json!({"window": "firefox which is in the left side"})));
        assert_eq!(one("close the firefox which is in back"), ("close_window".into(), json!({"window": "firefox which is in back"})));
        assert_eq!(one("close all the firefox windows"), ("close_window".into(), json!({"window": "all the firefox windows"})));
        assert_eq!(one("minimize the other one"), ("minimize_window".into(), json!({"window": "other one"})));
        // Still two requests.
        assert!(matches!(parse("open files, dark mode"), Parsed::Steps(s) if s.len() == 2));
    }

    #[test]
    fn bringing_back() {
        assert_eq!(one("bring firefox back"), ("focus_window".into(), json!({"window": "firefox"})));
        assert_eq!(one("bring the terminal back"), ("focus_window".into(), json!({"window": "terminal"})));
    }

    #[test]
    fn why_questions() {
        assert_eq!(one("why is my computer so slow"), ("system_status".into(), json!({})));
        assert_eq!(one("what's using all the memory"), ("system_status".into(), json!({})));
        assert_eq!(one("how much disk space is left"), ("disk_space".into(), json!({})));
        assert_eq!(one("why is the sound cutting out"), ("sound_check".into(), json!({})));
        assert_eq!(one("my headphones sound terrible"), ("sound_check".into(), json!({})));
        assert_eq!(one("why does the music sound like a phone"), ("sound_check".into(), json!({})));
        assert_eq!(one("volume 30"), ("set_volume".into(), json!({"percent": 30})), "still the volume");
    }

    #[test]
    fn near_misses() {
        assert_eq!(distance("maximate", "maximize"), 2);
        assert_eq!(distance("", "abc"), 3);
        assert_eq!(distance("files", "files"), 0);
        assert_eq!(for_voice("Maximate Firefox"), "maximize firefox");
        assert_eq!(parse(&for_voice("Maximate Firefox")), Parsed::Steps(vec![step("place_window", json!({"window": "firefox", "where": "fill"}))]));
        assert_eq!(for_voice("minimise the terminal"), "minimise the terminal", "understood already");
        assert_eq!(for_voice("Minimice the terminal"), "minimize the terminal");
        assert_eq!(for_voice("Volume 30"), "Volume 30", "understood: as heard");
        assert_eq!(for_voice("Two points"), "Two points", "nothing near: as heard");
        assert_eq!(corrected("clone firefox"), "close firefox", "one letter off a 5-letter word");
        assert_eq!(corrected("glass firefox"), "glass firefox", "two off a 5-letter word: no");
    }

    #[test]
    fn heard_with_commas() {
        assert_eq!(parse(&for_voice("Volume, 100")), Parsed::Steps(vec![step("set_volume", json!({"percent": 100}))]));
        assert_eq!(for_voice("volume 30, dark mode"), "volume 30, dark mode", "understood: kept as two requests");
        assert_eq!(for_voice("Matching my terminals"), "Matching my terminals", "not understood either way: as heard");
    }

    #[test]
    fn spoken_answers() {
        use crate::assistant::policy::Spoken;
        assert_eq!(spoken_answer("Cancel."), Some(Spoken::No));
        assert_eq!(spoken_answer("Siva, stop!"), Some(Spoken::No));
        assert_eq!(spoken_answer("Don't."), Some(Spoken::No));
        assert_eq!(spoken_answer("never mind"), Some(Spoken::No));
        assert_eq!(spoken_answer("Yes, please."), Some(Spoken::Yes));
        assert_eq!(spoken_answer("OK"), Some(Spoken::Yes));
        assert_eq!(spoken_answer("stop the music"), None, "a request, not an answer");
        assert_eq!(spoken_answer("yes close firefox"), None);
    }
}
