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
    let verbs = ["put", "move", "snap", "place", "send", "show", "keep", "set", "have", "dock"];
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

/// 20b: "open bluetooth settings", "show control center", "open my downloads", "search
/// youtube for lofi music", "send firefox to desktop 2", "show the desktop", "move the dock to
/// the left", "turn off animations".
fn desktop_clause(c: &str, w: &[&str]) -> Option<Vec<Step>> {
    let opens = ["open", "show", "go", "bring", "display", "launch", "take", "pull", "give", "get"].contains(w.first()?);
    // A window verb before "all" is about windows ("minimize all applications"), not a part.
    let about_windows = ["minimize", "minimise", "hide", "close", "quit", "exit", "kill", "restore", "maximize", "maximise"].contains(w.first()?);
    if c.contains("quick settings") {
        return Some(vec![step("show_part", json!({"part": "control-center"}))]);
    }
    if ["search apps", "search for an app", "find an app", "search for apps", "search applications"].contains(&c) {
        return Some(vec![step("show_part", json!({"part": "spotlight"}))]);
    }
    // Settings, at a pane ("open the bluetooth setting", "change the wallpaper", "open about",
    // "open sound": a pane's own name after open).
    if has(w, &["settings", "setting", "preferences"]) && !about_windows && switch(w).is_none() {
        let pane = pane_for(c);
        return Some(vec![step("open_settings", pane.map(|p| json!({"pane": p})).unwrap_or(json!({})))]);
    }
    if let Some(p) = pane_named(c) {
        return Some(vec![step("open_settings", json!({"pane": p}))]);
    }
    // Parts of the desktop.
    let part = if c.contains("control center") || c.contains("control centre") || c.contains("quick settings") {
        Some("control-center")
    } else if c.contains("notification") {
        Some("notifications")
    } else if c.contains("spotlight") {
        Some("spotlight")
    } else if !about_windows && (c.contains("launchpad") || c.contains("all apps") || c.contains("my apps") || c.contains("app grid") || c.contains("all applications") || c.contains("all programs")) {
        Some("launchpad")
    } else if c.contains("overview") || c.contains("all desktops") || c.contains("mission control") || c.contains("workspaces") {
        Some("overview")
    } else if c.contains("window switcher") || c.contains("switcher") {
        Some("switcher")
    } else if has(w, &["menu"]) && (has(w, &["kompact", "compact", "logo", "main", "apple", "start", "system"]) || c == "open menu" || c == "show menu" || c == "open the menu" || c == "show the menu") {
        Some("menu")
    } else {
        None
    };
    if let (Some(p), true) = (part, (opens || w.len() <= 3) && !about_windows) {
        return Some(vec![step("show_part", json!({"part": p}))]);
    }
    // Folders.
    if opens || first_is(w, "open") {
        if let Some(f) = folder_for(w) {
            return Some(vec![step("open_folder", json!({"folder": f}))]);
        }
    }
    // Searching in the browser: "search youtube for x", "search for x on youtube", "google x",
    // "look up x", "play x on youtube", "youtube x".
    if let Some(s) = search(c) {
        return Some(vec![s]);
    }
    for (site, name) in [("youtube", "youtube"), ("wikipedia", "wikipedia")] {
        let q = c
            .strip_prefix("play ")
            .and_then(|r| r.strip_suffix(&format!(" on {name}")).or(r.strip_suffix(&format!(" in {name}"))))
            .or_else(|| c.strip_prefix(&format!("{name} ")).filter(|r| !r.is_empty() && !["search", "for", "please"].contains(r)));
        if let Some(q) = q {
            return Some(vec![step("search_web", json!({"query": q.trim(), "site": site}))]);
        }
    }
    // Web addresses: "go to youtube.com", "open github.com" (the dots are gone by now:
    // "github com"), "go to en wikipedia org".
    const ENDINGS: [&str; 14] = ["com", "org", "net", "io", "in", "dev", "ai", "co", "edu", "gov", "app", "uk", "me", "tv"];
    let rest: Vec<&str> = w[1..].iter().copied().filter(|x| *x != "to").collect();
    if opens && (2..=4).contains(&rest.len()) && rest.last().is_some_and(|e| ENDINGS.contains(e)) {
        return Some(vec![step("open_link", json!({"url": rest.join(".")}))]);
    }
    // The desktop.
    if ["show desktop", "show the desktop", "show my desktop", "clear the screen", "show me the desktop"].contains(&c) {
        return Some(vec![step("show_desktop", json!({}))]);
    }
    // "minimize all applications", "hide everything", "close all apps": every window (closing
    // still asks first).
    let every = has(w, &["all", "everything", "every"]) && w.len() <= 4;
    if every && ["minimize", "minimise", "hide"].contains(w.first()?) {
        return Some(vec![step("minimize_window", json!({"window": "all windows"}))]);
    }
    if every && ["close", "quit", "exit", "kill"].contains(w.first()?) {
        return Some(vec![step("close_window", json!({"window": "all windows"}))]);
    }
    // "firefox to desktop 2" (no verb).
    if let Some(i) = c.find(" to desktop ").or(c.find(" to workspace ")) {
        // Verbs have their own rules ("go to desktop 2" switches, "send x to desktop 2" below).
        let verbless = !["go", "switch", "take", "move", "send", "put", "throw", "bring", "get"].contains(w.first()?);
        let name = c[..i].trim_start_matches("the ").trim();
        let n = c[i..].split_whitespace().nth(2).and_then(number).filter(|n| (1..=9).contains(n));
        if let (true, Some(n)) = (verbless && !name.is_empty() && name.split_whitespace().count() <= 3, n) {
            return Some(vec![step("move_to_desktop", json!({"window": name, "number": n}))]);
        }
    }
    // "send firefox to desktop 2", "move this window to desktop three".
    if ["send", "move", "put", "throw"].contains(w.first()?) {
        if let Some(i) = w.iter().position(|x| ["desktop", "workspace", "space"].contains(x)) {
            if let Some(n) = w.get(i + 1).and_then(|x| number(x)).filter(|n| (1..=9).contains(n)) {
                let name: Vec<&str> = w[1..i].iter().copied().filter(|x| !["the", "this", "my", "window", "app", "to", "onto", "on"].contains(x)).collect();
                let window = if name.is_empty() { "it".to_string() } else { name.join(" ") };
                return Some(vec![step("move_to_desktop", json!({"window": window, "number": n}))]);
            }
        }
    }
    // The dock.
    if has(w, &["dock"]) {
        let mut args = serde_json::Map::new();
        if has(w, &["left"]) {
            args.insert("position".into(), json!("left"));
        } else if has(w, &["right"]) {
            args.insert("position".into(), json!("right"));
        } else if has(w, &["bottom", "down"]) {
            args.insert("position".into(), json!("bottom"));
        }
        if c.contains("always show") || c.contains("dont hide") || c.contains("stop hiding") || c.contains("never hide") || c.contains("keep the dock") {
            args.insert("hide".into(), json!("never"));
        } else if has(w, &["hide", "hidden", "autohide"]) {
            args.insert("hide".into(), json!("always"));
        }
        if has(w, &["bigger", "larger", "big", "large"]) {
            args.insert("size".into(), json!("large"));
        } else if has(w, &["smaller", "small", "tiny"]) {
            args.insert("size".into(), json!("small"));
        }
        if !args.is_empty() {
            return Some(vec![step("dock", Value::Object(args))]);
        }
    }
    // kompact's switches: "turn off animations", "focus follows mouse on".
    let setting = if has(w, &["animations", "animation"]) {
        Some("animations")
    } else if c.contains("focus follows") {
        Some("focus-follows-mouse")
    } else if c.contains("tap to click") {
        Some("tap-to-click")
    } else if c.contains("left handed") || c.contains("lefthanded") {
        Some("left-handed")
    } else {
        None
    };
    if let (Some(name), Some(on)) = (setting, switch(w)) {
        return Some(vec![step("desktop_setting", json!({"setting": name, "on": on}))]);
    }
    None
}

fn first_is(w: &[&str], word: &str) -> bool {
    w.first() == Some(&word)
}

/// A Settings pane's own name after "open" / "show" ("open about", "open sound", "open date and
/// time"), or a question that only About answers ("what version is this").
fn pane_named(c: &str) -> Option<&'static str> {
    if ["about this computer", "about this pc", "system information", "system info", "show system information", "what version is this", "which version is this", "about kompact", "about the computer"].contains(&c) {
        return Some("about");
    }
    let rest = ["open the ", "show the ", "open my ", "show me the ", "show me ", "open ", "show ", "go to ", "change the ", "change my ", "change "].iter().find_map(|p| c.strip_prefix(p))?;
    // ("notifications" alone shows them: the Notification Center, not its settings.)
    const NAMES: [(&str, &str); 25] = [
        ("about", "about"), ("sound", "sound"), ("audio", "sound"), ("wallpaper", "wallpaper"), ("background", "wallpaper"), ("desktop background", "wallpaper"),
        ("display", "displays"), ("displays", "displays"), ("monitors", "displays"), ("appearance", "appearance"), ("theme", "appearance"), ("users", "users"),
        ("accounts", "users"), ("date and time", "datetime"), ("datetime", "datetime"), ("keyboard shortcuts", "keys"),
        ("shortcuts", "keys"), ("keyboard", "input"), ("mouse", "input"), ("touchpad", "input"), ("lock screen", "lock"), ("default apps", "defaults"),
        ("screen sharing", "sharing"), ("wifi", "network"), ("wi fi", "network"),
    ];
    NAMES.iter().find(|(n, _)| *n == rest).map(|(_, p)| *p)
}

/// Which Settings pane the words name.
fn pane_for(c: &str) -> Option<&'static str> {
    const PANES: [(&[&str], &str); 19] = [
        (&["wifi", "wi fi", "network", "internet", "ethernet"], "network"),
        (&["bluetooth"], "bluetooth"),
        (&["notification"], "notifications"),
        (&["sound", "audio", "volume", "speaker", "microphone"], "sound"),
        (&["about", "system info", "version"], "about"),
        (&["appearance", "theme", "dark mode", "light mode", "accent"], "appearance"),
        (&["control center", "menu bar"], "control"),
        (&["assistant", "siva", "voice"], "assistant"),
        (&["date", "time", "clock", "time zone"], "datetime"),
        (&["default app", "default browser"], "defaults"),
        (&["dock", "desktop"], "dock"),
        (&["display", "monitor", "screen resolution"], "displays"),
        (&["wallpaper", "background"], "wallpaper"),
        (&["screenshot"], "screenshots"),
        (&["sharing", "screen share"], "sharing"),
        (&["lock screen", "lock", "energy", "sleep"], "lock"),
        (&["user", "account", "login", "password"], "users"),
        (&["mouse", "keyboard layout", "touchpad", "trackpad", "keyboard"], "input"),
        (&["shortcut", "hotkey", "keys"], "keys"),
    ];
    // "keyboard shortcuts" is shortcuts, not the keyboard.
    if c.contains("shortcut") {
        return Some("keys");
    }
    PANES.iter().find(|(words, _)| words.iter().any(|x| c.contains(x))).map(|(_, p)| *p)
}

/// Which folder ("open my downloads", "show the documents folder", "open the trash").
fn folder_for(w: &[&str]) -> Option<&'static str> {
    const FOLDERS: [(&str, &str); 10] = [
        ("downloads", "downloads"),
        ("download", "downloads"),
        ("documents", "documents"),
        ("pictures", "pictures"),
        ("photos", "pictures"),
        ("music", "music"),
        ("videos", "videos"),
        ("screenshots", "screenshots"),
        ("trash", "trash"),
        ("bin", "trash"),
    ];
    let rest: Vec<&str> = w[1..].iter().copied().filter(|x| !["the", "my", "me", "folder", "directory", "files", "in", "up"].contains(x)).collect();
    match rest.as_slice() {
        ["home"] => Some("home"),
        ["desktop", "folder"] | ["desktop"] if w.contains(&"folder") => Some("desktop"),
        [one] => FOLDERS.iter().find(|(k, _)| k == one).map(|(_, f)| *f),
        _ => None,
    }
}

/// "search youtube for lofi music", "search for cats on youtube", "youtube search cats",
/// "google how tall is everest", "find pizza near me on maps": in the browser. 20d: "search for
/// rust traits", "look up the eiffel tower" (no site, not google, not "in the browser"): Siva
/// looks it up online and lists what it found (`look_up`).
fn search(c: &str) -> Option<Step> {
    const SITES: [(&str, &str); 8] = [("youtube", "youtube"), ("wikipedia", "wikipedia"), ("google maps", "maps"), ("maps", "maps"), ("images", "images"), ("news", "news"), ("github", "github"), ("google", "web")];
    // Siva's own look-up unless a site or the browser is named: what it wants.
    let mut online = Some("results");
    let (mut q, mut site) = if let Some(r) = c.strip_prefix("search for ").or(c.strip_prefix("search ")).or(c.strip_prefix("find ")) {
        (r.to_string(), "web")
    } else if let Some(r) = c.strip_prefix("look up ") {
        online = Some("answer");
        (r.to_string(), "web")
    } else if let Some(r) = c.strip_prefix("google ") {
        online = None;
        (r.to_string(), "web")
    } else {
        // "youtube search cats", "play lofi on youtube" isn't one (media).
        let (name, site) = SITES.iter().find(|(name, _)| c.starts_with(&format!("{name} search ")))?;
        online = None;
        (c[name.len() + " search ".len()..].to_string(), *site)
    };
    for (name, s) in SITES {
        // "youtube for cats" / "cats on youtube" / "on youtube for cats"
        if let Some(r) = q.strip_prefix(&format!("{name} for ")).or(q.strip_prefix(&format!("on {name} for "))) {
            (q, site, online) = (r.to_string(), s, None);
            break;
        }
        if let Some(r) = q.strip_suffix(&format!(" on {name}")).or(q.strip_suffix(&format!(" in {name}"))) {
            (q, site, online) = (r.to_string(), s, None);
            break;
        }
    }
    for tail in [" in the browser", " in my browser", " in browser", " in firefox", " in a browser"] {
        if let Some(r) = q.strip_suffix(tail) {
            (q, online) = (r.to_string(), None);
            break;
        }
    }
    let q = q.trim().trim_start_matches("for ").trim();
    if q.is_empty() {
        return None;
    }
    if let Some(want) = online {
        return Some(step("look_up", json!({"query": q, "want": want})));
    }
    Some(step("search_web", if site == "web" { json!({"query": q}) } else { json!({"query": q, "site": site}) }))
}

// ---- 20d: online ----

/// Words that end a place or a topic ("in chennai tomorrow", "about cricket today").
const TIME_WORDS: [&str; 16] = ["today", "tomorrow", "tomorrows", "this", "next", "week", "weekend", "now", "right", "like", "please", "outside", "currently", "tonight", "morning", "evening"];

/// The words after the last of `marks` (" in ", " about "…), up to a time word; None if none.
fn words_after(c: &str, marks: &[&str]) -> Option<String> {
    let (i, m) = marks.iter().filter_map(|m| c.rfind(m).map(|i| (i, *m))).max_by_key(|(i, _)| *i)?;
    let words: Vec<&str> = c[i + m.len()..].split_whitespace().take_while(|w| !TIME_WORDS.contains(w)).collect();
    let words: Vec<&str> = words.into_iter().skip_while(|w| ["the", "a"].contains(w)).collect();
    (!words.is_empty()).then(|| words.join(" "))
}

/// "weather", "will it rain tomorrow", "weather in chennai this week", "chennai weather", "is
/// it cold outside", "how hot is it in delhi".
fn weather_clause(c: &str, w: &[&str]) -> Option<Step> {
    const NOT_OUTSIDE: [&str; 9] = ["cpu", "computer", "processor", "laptop", "pc", "gpu", "room", "machine", "system"];
    if ["open", "launch", "start", "run"].contains(w.first()?) || has(w, &NOT_OUTSIDE) {
        return None;
    }
    let asks = has(w, &["will", "is", "going", "does", "whats", "what", "any", "chance", "how", "hows", "should", "do", "it"]);
    let about = has(w, &["weather", "forecast", "climate", "umbrella", "humidity"])
        || (has(w, &["rain", "raining", "rainy", "snow", "snowing", "sunny", "humid", "windy", "cloudy"]) && asks)
        || (has(w, &["temperature", "degrees", "hot", "cold", "warm"]) && asks && (has(w, &["outside", "today", "tomorrow", "week", "in", "now", "weather", "it"])));
    if !about {
        return None;
    }
    let day = if has(w, &["tomorrow", "tomorrows"]) { "tomorrow" } else if has(w, &["week", "weekend", "days"]) { "week" } else { "today" };
    const HERE: [&str; 6] = ["my", "here", "area", "town", "city", "place"];
    let place = words_after(c, &[" in ", " at ", " for "]).filter(|p| !p.split_whitespace().any(|x| HERE.contains(&x)) && !["it", "rain"].contains(&p.as_str())).or_else(|| {
        // "chennai weather", "london forecast tomorrow"
        let at = w.iter().position(|x| ["weather", "forecast"].contains(x))?;
        const FILLER: [&str; 20] = ["whats", "what", "is", "the", "how", "hows", "todays", "tomorrows", "current", "show", "me", "tell", "check", "get", "give", "s", "like", "local", "a", "of"];
        let before: Vec<&str> = w[..at].iter().copied().filter(|x| !FILLER.contains(x) && !TIME_WORDS.contains(x) && !HERE.contains(x)).collect();
        (1..=3).contains(&before.len()).then(|| before.join(" "))
    });
    let mut args = serde_json::Map::new();
    if let Some(p) = place {
        args.insert("place".into(), json!(p));
    }
    if day != "today" {
        args.insert("day".into(), json!(day));
    }
    Some(step("weather", Value::Object(args)))
}

/// 21f: a page read out, on this PC (no AI): "read the headlines on the page", "read the
/// headlines the page" (as heard), "read the headlines in firefox", "read this page aloud",
/// "read the article". Normalized words. Plain "read the headlines" is the news (`news_clause`).
fn page_clause(c: &str) -> Option<Step> {
    const PLACES: [&str; 14] = ["page", "this page", "that page", "the page", "the site", "this site", "the website", "this website", "the screen", "my screen", "the tab", "this tab", "the browser", "it"];
    const BROWSERS: [&str; 6] = ["firefox", "chrome", "chromium", "brave", "browser", "the browser"];
    let mut t = c.trim().strip_prefix("please ").unwrap_or(c.trim());
    for end in [" please", " aloud", " out loud", " for me", " to me"] {
        t = t.strip_suffix(end).unwrap_or(t);
    }
    const HEADLINES: [&str; 16] = [
        "read out the headlines", "read me the headlines", "read the news headlines", "read all the headlines", "read the headlines", "read headlines", "tell me the headlines", "what are the headlines",
        "whats the headlines", "read the top headlines", "read all the stories", "read the stories", "read me the stories", "read the top stories", "read all the news", "read the news",
    ];
    // "read all the stories": the page just opened ("open google news and read all the
    // stories"); alone, the page in front.
    if ["read all the stories", "read the stories", "read me the stories", "read the top stories", "read all stories"].contains(&t) {
        return Some(step("read_window", json!({"what": "headlines"})));
    }
    if let Some(rest) = HEADLINES.iter().find_map(|p| t.strip_prefix(p)) {
        let rest = rest.trim();
        let rest = ["on ", "of ", "from ", "in ", "at "].iter().find_map(|p| rest.strip_prefix(p)).unwrap_or(rest).trim();
        if PLACES.contains(&rest) {
            return Some(step("read_window", json!({"what": "headlines"})));
        }
        if BROWSERS.contains(&rest) {
            return Some(step("read_window", json!({"window": rest.trim_start_matches("the "), "what": "headlines"})));
        }
        return None;
    }
    const PAGE: [&str; 12] = [
        "read the page", "read this page", "read out the page", "read the whole page", "read the article", "read this article", "read the web page", "read this web page", "read whats on the page",
        "read what is on the page", "read the page out", "read out this page",
    ];
    if PAGE.contains(&t) {
        return Some(step("read_window", json!({"what": "page"})));
    }
    // "read the page in firefox"
    let (page, app) = t.split_once(" in ")?;
    (PAGE.contains(&page) && BROWSERS.contains(&app)).then(|| step("read_window", json!({"window": app.trim_start_matches("the "), "what": "page"})))
}

/// "news", "the headlines", "news about cricket", "tech news", "whats happening in tamil nadu",
/// "latest on the election".
fn news_clause(c: &str, w: &[&str]) -> Option<Step> {
    let happening = ["whats happening in ", "what is happening in ", "whats going on in ", "latest on ", "the latest on ", "any news on "].iter().find_map(|p| c.strip_prefix(p));
    if happening.is_none() && !has(w, &["news", "headlines", "headline"]) {
        return None;
    }
    // "open google news" is the site; "search news for x" the browser.
    let bare = ["news", "the news", "headlines", "the headlines", "the latest news", "todays news"];
    let after_verb = c.split_once(' ').map_or("", |(_, r)| r);
    if (["open", "launch", "start", "go"].contains(w.first()?) && !bare.contains(&after_verb)) || c.contains("search") {
        return None;
    }
    let topic = match happening {
        Some(t) => Some(t.trim_start_matches("the ").to_string()),
        None => words_after(c, &[" about ", " on ", " regarding ", " for ", " from ", " in "]).or_else(|| {
            let at = w.iter().position(|x| ["news", "headlines", "headline"].contains(x))?;
            const FILLER: [&str; 33] = [
                "whats", "what", "is", "are", "the", "show", "me", "tell", "read", "get", "give", "any", "some", "latest", "top", "todays", "today", "current", "breaking", "main", "big",
                "recent", "new", "s", "please", "daily", "trending", "i", "want", "hear", "open", "see", "check",
            ];
            let before: Vec<&str> = w[..at].iter().copied().filter(|x| !FILLER.contains(x) && !TIME_WORDS.contains(x)).collect();
            (1..=3).contains(&before.len()).then(|| before.join(" "))
        }),
    };
    Some(step("news", topic.map(|t| json!({"topic": t})).unwrap_or(json!({}))))
}

/// "who is a r rahman", "what is a black hole", "tell me about chennai", "define serendipity",
/// "how tall is mount everest": facts, looked up (Wikipedia, else search results). Not about
/// you, Siva or the desktop ("what is my volume" is answered by earlier rules, "whats up" is
/// talk).
fn look_up_clause(c: &str) -> Option<Step> {
    const FACTS: [&str; 15] = ["who is ", "who was ", "who are ", "who were ", "what is a ", "what is an ", "whats a ", "whats an ", "what are ", "what was ", "tell me about ", "define ", "definition of ", "meaning of ", "what is "];
    const QUESTIONS: [&str; 13] = ["how tall is ", "how old is ", "how far is ", "how big is ", "how many people ", "how long is ", "when was ", "when did ", "when is ", "why is ", "why do ", "how does ", "how to "];
    const NOT: [&str; 18] = ["my", "your", "you", "this", "that", "it", "up", "wrong", "the matter", "going on", "new", "there", "here", "happening", "open", "running", "playing", "on"];
    let (rest, want) = if let Some(r) = c.strip_prefix("whats ").filter(|r| !r.starts_with("a ") && !r.starts_with("an ")) {
        (r, "answer")
    } else if let Some(r) = FACTS.iter().find_map(|p| c.strip_prefix(p)) {
        (r, "answer")
    } else if QUESTIONS.iter().any(|p| c.starts_with(p)) {
        (c, "results")
    } else if let Some(r) = c.strip_prefix("what does ").and_then(|r| r.strip_suffix(" mean")) {
        (r, "answer")
    } else {
        return None;
    };
    let first = rest.split_whitespace().next()?;
    if NOT.contains(&first) || NOT.iter().any(|n| rest == *n || rest.ends_with(&format!(" {n}")) && n.len() > 3) {
        return None;
    }
    let rest = if want == "answer" { rest.trim_start_matches("the ") } else { rest };
    Some(step("look_up", json!({"query": rest.trim(), "want": want})))
}

/// "i live in chennai", "my town is madurai": remembered for the weather.
fn place_clause(c: &str) -> Option<Step> {
    let p = ["i live in ", "my town is ", "my city is ", "my location is ", "set my location to ", "set my town to ", "set my city to ", "remember i live in ", "i stay in "].iter().find_map(|p| c.strip_prefix(p))?;
    let p = p.trim_end_matches(" now").trim();
    (!p.is_empty() && p.split_whitespace().count() <= 4).then(|| step("set_place", json!({"place": p})))
}

/// One request, already split from the others.
// ---- 20e: typing and keys in the app in front ----

/// Apps "type … in …" may name (anything else after " in " is part of the text: "type see you
/// in the morning").
const TYPE_INTO: [&str; 22] = [
    "text editor", "editor", "gedit", "notes", "terminal", "firefox", "browser", "chrome", "chromium", "files", "writer", "libreoffice", "libreoffice writer",
    "code", "vs code", "visual studio code", "kate", "mousepad", "console", "ptyxis", "this window", "the app",
];

/// The shortcuts said in words, and their keys.
const SHORTCUTS: [(&[&str], &str); 24] = [
    (&["new tab", "open a new tab", "open new tab", "a new tab"], "ctrl+t"),
    (&["close tab", "close this tab", "close the tab", "close the current tab"], "ctrl+w"),
    (&["reopen tab", "reopen the tab", "reopen closed tab", "reopen the closed tab", "reopen the last tab", "undo close tab"], "ctrl+shift+t"),
    (&["next tab", "go to the next tab", "the next tab"], "ctrl+tab"),
    (&["previous tab", "go to the previous tab", "the previous tab", "last tab"], "ctrl+shift+tab"),
    (&["new window", "open a new window", "a new window"], "ctrl+n"),
    (&["save", "save it", "save this", "save that", "save the file", "save my work", "save the document", "save the page"], "ctrl+s"),
    (&["save as", "save it as", "save this as"], "ctrl+shift+s"),
    (&["select all", "select everything", "select all the text"], "ctrl+a"),
    (&["copy", "copy it", "copy that", "copy this", "copy the selection"], "ctrl+c"),
    (&["paste", "paste it", "paste that", "paste here", "paste it here"], "ctrl+v"),
    (&["cut", "cut it", "cut that", "cut this"], "ctrl+x"),
    (&["find in page", "find on this page", "find on page", "search this page", "search in this page", "search the page"], "ctrl+f"),
    (&["reload", "refresh", "reload the page", "refresh the page", "reload page", "refresh page", "reload this page", "refresh this page"], "f5"),
    (&["zoom in", "make the text bigger", "bigger text"], "ctrl+plus"),
    (&["zoom out", "make the text smaller", "smaller text"], "ctrl+minus"),
    (&["reset zoom", "actual size", "normal size"], "ctrl+0"),
    (&["previous page", "go back a page", "back a page", "page back"], "alt+left"),
    (&["next page", "go forward a page", "forward a page", "go forward"], "alt+right"),
    (&["print", "print this", "print it", "print the page", "print the document"], "ctrl+p"),
    (&["scroll down", "page down"], "page down"),
    (&["scroll up", "page up"], "page up"),
    (&["go to the top", "scroll to the top", "to the top"], "ctrl+home"),
    (&["go to the bottom", "scroll to the bottom", "to the bottom"], "ctrl+end"),
];

/// `text`'s end, if it's " in <one of TYPE_INTO>" ("… in the text editor"): the text before
/// it and the app.
fn into_app(text: &str) -> (String, Option<String>) {
    let lower = text.to_ascii_lowercase();
    if let Some(i) = lower.rfind(" in ") {
        let app = lower[i + 4..].trim().trim_end_matches('.').trim_start_matches("the ").trim().to_string();
        if TYPE_INTO.contains(&app.as_str()) {
            return (text[..i].trim().to_string(), Some(app));
        }
    }
    (text.trim().to_string(), None)
}

/// 20e: "type Hello, World!", "type “ls -la” in the terminal", "write ‘Dear Sam’ in Text
/// Editor", "type hello and press enter": as written (case and punctuation kept), so before
/// `normalize`. 20e-2: "write" without quotes is for Siva to write ("write a joke", "write
/// something", "draft an email to Sam about Friday"): `write_text`.
fn typing(raw: &str) -> Option<Vec<Step>> {
    let t = raw.trim();
    let at = |s: &str, i: usize, p: &str| s.get(i..i + p.len()).is_some_and(|x| x.eq_ignore_ascii_case(p));
    let mut start = 0;
    for lead in ["hey siva, ", "hey siva ", "siva, ", "siva ", "please ", "can you ", "could you "] {
        if at(t, start, lead) {
            start += lead.len();
        }
    }
    let verb = ["type out ", "type in ", "type ", "write down ", "write ", "compose ", "draft "].into_iter().find(|v| at(t, start, v))?;
    let mut writing = !verb.starts_with("type") && verb != "write down ";
    let mut rest = t[start + verb.len()..].trim();
    // 20e-3: dictation: "write this message: I'll be late", "write the following …".
    for lead in ["this message", "the message", "a message saying", "the following", "this text", "these words", "this", "the text"] {
        if writing && at(rest, 0, lead) {
            let after = rest[lead.len()..].trim_start_matches([':', ',', '.', ' ', '-']);
            if !after.is_empty() && (lead != "this" || rest[lead.len()..].starts_with([':', ','])) {
                (writing, rest) = (false, after);
                break;
            }
        }
    }
    let open_quotes = ['"', '“', '\'', '‘'];
    let (text, tail) = match rest.chars().next() {
        Some(q) if open_quotes.contains(&q) => {
            let inner = &rest[q.len_utf8()..];
            let close = inner.rfind(['"', '”', '\'', '’'])?;
            (inner[..close].to_string(), inner[close..].chars().skip(1).collect::<String>())
        }
        _ => {
            let (text, enter) = enter_after(rest);
            let (text, app) = into_app(&text);
            if writing {
                let about = text.trim_end_matches(['.', '!']).trim();
                if about.is_empty() || about.chars().any(char::is_control) {
                    return None;
                }
                let mut args = json!({"about": about});
                if let Some(a) = app {
                    args["window"] = json!(a);
                }
                let mut steps = vec![step("write_text", args.clone())];
                if enter {
                    let mut keys = json!({"keys": "enter"});
                    if let Some(w) = args.get("window") {
                        keys["window"] = w.clone();
                    }
                    steps.push(step("press_keys", keys));
                }
                return Some(steps);
            }
            // whisper ends what you said with a full stop; a full stop to type goes in quotes.
            let text = text.strip_suffix('.').filter(|t| !t.contains('.')).map(str::to_string).unwrap_or(text);
            return typed(text, app, enter);
        }
    };
    let (tail, enter) = enter_after(&tail);
    let (_, app) = into_app(&format!("x {}", tail.trim().trim_start_matches(',')));
    typed(text, app, enter)
}

/// 20e-3b: a file name as said ("one plus dot txt file" → "oneplus.txt", "notes text file" →
/// "notes.txt"); one already written ("let.txt") stays as it is. Said with "dot", the words
/// before it are one name, joined.
pub fn spoken_file_name(name: &str) -> String {
    const EXT: [(&str, &str); 22] = [
        ("txt", "txt"), ("text", "txt"), ("pdf", "pdf"), ("md", "md"), ("markdown", "md"), ("doc", "doc"), ("docx", "docx"), ("csv", "csv"), ("html", "html"),
        ("htm", "html"), ("py", "py"), ("python", "py"), ("rs", "rs"), ("rust", "rs"), ("json", "json"), ("jpg", "jpg"), ("jpeg", "jpg"), ("png", "png"), ("sh", "sh"),
        ("odt", "odt"), ("log", "log"), ("xml", "xml"),
    ];
    let ext = |w: &str| EXT.iter().find(|(said, _)| said.eq_ignore_ascii_case(w)).map(|(_, e)| *e);
    let mut words: Vec<&str> = name.split_whitespace().collect();
    // "… file", "… document": not part of the name.
    while words.len() > 1 && ["file", "document"].iter().any(|w| words.last().is_some_and(|l| l.eq_ignore_ascii_case(w))) {
        words.pop();
    }
    if let Some(i) = words.iter().rposition(|w| w.eq_ignore_ascii_case("dot")) {
        if i > 0 && i + 2 == words.len() {
            if let Some(e) = ext(words[i + 1]).map(str::to_string).or_else(|| Some(words[i + 1].to_lowercase())) {
                return format!("{}.{e}", words[..i].concat());
            }
        }
    }
    // "notes text file" (the "file" gone): "notes text" → "notes.txt" only when it said "file".
    if words.len() >= 2 && name.split_whitespace().count() > words.len() && !words.last().is_some_and(|w| w.contains('.')) {
        if let Some(e) = ext(words[words.len() - 1]) {
            return format!("{}.{e}", words[..words.len() - 1].join(" "));
        }
    }
    words.join(" ")
}

/// 21a: programs that make "run …" a command, not an app ("run firefox" opens Firefox).
const COMMANDS: &[&str] = &[
    "python", "python3", "pip", "pip3", "ls", "cat", "echo", "pwd", "cd", "mkdir", "touch", "cp", "mv", "rm", "git", "cargo", "rustc", "make", "cmake", "gcc", "g++", "clang",
    "node", "npm", "npx", "bash", "sh", "sudo", "apt", "df", "du", "free", "uname", "whoami", "date", "grep", "find", "head", "tail", "wc", "chmod", "ping", "ps",
    "kill", "java", "javac", "go", "ruby", "perl", "php", "lua", "tar", "zip", "unzip", "sed", "awk", "sort", "which", "systemctl", "journalctl", "nproc", "lscpu", "ip", "docker",
    "flatpak", "snap", "uptime", "hostname", "lsblk", "tree", "diff", "ln",
];

/// 21a: whether `text` reads as a command line: a known program first, a path to run, or
/// shell punctuation.
fn looks_like_command(text: &str) -> bool {
    let first = text.split_whitespace().next().unwrap_or("").to_ascii_lowercase();
    COMMANDS.contains(&first.as_str()) || first.starts_with("./") || first.starts_with("~/") || first.starts_with('/') || ["&&", " | ", " > ", "$("].iter().any(|p| text.contains(p))
}

/// 21a: `text` past the first of `leads` it starts with (any case), or None.
fn after_lead<'a>(text: &'a str, leads: &[&str]) -> Option<&'a str> {
    leads.iter().find_map(|l| text.get(..l.len()).filter(|x| x.eq_ignore_ascii_case(l)).map(|_| text[l.len()..].trim()))
}

/// 21a: one of XDG's folders by its spoken name ("my documents folder" → "Documents"), or the
/// words as a path; None for your home folder.
fn folder_name(words: &str) -> Option<String> {
    let w = words.trim().trim_end_matches(['.', '?']).trim();
    let mut w = w.strip_prefix("my ").or(w.strip_prefix("the ")).unwrap_or(w).trim();
    w = w.strip_suffix(" folder").or(w.strip_suffix(" directory")).unwrap_or(w).trim();
    let lower = w.to_ascii_lowercase();
    if lower.is_empty() || ["home", "home folder", "~"].contains(&lower.as_str()) {
        return None;
    }
    let xdg = ["documents", "downloads", "desktop", "pictures", "music", "videos", "templates", "public"];
    Some(if xdg.contains(&lower.as_str()) { format!("{}{}", lower[..1].to_uppercase(), &lower[1..]) } else { w.to_string() })
}

/// 21a: typed or said work on files and commands, as written (before `normalize`): "run
/// python3 hello.py", "run the command ls -la in Documents"… "create a file hello.py with
/// print('hi')", "read the file notes.txt", "list the files in my Documents folder".
fn work(raw: &str) -> Option<Step> {
    let mut t = raw.trim();
    for lead in ["hey siva, ", "siva, ", "siva ", "please ", "can you ", "could you "] {
        t = after_lead(t, &[lead]).unwrap_or(t);
    }
    let unquote = |x: &str| x.trim().trim_matches(['`', '"', '“', '”', '\'']).trim().to_string();
    // Commands.
    let said = after_lead(t, &["in the terminal run ", "in terminal run ", "run the command ", "run command ", "execute the command ", "execute command ", "type and run "]);
    let plain = said.is_none();
    if let Some(rest) = said.or_else(|| after_lead(t, &["run ", "execute "])) {
        let mut cmd = rest.to_string();
        for end in [" in the terminal", " in terminal", " in siva's terminal"] {
            if cmd.to_ascii_lowercase().ends_with(end) {
                cmd.truncate(cmd.len() - end.len());
            }
        }
        // A sentence's full stop, not a command's ("run ls ." keeps it).
        if cmd.ends_with('.') && !cmd.ends_with(" .") && !cmd.ends_with("..") {
            cmd.pop();
        }
        // Said aloud: "python3 hello dot py".
        let mut cmd = unquote(&cmd.replace(" dot ", "."));
        // 21e: "ls in ~", "ls in my documents folder": where, not part of the command (from
        // Siva's log: it ran `ls in ~`).
        let mut folder = None;
        if let Some(i) = cmd.to_ascii_lowercase().rfind(" in ") {
            let place = cmd[i + 4..].trim();
            let p = place.to_ascii_lowercase();
            let folderish = place.starts_with('~') || place.starts_with('/') || p.ends_with(" folder") || p.ends_with(" directory") || ["home", "my home", "documents", "downloads", "desktop", "pictures", "music", "videos"].contains(&p.trim_start_matches("my ").trim_start_matches("the "));
            if folderish && !place.contains(['|', '&', ';', '>', '"', '\'']) {
                folder = Some(folder_name(place).unwrap_or_else(|| "~".into()));
                cmd.truncate(i);
            }
        }
        // "run ls command": the word "command" after the program isn't the command's.
        let words: Vec<&str> = cmd.split_whitespace().collect();
        if words.len() == 2 && words[1].eq_ignore_ascii_case("command") {
            cmd = words[0].to_string();
        }
        if !cmd.is_empty() && (!plain || looks_like_command(&cmd)) {
            let mut args = json!({"command": cmd});
            if let Some(f) = folder {
                args["folder"] = json!(f);
            }
            return Some(step("run_command", args));
        }
    }
    // Reading a file.
    if let Some(rest) = after_lead(t, &["read the file ", "read file ", "show me the file ", "show the file ", "show file ", "what's in the file ", "what is in the file ", "print the file "]) {
        let name = spoken_file_name(&unquote(rest.trim_end_matches(['.', '?'])));
        return (!name.is_empty()).then(|| step("read_file", json!({"path": name})));
    }
    // 21f: a page read out ("read this page", "read the headlines on the page").
    if let Some(s) = page_clause(&normalize(t)) {
        return Some(s);
    }
    // 21d: what's in a window.
    let lower = t.to_ascii_lowercase();
    let lower = lower.trim_end_matches(['.', '?', '!']);
    for (lead, window) in [
        ("read this window", None),
        ("read the window", None),
        ("read the screen", None),
        ("read my screen", None),
        ("what's on the screen", None),
        ("what is on the screen", None),
        ("what's on my screen", None),
        ("what is on my screen", None),
        ("what's in this window", None),
        ("what is in this window", None),
        ("what does this window say", None),
        ("what does the page say", None),
    ] {
        if lower == lead {
            return Some(step("read_window", window.map(|w: &str| json!({"window": w})).unwrap_or(json!({}))));
        }
    }
    // "what does the terminal say", "read firefox", "read the text editor window".
    if let Some(app) = lower.strip_prefix("what does ").and_then(|r| r.strip_suffix(" say")).or_else(|| lower.strip_prefix("read ").filter(|r| r.ends_with(" window") || !r.contains(' ') || r.starts_with("the "))) {
        let app = app.trim_start_matches("the ").trim_end_matches(" window").trim();
        // Not a window: "read the headlines" is the news, "read the page" a page (21f).
        let not_window = ["headlines", "headline", "news", "page", "article", "weather", "top"].contains(&app.split_whitespace().next().unwrap_or(""));
        if !app.is_empty() && !not_window && !["file", "it", "this", "that", "me", "my", "aloud"].contains(&app) && !app.contains('.') {
            return Some(step("read_window", json!({"window": app})));
        }
    }
    // A folder's files.
    if let Some(rest) = after_lead(t, &["list the files in ", "list files in ", "what files are in ", "what files are there in ", "show the files in ", "list the files", "list files", "what files are there", "what files do i have"]) {
        let rest = rest.trim_end_matches(['.', '?']);
        return Some(step("list_files", folder_name(rest).map(|f| json!({"folder": f})).unwrap_or(json!({}))));
    }
    // A new file with what's in it.
    if let Some(rest) = after_lead(t, &["create a new file ", "create a file ", "create file ", "make a new file ", "make a file ", "write a file "]) {
        let lower = rest.to_ascii_lowercase();
        let (i, sep) = [" with the text ", " containing ", " that says ", " saying ", " with "].iter().find_map(|s| lower.find(s).map(|i| (i, *s)))?;
        let name = spoken_file_name(&unquote(rest[..i].trim_start_matches("called ").trim_start_matches("named ")));
        let content = unquote(&rest[i + sep.len()..]);
        if name.is_empty() || content.is_empty() {
            return None;
        }
        return Some(step("write_file", json!({"path": name, "content": format!("{content}\n")})));
    }
    None
}

/// 20e-3: "save it as notes.txt", "save the text editor with name, let.txt", "save this
/// file named Shopping List": as written (the dots of the name kept), so before `normalize`.
fn save_as(raw: &str) -> Option<Step> {
    let t = raw.trim().trim_end_matches(['.', '!']).trim();
    let lower = t.to_ascii_lowercase();
    let lower = lower.trim_start_matches("please ").trim_start_matches("siva ").trim_start_matches("siva, ");
    let skip = t.len() - lower.len();
    if !lower.starts_with("save ") {
        return None;
    }
    const MARKS: [&str; 10] = [" as ", " with the file name ", " with file name ", " with the name ", " with name ", " under the name ", " named ", " called ", " with the name,", " with name,"];
    let (at, mark) = MARKS.iter().filter_map(|m| lower.find(m).map(|i| (i, *m))).min_by_key(|(i, _)| *i)?;
    let name = t[skip + at + mark.len()..].trim().trim_matches([',', ':', '"', '“', '”', '\'', ' ']).trim();
    if name.is_empty() || name.chars().count() > 100 || name.contains('/') || name.chars().any(char::is_control) {
        return None;
    }
    // What's between "save" and the name may say which window ("save the text editor as …").
    let what = lower[5..at].trim().trim_start_matches("the ").to_string();
    let name = spoken_file_name(name);
    if name.is_empty() {
        return None;
    }
    let mut args = json!({"name": name});
    if TYPE_INTO.contains(&what.as_str()) {
        args["window"] = json!(what);
    }
    Some(step("save_as", args))
}

/// " and press enter" at the end of `text`: without it, and whether it was there.
fn enter_after(text: &str) -> (String, bool) {
    let lower = text.trim_end_matches('.').to_ascii_lowercase();
    for tail in [" and press enter", " then press enter", ", then press enter", ", and press enter", " and hit enter", " and press return", ", press enter"] {
        if lower.ends_with(tail) {
            return (text[..lower.len() - tail.len()].to_string(), true);
        }
    }
    (text.to_string(), false)
}

fn typed(text: String, app: Option<String>, enter: bool) -> Option<Vec<Step>> {
    if text.trim().is_empty() || text.chars().any(char::is_control) {
        return None;
    }
    let with = |mut args: Value| {
        if let Some(a) = &app {
            args["window"] = json!(a);
        }
        args
    };
    let mut steps = vec![step("type_text", with(json!({"text": text})))];
    if enter {
        steps.push(step("press_keys", with(json!({"keys": "enter"}))));
    }
    Some(steps)
}

/// 20e: a shortcut in words ("new tab", "save it in text editor") or keys ("press ctrl+t",
/// "hit enter", "press control shift t").
fn keys_clause(c: &str) -> Option<Step> {
    let (phrase, app) = match c.rfind(" in ") {
        Some(i) if c[i + 4..].split_whitespace().count() <= 3 => (&c[..i], Some(c[i + 4..].trim_start_matches("the ").to_string())),
        _ => (c, None),
    };
    let with = |keys: &str, app: Option<String>| {
        let mut args = json!({"keys": keys});
        if let Some(a) = app {
            args["window"] = json!(a);
        }
        step("press_keys", args)
    };
    for (said, keys) in SHORTCUTS {
        if said.contains(&phrase) {
            return Some(with(keys, app));
        }
        if said.contains(&c) {
            return Some(with(keys, None));
        }
    }
    let rest = ["press ", "hit ", "push "].iter().find_map(|v| c.strip_prefix(v))?;
    let (rest, app) = match rest.rfind(" in ") {
        Some(i) => (&rest[..i], Some(rest[i + 4..].trim_start_matches("the ").to_string())),
        None => (rest, None),
    };
    let words: Vec<&str> = rest.split_whitespace().filter(|w| !["the", "key", "keys", "button", "a"].contains(w)).collect();
    if words.is_empty() || ["play", "pause"].contains(&words[0]) {
        return None;
    }
    let keys = if rest.contains('+') {
        words.concat()
    } else {
        const MODS: [&str; 7] = ["ctrl", "control", "shift", "alt", "super", "windows", "command"];
        let n = words.iter().take_while(|w| MODS.contains(w)).count();
        let key = words[n..].join(" ");
        if key.is_empty() {
            return None;
        }
        words[..n].iter().map(|m| if *m == "control" { "ctrl" } else { m }).chain(std::iter::once(key.as_str())).collect::<Vec<_>>().join("+")
    };
    Some(with(&keys, app))
}

fn clause(c: &str) -> Option<Vec<Step>> {
    let w: Vec<&str> = c.split_whitespace().collect();
    let first = *w.first()?;
    // 20e: shortcuts in the app in front ("close tab" isn't closing a window).
    if let Some(s) = keys_clause(c) {
        return Some(vec![s]);
    }
    // 20b: the rest of the desktop, before the rules below take its words ("open bluetooth
    // settings" isn't Bluetooth on, "move the dock to the left" isn't a window).
    if let Some(s) = desktop_clause(c, &w) {
        return Some(s);
    }
    // 21f: a page read out, before "the headlines" is the news.
    if let Some(s) = page_clause(c) {
        return Some(vec![s]);
    }
    // 20d: online (after the desktop's own words: "open the network settings" isn't news).
    if let Some(s) = place_clause(c).or_else(|| weather_clause(c, &w)).or_else(|| news_clause(c, &w)) {
        return Some(vec![s]);
    }

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
    // 20b: "firefox to the front" (no verb).
    if let Some(name) = c.strip_suffix(" to the front").or(c.strip_suffix(" to front")) {
        if !name.contains(' ') || name.starts_with("the ") {
            let n = name_after(&name.split_whitespace().collect::<Vec<_>>(), 0)?;
            return Some(vec![step("focus_window", json!({"window": n}))]);
        }
    }
    // 20a: "bring terminal to the front", "bring firefox forward".
    for tail in [" to the front", " to front", " forward", " in front"] {
        if let Some(name) = c.strip_prefix("bring ").and_then(|r| r.strip_suffix(tail)) {
            let n = name_after(&name.split_whitespace().collect::<Vec<_>>(), 0)?;
            return Some(vec![step("focus_window", json!({"window": n}))]);
        }
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

    // 20d: facts, after everything about the desktop ("what is my volume", "whats playing").
    if let Some(s) = look_up_clause(c) {
        return Some(vec![s]);
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
        "I'm Siva, the assistant built into your desktop. I run right here on this PC: your voice never leaves it, and I go online only to look up what you ask."
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
    let title = |t: String| -> String {
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
        "show_part" => match text("part").as_str() {
            "menu" => "Open the menu".into(),
            "control-center" => "Open Control Center".into(),
            "notifications" => "Show the notifications".into(),
            "overview" => "Show all desktops".into(),
            "switcher" => "Open the window switcher".into(),
            p => format!("Open {}", title(p.to_string())),
        },
        "open_settings" if text("pane").is_empty() => "Open Settings".into(),
        "open_settings" => format!("Open {} settings", title(text("pane"))),
        "open_folder" => format!("Open {}", title(text("folder"))),
        "open_link" => format!("Open {}", text("url")),
        // 20d
        "weather" => {
            let at = if text("place").is_empty() { String::new() } else { format!(" in {}", title(text("place"))) };
            match text("day").as_str() {
                "tomorrow" => format!("Check tomorrow's weather{at}"),
                "week" => format!("Check this week's weather{at}"),
                _ => format!("Check the weather{at}"),
            }
        }
        "news" if text("topic").is_empty() => "Get the headlines".into(),
        "news" => format!("Get the news on {}", text("topic")),
        "look_up" => format!("Look up “{}”", text("query")),
        "set_place" => format!("Remember your town: {}", title(text("place"))),
        "run_command" => format!("Run “{}”", text("command")),
        "write_file" => format!("Write the file {}", text("path")),
        "read_file" => format!("Read the file {}", text("path")),
        "read_window" if text("what") == "headlines" => "Read the headlines".into(),
        "read_window" if text("what") == "page" => "Read the page".into(),
        "read_window" if text("window").is_empty() => "Read the window in front".into(),
        "read_window" => format!("Read {}", text("window")),
        "list_files" if text("folder").is_empty() => "List the files in your home folder".into(),
        "list_files" => format!("List the files in {}", text("folder")),
        "type_text" | "write_text" | "press_keys" | "save_as" => {
            let what = match s.tool {
                "type_text" => format!("Type “{}”", text("text")),
                "write_text" => format!("Write {}", text("about")),
                "save_as" => format!("Save as “{}”", text("name")),
                _ => format!("Press {}", text("keys")),
            };
            match s.args.get("window").and_then(Value::as_str) {
                Some(w) => format!("{what} in {}", title(w.to_string())),
                None => what,
            }
        }
        "search_web" if text("site").is_empty() => format!("Search the web for {}", text("query")),
        "search_web" => format!("Search {} for {}", title(text("site")), text("query")),
        "move_to_desktop" => format!("Move {} to desktop {}", title(text("window")), int("number")),
        "show_desktop" => "Show the desktop".into(),
        "dock" => "Change the dock".into(),
        "desktop_setting" => switch(&title(text("setting").replace('-', " "))),
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

/// 21f: names whisper mishears often, put right (from Siva's log, 2026-10-27: "open google
/// mute in firefox", "open google use"). Normalized words.
fn heard_right(c: &str) -> String {
    let mut t = format!(" {c} ");
    for wrong in ["google mute", "google use", "google muse", "google knows", "google nudes", "google new", "google mews"] {
        t = t.replace(&format!(" {wrong} "), " google news ");
    }
    t.trim().to_string()
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
    // "date and time" is one thing (the pane), not two requests.
    let c = heard_right(&normalize(&marked).replace("date and time", "datetime"));
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
    // 21a: files and commands, as written.
    if let Some(s) = work(text) {
        return Parsed::Steps(vec![s]);
    }
    // 20e-3: "… and save it (as notes.txt)": saving is its own step, after the rest.
    let lower = text.to_ascii_lowercase();
    for join in [" and then save ", ", and save ", " and save ", ", then save ", " then save ", ", save "] {
        if let Some(i) = lower.find(join) {
            let tail = &text[i + join.find("save").unwrap_or(0)..];
            let saving = save_as(tail).map(|s| vec![s]).or_else(|| match parse(tail) {
                Parsed::Steps(s) if s.iter().all(|s| s.tool == "press_keys") => Some(s),
                _ => None,
            });
            if let (Parsed::Steps(mut first), Some(more)) = (parse(&text[..i]), saving) {
                first.extend(more);
                resolve_it(&mut first);
                return Parsed::Steps(first);
            }
        }
    }
    if let Some(s) = save_as(text) {
        let mut steps = vec![s];
        resolve_it(&mut steps);
        return Parsed::Steps(steps);
    }
    // 20e: what to type, as written; also after something else ("open text editor and type
    // Dear Sam").
    if let Some(steps) = typing(text) {
        return Parsed::Steps(steps);
    }
    for join in [" and and type ", " and and write ", " and then type ", ", then type ", " then type ", " and type ", ", type ", " and then write ", " then write ", " and write ", ", write "] {
        if let Some(i) = lower.find(join) {
            if let (Parsed::Steps(mut first), Some(more)) = (parse(&text[..i]), typing(&text[i + join.find("type").or(join.find("write")).unwrap_or(0)..])) {
                first.extend(more);
                resolve_it(&mut first);
                return Parsed::Steps(first);
            }
        }
    }
    let mut steps = Vec::new();
    for part in split(&c) {
        match clause(&part) {
            Some(s) => steps.extend(s),
            None => return Parsed::Unknown(part),
        }
    }
    resolve_it(&mut steps);
    // 21f: "open google news and read the headlines": the headlines on the page just opened, not
    // the news looked up.
    for i in 1..steps.len() {
        let browsed = steps[..i].iter().any(|s| matches!(s.tool, "open_link" | "search_web") || (s.tool == "open_app" && s.args["app"].as_str().and_then(super::browser::site_for).is_some()));
        if browsed && steps[i].tool == "news" && steps[i].args.get("topic").is_none() {
            steps[i] = step("read_window", json!({"what": "headlines"}));
        }
    }
    // 21e: the same look twice in a row ("open news and read the headlines": news, news) once.
    steps.dedup_by(|b, a| a.tool == b.tool && a.args == b.args && super::tools::find(a.tool).is_some_and(|t| t.tier == super::tools::Tier::Read));
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
    let c = heard_right(&normalize(&glued.replace([',', ';'], " and ")).replace("date and time", "datetime"));
    let parts: Vec<Result<Vec<Step>, String>> = split(&c).into_iter().map(|p| clause(&p).ok_or(p)).collect();
    (!parts.is_empty()).then_some(parts)
}

/// 20a: only a name, no action ("Firefox", "the terminal", "files please"): at most three
/// words once the fillers go.
pub fn just_a_name(text: &str) -> bool {
    const FILLER: [&str; 9] = ["the", "my", "a", "app", "window", "please", "uh", "um", "siva"];
    let c = normalize(text);
    let words: Vec<&str> = c.split_whitespace().filter(|w| !FILLER.contains(w)).collect();
    (1..=3).contains(&words.len())
}

/// 20a: the first of `names` (apps, windows) that `words` say, as whole words (the longest name
/// first, so "text editor" before "editor"). As the name is written.
pub fn named_in(words: &str, names: &[&str]) -> Option<String> {
    let said = format!(" {} ", normalize(words));
    let mut list: Vec<&str> = names.iter().copied().filter(|n| !normalize(n).is_empty()).collect();
    list.sort_by_key(|n| std::cmp::Reverse(n.len()));
    list.into_iter().find(|n| said.contains(&format!(" {} ", normalize(n)))).map(str::to_string)
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
        // 20e-2: typing after "open text editor" (or "bring up firefox") goes there, not to
        // whatever was in front before it.
        if matches!(s.tool, "type_text" | "write_text" | "press_keys" | "save_as") && s.args.get("window").is_none() {
            if let Some(name) = &last {
                s.args["window"] = json!(name);
            }
        }
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
        // 21a: a command now (and `work::command_asks` asks before it runs).
        assert_eq!(one("run sudo rm -rf"), ("run_command".into(), json!({"command": "sudo rm -rf"})));
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
    fn files_and_commands() {
        let steps = |t: &str| match parse(t) {
            Parsed::Steps(s) => s.iter().map(|s| format!("{} {}", s.tool, s.args)).collect::<Vec<_>>(),
            other => panic!("{t:?}: {other:?}"),
        };
        assert_eq!(steps("run python3 hello.py"), [r#"run_command {"command":"python3 hello.py"}"#]);
        assert_eq!(steps("Run python3 hello.py."), [r#"run_command {"command":"python3 hello.py"}"#], "whisper's full stop");
        assert_eq!(steps("run python3 hello dot py"), [r#"run_command {"command":"python3 hello.py"}"#], "said aloud");
        assert_eq!(steps("please run `ls -la` in the terminal"), [r#"run_command {"command":"ls -la"}"#]);
        assert_eq!(steps("run the command cowsay hi"), [r#"run_command {"command":"cowsay hi"}"#], "said it's a command");
        assert_eq!(steps("run ./build.sh"), [r#"run_command {"command":"./build.sh"}"#]);
        assert_eq!(steps("execute git status"), [r#"run_command {"command":"git status"}"#]);
        // 21e: from Siva's log (2026-10-27).
        assert_eq!(steps("run ls command in ~"), [r#"run_command {"command":"ls","folder":"~"}"#]);
        assert_eq!(steps("Run ls in ~"), [r#"run_command {"command":"ls","folder":"~"}"#]);
        assert_eq!(steps("run ls -la in my documents folder"), [r#"run_command {"command":"ls -la","folder":"Documents"}"#]);
        assert_eq!(steps("run grep -r todo in ~/code"), [r#"run_command {"command":"grep -r todo","folder":"~/code"}"#]);
        assert_eq!(steps("run echo stay in touch"), [r#"run_command {"command":"echo stay in touch"}"#], "not a folder");
        assert_eq!(steps("open news and read the headlines"), [r#"news {}"#], "once");
        // Apps stay apps.
        assert_eq!(steps("run firefox"), [r#"open_app {"app":"firefox"}"#]);
        assert_eq!(steps("run files"), [r#"open_app {"app":"files"}"#]);
        // Files.
        assert_eq!(steps("create a file hello.py with print('Hello')"), [r#"write_file {"content":"print('Hello')\n","path":"hello.py"}"#]);
        assert_eq!(steps("make a file called notes dot txt saying buy milk"), [r#"write_file {"content":"buy milk\n","path":"notes.txt"}"#]);
        assert!(matches!(parse("create a file"), Parsed::Unknown(_) | Parsed::Steps(_)), "no name or contents: not a file");
        assert_eq!(steps("read the file notes.txt"), [r#"read_file {"path":"notes.txt"}"#]);
        assert_eq!(steps("show me the file hello dot py"), [r#"read_file {"path":"hello.py"}"#]);
        assert_eq!(steps("list the files in my documents folder"), [r#"list_files {"folder":"Documents"}"#]);
        assert_eq!(steps("list files"), [r#"list_files {}"#]);
        assert_eq!(steps("what files are in ~/code?"), [r#"list_files {"folder":"~/code"}"#]);
        assert_eq!(steps("list the files in my home folder"), [r#"list_files {}"#]);
        // 21d: what's in a window.
        assert_eq!(steps("read this window"), [r#"read_window {}"#]);
        assert_eq!(steps("What's on the screen?"), [r#"read_window {}"#]);
        assert_eq!(steps("what does the terminal say"), [r#"read_window {"window":"terminal"}"#]);
        assert_eq!(steps("read firefox"), [r#"read_window {"window":"firefox"}"#]);
        assert_eq!(steps("read the text editor window"), [r#"read_window {"window":"text editor"}"#]);
        assert_eq!(steps("read the file notes.txt"), [r#"read_file {"path":"notes.txt"}"#], "a file stays a file");
        // 21f: pages read out on this PC (from Siva's log, 2026-10-27).
        assert_eq!(steps("Open google news and read the headlines the page"), [r#"open_app {"app":"google news"}"#, r#"read_window {"what":"headlines"}"#]);
        assert_eq!(steps("open google news and read the headlines"), [r#"open_app {"app":"google news"}"#, r#"read_window {"what":"headlines"}"#]);
        assert_eq!(steps("go to bbc.com and read the headlines"), [r#"open_link {"url":"bbc.com"}"#, r#"read_window {"what":"headlines"}"#]);
        assert_eq!(steps("read the headlines on this page"), [r#"read_window {"what":"headlines"}"#]);
        assert_eq!(steps("Read the headlines in Firefox."), [r#"read_window {"what":"headlines","window":"firefox"}"#]);
        assert_eq!(steps("read this page aloud"), [r#"read_window {"what":"page"}"#]);
        assert_eq!(steps("read the page"), [r#"read_window {"what":"page"}"#]);
        assert_eq!(steps("read the article in firefox"), [r#"read_window {"what":"page","window":"firefox"}"#]);
        assert_eq!(steps("read the headlines"), [r#"news {}"#], "the news, looked up");
        assert_eq!(steps("read the headlines about cricket"), [r#"news {"topic":"cricket"}"#]);
        assert_eq!(steps("Open Google News in Firefox and read the headlines"), [r#"open_app {"app":"google news in firefox"}"#, r#"read_window {"what":"headlines"}"#]);
        assert_eq!(steps("Open Google News and read all the stories"), [r#"open_app {"app":"google news"}"#, r#"read_window {"what":"headlines"}"#]);
        assert_eq!(steps("Open Google mute in Firefox and read all the stories"), [r#"open_app {"app":"google news in firefox"}"#, r#"read_window {"what":"headlines"}"#], "misheard");
        assert_eq!(steps("Open Google use"), [r#"open_app {"app":"google news"}"#]);
        assert_eq!(steps("read the news on this page"), [r#"read_window {"what":"headlines"}"#]);
        assert_eq!(steps("read the news"), [r#"news {}"#], "the news, looked up");
    }

    #[test]
    fn typing_and_keys() {
        let steps = |t: &str| match parse(t) {
            Parsed::Steps(s) => s.iter().map(|s| format!("{} {}", s.tool, s.args)).collect::<Vec<_>>(),
            other => panic!("{t:?}: {other:?}"),
        };
        assert_eq!(steps("type Hello, World!"), [r#"type_text {"text":"Hello, World!"}"#]);
        assert_eq!(steps("Type hello world."), [r#"type_text {"text":"hello world"}"#], "whisper's full stop");
        assert_eq!(steps("type \"Done.\""), [r#"type_text {"text":"Done."}"#], "quoted: as it is");
        assert_eq!(steps("type see you in the morning"), [r#"type_text {"text":"see you in the morning"}"#]);
        assert_eq!(steps("type “ls -la” in the terminal"), [r#"type_text {"text":"ls -la","window":"terminal"}"#]);
        assert_eq!(steps("write ‘Dear Sam’ in Text Editor"), [r#"type_text {"text":"Dear Sam","window":"text editor"}"#], "quoted: as it is");
        // 20e-2: Siva writes it.
        assert_eq!(steps("write a joke in the text editor"), [r#"write_text {"about":"a joke","window":"text editor"}"#]);
        assert_eq!(steps("Write today's joke."), [r#"write_text {"about":"today's joke"}"#]);
        assert_eq!(steps("Open text editor and write something"), [r#"open_app {"app":"text editor"}"#, r#"write_text {"about":"something","window":"text editor"}"#], "into the app just opened");
        assert_eq!(steps("open text editor and and write a poem about rain"), [r#"open_app {"app":"text editor"}"#, r#"write_text {"about":"a poem about rain","window":"text editor"}"#]);
        assert_eq!(steps("draft an email to Sam about Friday"), [r#"write_text {"about":"an email to Sam about Friday"}"#]);
        assert_eq!(steps("open firefox and new tab"), [r#"open_app {"app":"firefox"}"#, r#"press_keys {"keys":"ctrl+t","window":"firefox"}"#]);
        assert_eq!(steps("Siva, type cats and press enter"), [r#"type_text {"text":"cats"}"#, r#"press_keys {"keys":"enter"}"#]);
        assert_eq!(steps("open text editor and type Dear Sam,"), [r#"open_app {"app":"text editor"}"#, r#"type_text {"text":"Dear Sam,","window":"text editor"}"#]);

        assert_eq!(steps("new tab"), [r#"press_keys {"keys":"ctrl+t"}"#]);
        assert_eq!(steps("close this tab"), [r#"press_keys {"keys":"ctrl+w"}"#], "not closing the window");
        assert_eq!(steps("save it in text editor"), [r#"press_keys {"keys":"ctrl+s","window":"text editor"}"#]);
        assert_eq!(steps("refresh the page"), [r#"press_keys {"keys":"f5"}"#]);
        assert_eq!(steps("press ctrl+shift+t"), [r#"press_keys {"keys":"ctrl+shift+t"}"#]);
        assert_eq!(steps("press control shift t"), [r#"press_keys {"keys":"ctrl+shift+t"}"#]);
        assert_eq!(steps("hit the enter key"), [r#"press_keys {"keys":"enter"}"#]);
        assert_eq!(steps("press page down in firefox"), [r#"press_keys {"keys":"page down","window":"firefox"}"#]);
        assert_eq!(steps("zoom in"), [r#"press_keys {"keys":"ctrl+plus"}"#]);
        // 20e-3b: names as said.
        assert_eq!(spoken_file_name("one plus dot txt file"), "oneplus.txt");
        assert_eq!(spoken_file_name("Shopping List dot text"), "ShoppingList.txt");
        assert_eq!(spoken_file_name("notes text file"), "notes.txt");
        assert_eq!(spoken_file_name("report dot pdf"), "report.pdf");
        assert_eq!(spoken_file_name("let.txt"), "let.txt");
        assert_eq!(spoken_file_name("Shopping List"), "Shopping List");
        assert_eq!(spoken_file_name("my text"), "my text", "\"text\" alone isn't an extension");
        assert_eq!(steps("save it as one plus dot txt file"), [r#"save_as {"name":"oneplus.txt"}"#]);
        assert_eq!(
            steps("Open text editor and write one plus node board, then save it as one plus dot txt file"),
            [r#"open_app {"app":"text editor"}"#, r#"write_text {"about":"one plus node board","window":"text editor"}"#, r#"save_as {"name":"oneplus.txt","window":"text editor"}"#]
        );
        // 20e-3: saving with a name, dictation.
        assert_eq!(steps("Save the text editor with name, let.txt."), [r#"save_as {"name":"let.txt","window":"text editor"}"#]);
        assert_eq!(steps("save it as Shopping List"), [r#"save_as {"name":"Shopping List"}"#]);
        assert_eq!(
            steps("Open text editor and write a short note to Sam and save it as sam.txt"),
            [r#"open_app {"app":"text editor"}"#, r#"write_text {"about":"a short note to Sam","window":"text editor"}"#, r#"save_as {"name":"sam.txt","window":"text editor"}"#]
        );
        assert_eq!(steps("write a joke in text editor and save it"), [r#"write_text {"about":"a joke","window":"text editor"}"#, r#"press_keys {"keys":"ctrl+s","window":"text editor"}"#]);
        assert_eq!(steps("Open text editor and write this message. I will be late today"), [r#"open_app {"app":"text editor"}"#, r#"type_text {"text":"I will be late today","window":"text editor"}"#]);
        assert_eq!(steps("write the message, I'll be late today."), [r#"type_text {"text":"I'll be late today"}"#]);
        assert_eq!(steps("write down milk, eggs and bread"), [r#"type_text {"text":"milk, eggs and bread"}"#]);
        assert_eq!(steps("write this: see you soon"), [r#"type_text {"text":"see you soon"}"#]);
        assert_eq!(steps("write this as a poem"), [r#"write_text {"about":"this as a poem"}"#], "not dictation");
        // Still what they were.
        assert_eq!(steps("go back"), [r#"focus_window {"window":"other one"}"#]);
        assert_eq!(steps("open firefox and then type cats"), [r#"open_app {"app":"firefox"}"#, r#"type_text {"text":"cats","window":"firefox"}"#]);
        assert_eq!(steps("next"), [r#"media {"action":"next"}"#]);
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

    /// Where many ways of saying 20b's requests land: `cargo test phrasebook -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn phrasebook() {
        for t in [
            "open about", "about this computer", "show system information", "open about settings", "what version is this",
            "minimize all applications", "minimize all apps", "minimize everything", "minimize all windows", "hide all apps", "close all applications", "close all windows",
            "show all apps", "show all applications", "open launchpad", "show me my apps", "open the app grid",
            "open sound settings", "sound settings", "open the bluetooth setting", "with the bluetooth setting", "bluetooth settings please", "take me to wifi settings", "change the wallpaper", "open wallpaper settings", "open display settings", "open the dock settings", "open keyboard settings", "open the network settings", "open notification settings",
            "open sound", "open wallpaper", "open displays", "open appearance", "open users", "open date and time",
            "give me the compact menu", "show the kompact menu", "open the logo menu", "open menu",
            "open control center", "show quick settings", "open notification center", "show my notifications", "show all desktops", "open mission control", "show the window switcher",
            "open spotlight", "search apps",
            "open downloads", "open my documents folder", "show me my pictures", "open home folder", "open the trash", "empty the trash",
            "search youtube for tamil music", "search for tamil songs on youtube", "play tamil songs on youtube", "youtube tamil songs", "look up rust traits", "google weather in chennai", "find pizza places on maps", "search wikipedia for chennai",
            "go to github.com", "open youtube", "open youtube.com", "open gmail",
            "show the desktop", "show desktop", "clear the screen", "minimize everything",
            "send firefox to desktop 2", "move firefox to desktop two", "move this window to desktop 3", "firefox to desktop 2", "put terminal on desktop 2",
            "move the dock to the left", "put the dock on the right", "dock to the bottom", "hide the dock", "always show the dock", "make the dock bigger", "make the dock smaller",
            "turn off animations", "disable animations", "turn on focus follows mouse", "enable tap to click",
            "keep firefox on the left", "firefox to the front", "bring firefox to the front",
        ] {
            println!("{t:45} → {:?}", match parse(t) {
                Parsed::Steps(s) => s.iter().map(|s| format!("{} {}", s.tool, s.args)).collect::<Vec<_>>().join("; "),
                other => format!("{other:?}"),
            });
        }
    }

    #[test]
    fn online() {
        // 20d: the weather.
        assert_eq!(one("what's the weather"), ("weather".into(), json!({})));
        assert_eq!(one("weather in Chennai tomorrow"), ("weather".into(), json!({"place": "chennai", "day": "tomorrow"})));
        assert_eq!(one("What's the weather like in New York this week?"), ("weather".into(), json!({"place": "new york", "day": "week"})));
        assert_eq!(one("chennai weather"), ("weather".into(), json!({"place": "chennai"})));
        assert_eq!(one("will it rain tomorrow"), ("weather".into(), json!({"day": "tomorrow"})));
        assert_eq!(one("is it raining in London"), ("weather".into(), json!({"place": "london"})));
        assert_eq!(one("is it cold outside"), ("weather".into(), json!({})));
        assert_eq!(one("how hot is it in delhi"), ("weather".into(), json!({"place": "delhi"})));
        assert_eq!(one("weather in my area"), ("weather".into(), json!({})));
        assert_eq!(one("give me the climate report"), ("weather".into(), json!({})));
        assert_eq!(one("weather forecast for the weekend"), ("weather".into(), json!({"day": "week"})));
        assert_ne!(one("why is my computer so hot").0, "weather");
        assert_eq!(one("google weather in chennai").0, "search_web", "google: the browser");
        // The news.
        assert_eq!(one("news"), ("news".into(), json!({})));
        assert_eq!(one("what are today's headlines"), ("news".into(), json!({})));
        assert_eq!(one("read me the latest news"), ("news".into(), json!({})));
        assert_eq!(one("news about cricket"), ("news".into(), json!({"topic": "cricket"})));
        assert_eq!(one("tech news"), ("news".into(), json!({"topic": "tech"})));
        assert_eq!(one("show me the news on the stock market today"), ("news".into(), json!({"topic": "stock market"})));
        assert_eq!(one("what's happening in Tamil Nadu"), ("news".into(), json!({"topic": "tamil nadu"})));
        assert_eq!(one("latest on the election"), ("news".into(), json!({"topic": "election"})));
        assert_eq!(one("open the news"), ("news".into(), json!({})));
        assert_eq!(one("open google news").0, "open_app", "the site");
        assert_eq!(one("search news for chennai").0, "search_web");
        // Facts and searches.
        assert_eq!(one("who is A R Rahman"), ("look_up".into(), json!({"query": "a r rahman", "want": "answer"})));
        assert_eq!(one("what is a black hole"), ("look_up".into(), json!({"query": "black hole", "want": "answer"})));
        assert_eq!(one("tell me about the Eiffel Tower"), ("look_up".into(), json!({"query": "eiffel tower", "want": "answer"})));
        assert_eq!(one("what does serendipity mean"), ("look_up".into(), json!({"query": "serendipity", "want": "answer"})));
        assert_eq!(one("how tall is mount everest"), ("look_up".into(), json!({"query": "how tall is mount everest", "want": "results"})));
        assert_eq!(one("search for rust traits"), ("look_up".into(), json!({"query": "rust traits", "want": "results"})));
        assert_eq!(one("look up the eiffel tower"), ("look_up".into(), json!({"query": "the eiffel tower", "want": "answer"})));
        assert_eq!(one("search rust traits in the browser"), ("search_web".into(), json!({"query": "rust traits"})));
        // Not facts: about the desktop, or talk.
        assert_eq!(one("what's the time").0, "date_time");
        assert_eq!(one("what is my volume").0, "sound_status");
        assert_eq!(one("what's playing").0, "now_playing");
        assert!(matches!(parse("what's up"), Parsed::Unknown(_) | Parsed::Talk(_)));
        assert!(matches!(parse("what is wrong with you"), Parsed::Unknown(_)));
        assert!(matches!(parse("what is this"), Parsed::Unknown(_)));
        assert!(matches!(parse("who are you"), Parsed::Talk(_)));
        // Your town.
        assert_eq!(one("I live in Chennai"), ("set_place".into(), json!({"place": "chennai"})));
        assert_eq!(one("my city is madurai"), ("set_place".into(), json!({"place": "madurai"})));
        assert_eq!(describe(&step("weather", json!({"place": "chennai", "day": "tomorrow"}))), "Check tomorrow's weather in Chennai");
        assert_eq!(describe(&step("news", json!({"topic": "cricket"}))), "Get the news on cricket");
    }

    #[test]
    fn misroutes_from_the_log() {
        // 2026-10-25: "Open about" looked for an app; "Minimize all applications" opened Launchpad;
        // "With the Bluetooth setting" turned Bluetooth on.
        assert_eq!(one("Open about"), ("open_settings".into(), json!({"pane": "about"})));
        assert_eq!(one("about this computer"), ("open_settings".into(), json!({"pane": "about"})));
        assert_eq!(one("what version is this"), ("open_settings".into(), json!({"pane": "about"})));
        assert_eq!(one("Minimize all applications"), ("minimize_window".into(), json!({"window": "all windows"})));
        assert_eq!(one("minimize everything"), ("minimize_window".into(), json!({"window": "all windows"})));
        assert_eq!(one("hide all apps"), ("minimize_window".into(), json!({"window": "all windows"})));
        assert_eq!(one("close all applications"), ("close_window".into(), json!({"window": "all windows"})));
        assert_eq!(one("With the Bluetooth setting"), ("open_settings".into(), json!({"pane": "bluetooth"})));
        assert_eq!(one("open the bluetooth setting"), ("open_settings".into(), json!({"pane": "bluetooth"})));
        assert_eq!(one("change the wallpaper"), ("open_settings".into(), json!({"pane": "wallpaper"})));
        assert_eq!(one("open sound"), ("open_settings".into(), json!({"pane": "sound"})));
        assert_eq!(one("open date and time"), ("open_settings".into(), json!({"pane": "datetime"})));
        assert_eq!(one("open displays"), ("open_settings".into(), json!({"pane": "displays"})));
        assert_eq!(one("Give me the compact menu"), ("show_part".into(), json!({"part": "menu"})));
        assert_eq!(one("show quick settings"), ("show_part".into(), json!({"part": "control-center"})));
        assert_eq!(one("search apps"), ("show_part".into(), json!({"part": "spotlight"})));
        assert_eq!(one("show me my apps"), ("show_part".into(), json!({"part": "launchpad"})));
        assert_eq!(one("show all applications"), ("show_part".into(), json!({"part": "launchpad"})));
        assert_eq!(one("show me my pictures"), ("open_folder".into(), json!({"folder": "pictures"})));
        assert_eq!(one("play tamil songs on youtube"), ("search_web".into(), json!({"query": "tamil songs", "site": "youtube"})));
        assert_eq!(one("youtube tamil songs"), ("search_web".into(), json!({"query": "tamil songs", "site": "youtube"})));
        assert_eq!(one("firefox to desktop 2"), ("move_to_desktop".into(), json!({"window": "firefox", "number": 2})));
        assert_eq!(one("go to desktop 2"), ("switch_desktop".into(), json!({"number": 2})), "still switching");
        assert_eq!(one("keep firefox on the left"), ("place_window".into(), json!({"window": "firefox", "where": "left"})));
        assert_eq!(one("firefox to the front"), ("focus_window".into(), json!({"window": "firefox"})));
        assert_eq!(one("play"), ("media".into(), json!({"action": "play-pause"})), "plain play is still the media");
        assert_eq!(one("open files"), ("open_app".into(), json!({"app": "files"})), "an app is still an app");
    }

    #[test]
    fn the_whole_desktop() {
        assert_eq!(one("open bluetooth settings"), ("open_settings".into(), json!({"pane": "bluetooth"})));
        assert_eq!(one("open the keyboard shortcuts settings"), ("open_settings".into(), json!({"pane": "keys"})));
        assert_eq!(one("open settings"), ("open_settings".into(), json!({})));
        assert_eq!(one("Show me the compact menu"), ("show_part".into(), json!({"part": "menu"})), "from the 2026-10-25 log");
        assert_eq!(one("open control center"), ("show_part".into(), json!({"part": "control-center"})));
        assert_eq!(one("show notifications"), ("show_part".into(), json!({"part": "notifications"})));
        assert_eq!(one("show all desktops"), ("show_part".into(), json!({"part": "overview"})));
        assert_eq!(one("open my downloads"), ("open_folder".into(), json!({"folder": "downloads"})));
        assert_eq!(one("open the trash"), ("open_folder".into(), json!({"folder": "trash"})));
        assert_eq!(one("search youtube for lofi music"), ("search_web".into(), json!({"query": "lofi music", "site": "youtube"})));
        assert_eq!(one("search for cats on youtube"), ("search_web".into(), json!({"query": "cats", "site": "youtube"})));
        assert_eq!(one("search claude in youtube"), ("search_web".into(), json!({"query": "claude", "site": "youtube"})), "from the 2026-10-22 log");
        assert_eq!(one("google how tall is everest"), ("search_web".into(), json!({"query": "how tall is everest"})));
        assert_eq!(one("go to github.com"), ("open_link".into(), json!({"url": "github.com"})));
        assert_eq!(one("show the desktop"), ("show_desktop".into(), json!({})));
        assert_eq!(one("send firefox to desktop 2"), ("move_to_desktop".into(), json!({"window": "firefox", "number": 2})));
        assert_eq!(one("move this window to desktop three"), ("move_to_desktop".into(), json!({"window": "it", "number": 3})));
        assert_eq!(one("move the dock to the left"), ("dock".into(), json!({"position": "left"})));
        assert_eq!(one("hide the dock"), ("dock".into(), json!({"hide": "always"})));
        assert_eq!(one("make the dock bigger"), ("dock".into(), json!({"size": "large"})));
        assert_eq!(one("turn off animations"), ("desktop_setting".into(), json!({"setting": "animations", "on": false})));
        // Still themselves.
        assert_eq!(one("open firefox"), ("open_app".into(), json!({"app": "firefox"})));
        assert_eq!(one("put firefox on the left"), ("place_window".into(), json!({"window": "firefox", "where": "left"})));
        assert_eq!(one("turn off bluetooth"), ("bluetooth".into(), json!({"on": false})));
    }

    #[test]
    fn asking_back() {
        assert!(just_a_name("Firefox"));
        assert!(just_a_name("the terminal please"));
        assert!(!just_a_name("open firefox and put it on the left"));
        let names = ["Firefox", "Text Editor", "Files", "Terminal"];
        assert_eq!(named_in("Firefox", &names).as_deref(), Some("Firefox"));
        assert_eq!(named_in("Terminal to friendly", &names).as_deref(), Some("Terminal"), "a misheard command still names it");
        assert_eq!(named_in("the text editor", &names).as_deref(), Some("Text Editor"));
        assert_eq!(named_in("tell me a joke", &names), None);
        assert_eq!(one("Bring terminal to front"), ("focus_window".into(), json!({"window": "terminal"})));
        assert_eq!(one("bring the terminal to the front"), ("focus_window".into(), json!({"window": "terminal"})));
    }

    #[test]
    fn talking() {
        assert!(matches!(parse("How are you?"), Parsed::Talk(t) if t.starts_with("I'm good")));
        assert!(matches!(parse("Thanks, Siva"), Parsed::Talk(_)));
        assert!(matches!(parse("what's your name"), Parsed::Talk(t) if t.contains("Siva")));
        assert_eq!(one("what time is it"), ("date_time".into(), json!({})));
        assert_eq!(one("what's the date today"), ("date_time".into(), json!({})));
        assert_eq!(one("what day is it"), ("date_time".into(), json!({})));
        // 20d: looked up online now (Wikipedia), no longer the model's to make up.
        assert_eq!(one("tell me about the moon"), ("look_up".into(), json!({"query": "moon", "want": "answer"})));
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
