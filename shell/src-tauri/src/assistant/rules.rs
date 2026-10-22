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
    if c.contains("playing") && has(&w, &["what", "whats", "now"]) || c == "now playing" {
        return Some(vec![step("now_playing", json!({}))]);
    }
    if (has(&w, &["what", "whats", "how"]) && has(&w, &["volume", "loud"])) || c == "sound status" {
        return Some(vec![step("sound_status", json!({}))]);
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
    match first {
        "maximize" | "maximise" | "fill" | "enlarge" => return name_after(&w, 1).map(|n| vec![step("place_window", json!({"window": n, "where": "fill"}))]),
        "restore" | "unmaximize" | "unmaximise" => return name_after(&w, 1).map(|n| vec![step("place_window", json!({"window": n, "where": "restore"}))]),
        "minimize" | "minimise" | "hide" => return name_after(&w, 1).map(|n| vec![step("minimize_window", json!({"window": n}))]),
        "close" | "quit" | "exit" => return name_after(&w, 1).map(|n| vec![step("close_window", json!({"window": n}))]),
        "focus" => return name_after(&w, 1).map(|n| vec![step("focus_window", json!({"window": n}))]),
        _ => {}
    }
    if c.starts_with("switch to ") || c.starts_with("go to ") || c.starts_with("bring up ") || c.starts_with("show me ") {
        return name_after(&w, 2).map(|n| vec![step("focus_window", json!({"window": n}))]);
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
/// whole.
fn split(c: &str) -> Vec<String> {
    let mut out = Vec::new();
    for part in c.split(" and then ").flat_map(|p| p.split(" then ")) {
        if part.contains(" side by side") {
            out.push(part.trim().to_string());
            continue;
        }
        out.extend(part.split(" and ").map(|s| s.trim().to_string()));
    }
    out.retain(|s| !s.is_empty());
    out
}

pub fn parse(text: &str) -> Parsed {
    // Commas and semicolons separate requests too: mark them before punctuation goes.
    let marked = text.replace([',', ';'], " and ");
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
    let mut steps = Vec::new();
    for part in split(&c) {
        match clause(&part) {
            Some(s) => steps.extend(s),
            None => return Parsed::Unknown(part),
        }
    }
    // "open files then put it on the left": "it" is the app just opened.
    let mut last: Option<String> = None;
    for s in &mut steps {
        if let Some(name) = s.args.get("app").and_then(Value::as_str) {
            last = Some(name.to_string());
        }
        if s.args.get("window").and_then(Value::as_str) == Some("it") {
            if let Some(name) = &last {
                s.args["window"] = json!(name);
            }
        }
        if let Some(name) = s.args.get("window").and_then(Value::as_str) {
            last = Some(name.to_string());
        }
    }
    if steps.is_empty() {
        Parsed::Unknown(c)
    } else {
        Parsed::Steps(steps)
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
}
