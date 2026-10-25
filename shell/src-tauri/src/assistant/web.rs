//! 20d: Siva looks things up online when you ask: the weather (Open-Meteo), the news, by topic
//! too (Google News' feeds), facts (Wikipedia) and search results (DuckDuckGo). All of them are
//! free and need no account or key.
//!
//! This is the only part of the assistant that goes online (`no_network` holds the rest to that).
//! It goes only to the hosts in `HOSTS`, only over https, with no redirects, through `curl`. Your
//! voice, your windows and your files never go. What is sent is the place, the topic or the
//! question you asked about. `~/.config/kompact/assistant-internet` says whether it may go
//! online: `internet=always` (the default, chosen 2026-10-25), `ask` (a card each time) or `off`.
//! It also holds your town for the weather (`place=`).
//!
//! The parsers are pure and tested; `fetch` runs curl (call it off the main thread).

use std::path::PathBuf;
use std::process::Command;
use std::sync::Mutex;
use std::time::Instant;

use serde::Serialize;
use serde_json::Value;
use tracing::info;

/// Where Siva may go, and nowhere else.
pub const HOSTS: &[&str] = &["api.open-meteo.com", "geocoding-api.open-meteo.com", "news.google.com", "html.duckduckgo.com", "en.wikipedia.org"];

/// The tools that go online.
pub const ONLINE: &[&str] = &["weather", "news", "look_up"];

pub fn online(tool: &str) -> bool {
    ONLINE.contains(&tool)
}

const AGENT: &str = "Mozilla/5.0 (X11; Linux x86_64) kompact-siva/0.1";
/// At most this much of a reply is read.
const MAX_BYTES: usize = 3_000_000;

// ---- settings ----

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    Off,
    Ask,
    Always,
}

impl Mode {
    pub fn parse(s: &str) -> Option<Mode> {
        match s.trim() {
            "off" => Some(Mode::Off),
            "ask" => Some(Mode::Ask),
            "always" => Some(Mode::Always),
            _ => None,
        }
    }
    pub fn name(self) -> &'static str {
        match self {
            Mode::Off => "off",
            Mode::Ask => "ask",
            Mode::Always => "always",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Prefs {
    pub mode: Mode,
    /// Your town for the weather; empty: the time zone's city.
    pub place: String,
}

pub fn parse_prefs(text: &str) -> Prefs {
    let mut p = Prefs { mode: Mode::Always, place: String::new() };
    for line in text.lines().map(str::trim) {
        if let Some(m) = line.strip_prefix("internet=").and_then(Mode::parse) {
            p.mode = m;
        } else if let Some(v) = line.strip_prefix("place=") {
            p.place = clean_place(v);
        }
    }
    p
}

pub fn prefs_text(p: &Prefs) -> String {
    format!("# Siva online (written by System Settings → Assistant)\ninternet={}\nplace={}\n", p.mode.name(), p.place)
}

/// A town as typed: no control characters, at most 80 characters.
pub fn clean_place(s: &str) -> String {
    s.chars().filter(|c| !c.is_control()).take(80).collect::<String>().trim().to_string()
}

fn prefs_file() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from).or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
    Some(base.join("kompact").join("assistant-internet"))
}

pub fn load_prefs() -> Prefs {
    parse_prefs(&prefs_file().and_then(|f| std::fs::read_to_string(f).ok()).unwrap_or_default())
}

pub fn save_prefs(p: &Prefs) -> Result<(), String> {
    let file = prefs_file().ok_or("no home folder")?;
    if let Some(dir) = file.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    std::fs::write(&file, prefs_text(p)).map_err(|e| format!("can't save {}: {e}", file.display()))
}

/// This PC's time zone ("Asia/Kolkata").
fn time_zone() -> String {
    std::fs::read_link("/etc/localtime")
        .ok()
        .and_then(|p| p.to_string_lossy().split("zoneinfo/").nth(1).map(str::to_string))
        .or_else(|| std::fs::read_to_string("/etc/timezone").ok().map(|s| s.trim().to_string()))
        .unwrap_or_default()
}

/// The time zone's city ("Asia/Kolkata" → "Kolkata", "America/New_York" → "New York").
pub fn zone_city(zone: &str) -> String {
    let city = zone.rsplit('/').next().unwrap_or("");
    if city.is_empty() || !zone.contains('/') || zone.starts_with("Etc/") {
        return String::new();
    }
    match city {
        "Calcutta" => "Kolkata".into(),
        c => c.replace('_', " "),
    }
}

/// The country for the news, from the time zone (Google News' `gl`).
pub fn region(zone: &str) -> &'static str {
    const ZONES: [(&str, &str); 14] = [
        ("Asia/Kolkata", "IN"), ("Asia/Calcutta", "IN"), ("Europe/London", "GB"), ("Europe/Dublin", "IE"), ("Asia/Singapore", "SG"), ("Asia/Dubai", "AE"),
        ("Asia/Colombo", "LK"), ("Asia/Karachi", "PK"), ("Pacific/Auckland", "NZ"), ("Africa/Johannesburg", "ZA"), ("Africa/Lagos", "NG"), ("Africa/Nairobi", "KE"),
        ("America/Toronto", "CA"), ("America/Vancouver", "CA"),
    ];
    if let Some((_, r)) = ZONES.iter().find(|(z, _)| *z == zone) {
        return r;
    }
    if zone.starts_with("Australia/") {
        "AU"
    } else {
        "US"
    }
}

/// Your town for the weather: the one set in Settings, else the time zone's city.
pub fn home_place() -> String {
    let p = load_prefs();
    if !p.place.is_empty() {
        p.place
    } else {
        zone_city(&time_zone())
    }
}

// ---- going online ----

/// Whether Siva may open `url`: https, no user or port, a host in `HOSTS`.
pub fn allowed(url: &str) -> bool {
    let Some(rest) = url.strip_prefix("https://") else { return false };
    let host = rest.split(['/', '?', '#']).next().unwrap_or("");
    HOSTS.contains(&host) && !url.chars().any(|c| c.is_whitespace() || c.is_control())
}

/// GET `url` (one of `HOSTS`): the body, or what went wrong, said for a person. Blocking.
fn fetch(url: &str) -> Result<String, String> {
    if !allowed(url) {
        return Err("I'm not allowed to go there.".into());
    }
    let host = url.trim_start_matches("https://").split('/').next().unwrap_or("");
    let started = Instant::now();
    let out = Command::new("curl")
        .args(["--silent", "--show-error", "--fail", "--compressed", "--proto", "=https", "--max-time", "8", "--max-filesize", "3000000", "--user-agent", AGENT, "--", url])
        .output()
        .map_err(|_| "I need curl to go online: install it with “sudo apt install curl”.".to_string())?;
    if !out.status.success() {
        let code = out.status.code().unwrap_or(-1);
        info!("web: {host} failed (curl {code}) after {} ms", started.elapsed().as_millis());
        return Err(match code {
            6 | 7 => "I can't reach the internet right now.".into(),
            28 => "The internet is too slow right now; try again in a moment.".into(),
            22 => "That service didn't answer properly; try again later.".into(),
            _ => "Something went wrong going online.".into(),
        });
    }
    let mut body = out.stdout;
    body.truncate(MAX_BYTES);
    info!("web: {host} answered in {} ms ({} bytes)", started.elapsed().as_millis(), body.len());
    Ok(String::from_utf8_lossy(&body).into_owned())
}

fn fetch_json(url: &str) -> Result<Value, String> {
    serde_json::from_str(&fetch(url)?).map_err(|_| "That service sent something I can't read.".to_string())
}

/// What a look-up found: what to say, and the links (or lines) the HUD lists.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Found {
    pub text: String,
    pub links: Vec<Link>,
}

/// One line under the reply: a headline, a result, a day of the forecast. With a `url` it's a
/// link (opened in your browser).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Link {
    pub title: String,
    pub detail: String,
    pub url: Option<String>,
}

// ---- text (pure) ----

/// `&amp;`, `&#39;`, `&#x27;` and friends.
pub fn entities(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        rest = &rest[i..];
        let end = rest[..rest.len().min(12)].find(';');
        let decoded = end.and_then(|e| {
            let name = &rest[1..e];
            let c = match name {
                "amp" => Some('&'),
                "lt" => Some('<'),
                "gt" => Some('>'),
                "quot" => Some('"'),
                "apos" => Some('\''),
                "nbsp" => Some(' '),
                n if n.starts_with("#x") || n.starts_with("#X") => u32::from_str_radix(&n[2..], 16).ok().and_then(char::from_u32),
                n if n.starts_with('#') => n[1..].parse().ok().and_then(char::from_u32),
                _ => None,
            };
            c.map(|c| (c, e))
        });
        match decoded {
            Some((c, e)) => {
                out.push(c);
                rest = &rest[e + 1..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// Tags dropped, entities decoded, spaces single.
pub fn plain(html: &str) -> String {
    let mut out = String::new();
    let mut in_tag = false;
    for c in html.chars() {
        match c {
            '<' => in_tag = true,
            '>' if in_tag => in_tag = false,
            c if !in_tag => out.push(c),
            _ => {}
        }
    }
    entities(&out).split_whitespace().collect::<Vec<_>>().join(" ")
}

/// `%2F` → `/`, `+` → space.
pub fn url_decode(s: &str) -> String {
    let hex = |c: u8| (c as char).to_digit(16).map(|d| d as u8);
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'%' if i + 2 < b.len() => match (hex(b[i + 1]), hex(b[i + 2])) {
                (Some(h), Some(l)) => {
                    out.push(h * 16 + l);
                    i += 3;
                    continue;
                }
                _ => out.push(b'%'),
            },
            b'+' => out.push(b' '),
            c => out.push(c),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// At most `n` sentences of `text`, and at most `max` characters.
pub fn sentences(text: &str, n: usize, max: usize) -> String {
    let mut out = String::new();
    let mut count = 0;
    for (i, c) in text.char_indices() {
        out.push(c);
        // A sentence ends at ". " not after a one-letter initial ("J. R. R. Tolkien").
        if matches!(c, '.' | '!' | '?') && text[i + c.len_utf8()..].starts_with(' ') {
            let word = out.rsplit(' ').next().unwrap_or("");
            if word.trim_end_matches(['.', '!', '?']).chars().count() > 1 {
                count += 1;
                if count == n {
                    break;
                }
            }
        }
    }
    let out = out.trim().to_string();
    if out.chars().count() <= max {
        return out;
    }
    let cut: String = out.chars().take(max).collect();
    format!("{}…", cut.rsplit_once(' ').map(|(a, _)| a).unwrap_or(&cut))
}

// ---- the weather (Open-Meteo) ----

#[derive(Debug, Clone, PartialEq)]
pub struct Place {
    pub name: String,
    pub lat: f64,
    pub lon: f64,
}

pub fn place_of(v: &Value) -> Option<Place> {
    let r = v["results"].as_array()?.first()?;
    Some(Place { name: r["name"].as_str()?.to_string(), lat: r["latitude"].as_f64()?, lon: r["longitude"].as_f64()? })
}

/// A WMO weather code in words.
pub fn sky(code: i64) -> &'static str {
    match code {
        0 => "clear sky",
        1 => "mainly clear",
        2 => "partly cloudy",
        3 => "cloudy",
        45 | 48 => "fog",
        51 => "light drizzle",
        53 | 55 => "drizzle",
        56 | 57 => "freezing drizzle",
        61 => "light rain",
        63 => "rain",
        65 => "heavy rain",
        66 | 67 => "freezing rain",
        71 | 73 | 75 | 77 => "snow",
        80 | 81 => "rain showers",
        82 => "heavy rain showers",
        85 | 86 => "snow showers",
        95 => "thunderstorms",
        96 | 99 => "thunderstorms with hail",
        _ => "mixed weather",
    }
}

/// The day of the week of "2026-10-25".
pub fn weekday(date: &str) -> &'static str {
    const NAMES: [&str; 7] = ["Sunday", "Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday"];
    let mut it = date.split('-').map(|p| p.parse::<i64>().unwrap_or(1));
    let (mut y, m, d) = (it.next().unwrap_or(2000), it.next().unwrap_or(1), it.next().unwrap_or(1));
    // Sakamoto's method.
    const T: [i64; 12] = [0, 3, 2, 5, 0, 3, 5, 1, 4, 6, 2, 4];
    if m < 3 {
        y -= 1;
    }
    let w = (y + y / 4 - y / 100 + y / 400 + T[((m - 1).clamp(0, 11)) as usize] + d).rem_euclid(7);
    NAMES[w as usize]
}

fn deg(v: &Value) -> String {
    v.as_f64().map(|t| format!("{}°C", t.round() as i64)).unwrap_or_else(|| "?".into())
}

/// The forecast's day `i`: (sky, low, high, chance of rain).
fn day_of(v: &Value, i: usize) -> Option<(&'static str, String, String, i64)> {
    let d = &v["daily"];
    Some((sky(d["weather_code"].get(i)?.as_i64()?), deg(d["temperature_2m_min"].get(i)?), deg(d["temperature_2m_max"].get(i)?), d["precipitation_probability_max"].get(i).and_then(Value::as_i64).unwrap_or(0)))
}

fn rain_note(chance: i64) -> String {
    if chance >= 20 {
        format!(", {chance}% chance of rain")
    } else {
        String::new()
    }
}

/// The forecast in words: `day` is `today`, `tomorrow` or `week`.
pub fn weather_found(place: &str, v: &Value, day: &str) -> Option<Found> {
    let days = v["daily"]["time"].as_array()?;
    let lines: Vec<Link> = (0..days.len())
        .filter_map(|i| {
            let (s, lo, hi, rain) = day_of(v, i)?;
            let name = match i {
                0 => "Today".to_string(),
                1 => "Tomorrow".to_string(),
                _ => weekday(days[i].as_str()?).to_string(),
            };
            Some(Link { title: name, detail: format!("{s}, {lo} to {hi}{}", rain_note(rain)), url: None })
        })
        .collect();
    let text = match day {
        "tomorrow" => {
            let (s, lo, hi, rain) = day_of(v, 1)?;
            format!("Tomorrow in {place}: {s}, {lo} to {hi}{}.", rain_note(rain))
        }
        "week" => {
            let lows: Vec<f64> = v["daily"]["temperature_2m_min"].as_array()?.iter().filter_map(Value::as_f64).collect();
            let highs: Vec<f64> = v["daily"]["temperature_2m_max"].as_array()?.iter().filter_map(Value::as_f64).collect();
            let lo = lows.iter().cloned().fold(f64::INFINITY, f64::min).round() as i64;
            let hi = highs.iter().cloned().fold(f64::NEG_INFINITY, f64::max).round() as i64;
            let wet: Vec<String> = lines
                .iter()
                .zip(0..)
                .filter(|(_, i)| day_of(v, *i).is_some_and(|d| d.3 >= 50))
                .map(|(l, i)| if i < 2 { l.title.to_lowercase() } else { l.title.clone() })
                .collect();
            let rain = match wet.as_slice() {
                [] => "little chance of rain".to_string(),
                [one] => format!("rain likely {one}"),
                many if many.len() == lines.len() => "rain likely every day".to_string(),
                many => format!("rain likely {} and {}", many[..many.len() - 1].join(", "), many[many.len() - 1]),
            };
            format!("This week in {place}: {lo} to {hi}°C, {rain}.")
        }
        _ => {
            let c = &v["current"];
            let now = sky(c["weather_code"].as_i64()?);
            let t = deg(&c["temperature_2m"]);
            let feels = deg(&c["apparent_temperature"]);
            let feels = if feels != t { format!(", feels like {feels}") } else { String::new() };
            let (_, lo, hi, rain) = day_of(v, 0)?;
            format!("In {place} it's {t} with {now}{feels}. Today: {lo} to {hi}{}.", rain_note(rain))
        }
    };
    Some(Found { text, links: if day == "week" { lines } else { Vec::new() } })
}

/// Places already found (a town's coordinates don't change).
static PLACES: Mutex<Vec<(String, Place)>> = Mutex::new(Vec::new());

fn find_place(name: &str) -> Result<Place, String> {
    let key = name.to_lowercase();
    if let Some((_, p)) = PLACES.lock().unwrap_or_else(|e| e.into_inner()).iter().find(|(k, _)| *k == key) {
        return Ok(p.clone());
    }
    // "Chennai, India" → Open-Meteo wants the town alone.
    let town = name.split(',').next().unwrap_or(name).trim();
    let v = fetch_json(&format!("https://geocoding-api.open-meteo.com/v1/search?name={}&count=1&language=en&format=json", super::browser::encode_query(town)))?;
    let p = place_of(&v).ok_or_else(|| format!("I can't find a place called “{name}”."))?;
    PLACES.lock().unwrap_or_else(|e| e.into_inner()).push((key, p.clone()));
    Ok(p)
}

/// The weather at `place` (your town if None). Blocking.
pub fn weather(place: Option<&str>, day: &str) -> Result<Found, String> {
    let name = match place.map(str::trim).filter(|p| !p.is_empty()) {
        Some(p) => p.to_string(),
        None => home_place(),
    };
    if name.is_empty() {
        return Err("Which town? Say “weather in Chennai”, or set your town in Settings → Assistant.".into());
    }
    let p = find_place(&name)?;
    let v = fetch_json(&format!(
        "https://api.open-meteo.com/v1/forecast?latitude={:.3}&longitude={:.3}&current=temperature_2m,apparent_temperature,weather_code&daily=weather_code,temperature_2m_max,temperature_2m_min,precipitation_probability_max&timezone=auto&forecast_days=7",
        p.lat, p.lon
    ))?;
    weather_found(&p.name, &v, day).ok_or_else(|| "The forecast came back in a shape I can't read.".to_string())
}

// ---- the news (Google News' feeds) ----

/// The items of an RSS feed: the headline, its source (Google News puts it after " - "), the link.
pub fn rss_items(xml: &str, max: usize) -> Vec<Link> {
    let field = |item: &str, tag: &str| -> Option<String> {
        // `<source url="…">The Hindu</source>`: the tag may carry attributes.
        let open = item.find(&format!("<{tag}>")).or_else(|| item.find(&format!("<{tag} ")))?;
        let start = open + item[open..].find('>')? + 1;
        let end = item[start..].find(&format!("</{tag}>"))? + start;
        let raw = item[start..end].trim();
        let raw = raw.strip_prefix("<![CDATA[").and_then(|r| r.strip_suffix("]]>")).unwrap_or(raw);
        Some(entities(raw).trim().to_string())
    };
    xml.split("<item>")
        .skip(1)
        .filter_map(|item| {
            let item = item.split("</item>").next()?;
            let title = field(item, "title")?;
            let link = field(item, "link").filter(|l| l.starts_with("https://"));
            let source = field(item, "source");
            let (title, from) = match title.rsplit_once(" - ") {
                Some((t, s)) if source.as_deref().map_or(s.len() < 60, |src| src == s) => (t.to_string(), s.to_string()),
                _ => (title.clone(), source.unwrap_or_default()),
            };
            (!title.is_empty()).then_some(Link { title, detail: from, url: link })
        })
        .take(max)
        .collect()
}

pub fn news_found(topic: Option<&str>, items: Vec<Link>) -> Found {
    if items.is_empty() {
        return Found { text: match topic { Some(t) => format!("I found no news about {t} today."), None => "I couldn't get the headlines.".into() }, links: items };
    }
    // Said aloud: the headline without its section (" | World News", " | LIVE").
    let top: Vec<String> = items.iter().take(3).map(|l| l.title.split(" | ").next().unwrap_or(&l.title).trim_end_matches(['.', ' ']).to_string()).collect();
    let text = match topic {
        Some(t) => format!("The latest on {t}: {}.", top.join(". ")),
        None => format!("Top headlines: {}.", top.join(". ")),
    };
    Found { text, links: items }
}

/// The news, on `topic` if given. Blocking.
pub fn news(topic: Option<&str>) -> Result<Found, String> {
    let gl = region(&time_zone());
    let tail = format!("hl=en-{gl}&gl={gl}&ceid={gl}:en");
    let topic = topic.map(str::trim).filter(|t| !t.is_empty());
    let items = match topic {
        None => rss_items(&fetch(&format!("https://news.google.com/rss?{tail}"))?, 8),
        Some(t) => {
            let q = super::browser::encode_query(t);
            let today = rss_items(&fetch(&format!("https://news.google.com/rss/search?q={q}+when:1d&{tail}"))?, 8);
            if today.is_empty() {
                rss_items(&fetch(&format!("https://news.google.com/rss/search?q={q}+when:7d&{tail}"))?, 8)
            } else {
                today
            }
        }
    };
    Ok(news_found(topic, items))
}

// ---- looking things up (Wikipedia, DuckDuckGo) ----

/// The page Wikipedia's search put first: (title, its first sentences, its address). None for
/// a disambiguation page ("may refer to").
pub fn wiki_answer(v: &Value) -> Option<(String, String, String)> {
    let pages = v["query"]["pages"].as_object()?;
    let page = pages.values().min_by_key(|p| p["index"].as_i64().unwrap_or(i64::MAX))?;
    let title = page["title"].as_str()?.to_string();
    let extract = page["extract"].as_str()?.replace('\n', " ");
    if extract.trim().is_empty() || extract.contains("may refer to") || extract.contains("may also refer to") {
        return None;
    }
    let url = page["fullurl"].as_str().map(str::to_string).unwrap_or_else(|| format!("https://en.wikipedia.org/wiki/{}", title.replace(' ', "_")));
    Some((title, sentences(&without_asides(&extract), 2, 360), url))
}

/// Pronunciations and other languages' names out: "( EYE-fəl; French: Tour Eiffel )".
fn without_asides(text: &str) -> String {
    let mut out = String::new();
    let mut rest = text;
    while let Some(open) = rest.find(" (") {
        let Some(close) = rest[open..].find(')') else { break };
        let inside = &rest[open + 2..open + close];
        out.push_str(&rest[..open]);
        if !(inside.contains(';') || inside.starts_with(' ') || inside.contains("pronounced") || inside.contains('/')) {
            out.push_str(&rest[open..open + close + 1]);
        }
        rest = &rest[open + close + 1..];
    }
    out.push_str(rest);
    out.split_whitespace().collect::<Vec<_>>().join(" ").replace(" ,", ",")
}

/// DuckDuckGo's results page: title, snippet, the site's own address (not DuckDuckGo's
/// redirect). Ads are left out.
pub fn ddg_results(html: &str, max: usize) -> Vec<Link> {
    const A: &str = "class=\"result__a\" href=\"";
    let mut out = Vec::new();
    let parts: Vec<&str> = html.split(A).skip(1).collect();
    for part in parts {
        let Some(href_end) = part.find('"') else { continue };
        let href = entities(&part[..href_end]);
        let url = match href.split_once("uddg=") {
            Some((_, enc)) => url_decode(enc.split('&').next().unwrap_or("")),
            None => href.clone(),
        };
        if !(url.starts_with("https://") || url.starts_with("http://")) || url.contains("duckduckgo.com/y.js") {
            continue;
        }
        let rest = &part[href_end..];
        let Some(gt) = rest.find('>') else { continue };
        let Some(end) = rest[gt..].find("</a>") else { continue };
        let title = plain(&rest[gt + 1..gt + end]);
        let snippet = rest
            .find("class=\"result__snippet\"")
            .and_then(|s| {
                let after = &rest[s..];
                let gt = after.find('>')?;
                let end = after[gt..].find("</a>")?;
                Some(plain(&after[gt + 1..gt + end]))
            })
            .unwrap_or_default();
        if title.is_empty() {
            continue;
        }
        out.push(Link { title, detail: sentences(&snippet, 2, 200), url: Some(url) });
        if out.len() == max {
            break;
        }
    }
    out
}

/// Search results in words: the first one said, all of them listed, and more in the browser.
pub fn results_found(query: &str, mut items: Vec<Link>) -> Found {
    let more = Link { title: "More results in the browser".into(), detail: String::new(), url: Some(super::browser::search_url("web", query)) };
    let text = match items.first() {
        None => format!("I found nothing for “{query}”."),
        Some(first) if first.detail.is_empty() => format!("Here's what I found for “{query}”: {}.", first.title),
        Some(first) => format!("Here's what I found for “{query}”: {}", first.detail),
    };
    items.push(more);
    Found { text, links: items }
}

/// Look `query` up: `answer` a fact (Wikipedia first, else the search results) or list the
/// `results`. Blocking.
pub fn look_up(query: &str, want: &str) -> Result<Found, String> {
    let q = query.trim();
    if want != "results" {
        let v = fetch_json(&format!(
            "https://en.wikipedia.org/w/api.php?action=query&generator=search&gsrsearch={}&gsrlimit=1&prop=extracts|info&inprop=url&exintro=1&explaintext=1&exsentences=3&format=json&redirects=1",
            super::browser::encode_query(q)
        ))?;
        if let Some((title, text, url)) = wiki_answer(&v) {
            return Ok(Found { text, links: vec![Link { title: format!("{title} on Wikipedia"), detail: String::new(), url: Some(url) }, Link { title: "Search the web".into(), detail: String::new(), url: Some(super::browser::search_url("web", q)) }] });
        }
    }
    let html = fetch(&format!("https://html.duckduckgo.com/html/?q={}", super::browser::encode_query(q)))?;
    Ok(results_found(q, ddg_results(&html, 6)))
}

// ---- Settings → Assistant ----

#[derive(Debug, Serialize)]
pub struct InternetState {
    pub mode: Mode,
    pub place: String,
    /// The town used when none is set (the time zone's city).
    pub zone_place: String,
    /// curl is there to go online with.
    pub curl: bool,
}

#[tauri::command]
pub fn internet_state() -> InternetState {
    let p = load_prefs();
    let curl = std::env::var_os("PATH").is_some_and(|path| std::env::split_paths(&path).any(|d| d.join("curl").is_file()));
    InternetState { mode: p.mode, place: p.place, zone_place: zone_city(&time_zone()), curl }
}

#[tauri::command]
pub fn set_internet(mode: String, place: String) -> Result<InternetState, String> {
    let mode = Mode::parse(&mode).ok_or_else(|| format!("{mode:?} isn't always, ask or off"))?;
    save_prefs(&Prefs { mode, place: clean_place(&place) })?;
    info!("web: internet {}", mode.name());
    Ok(internet_state())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// The real services (online): `cargo test live -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn live() {
        for f in [weather(Some("Chennai"), "today"), weather(Some("Chennai"), "week"), news(None), news(Some("cricket")), look_up("Eiffel Tower", "answer"), look_up("rust traits", "results"), look_up("how tall is mount everest", "results")] {
            let f = f.unwrap();
            println!("{}\n  {}\n", f.text, f.links.iter().take(3).map(|l| format!("{} | {} | {}", l.title, l.detail, l.url.as_deref().unwrap_or("-"))).collect::<Vec<_>>().join("\n  "));
        }
    }

    #[test]
    fn only_where_it_may_go() {
        assert!(allowed("https://api.open-meteo.com/v1/forecast?latitude=1"));
        assert!(allowed("https://news.google.com/rss?hl=en-IN"));
        assert!(!allowed("http://news.google.com/rss"), "https only");
        assert!(!allowed("https://news.google.com.evil.example/rss"));
        assert!(!allowed("https://user@news.google.com/rss"));
        assert!(!allowed("https://news.google.com:8080/rss"));
        assert!(!allowed("https://elsewhere.example/"));
        assert!(!allowed("https://en.wikipedia.org/w x"), "no spaces");
        assert!(!allowed("file:///etc/passwd"));
        // Every address in this file is to one of the hosts.
        let me = include_str!("web.rs").split(&["#[cfg", "(test)]"].concat()).next().unwrap();
        let scheme = ["https", "://"].concat();
        let mut seen = 0;
        for (i, _) in me.match_indices(&scheme) {
            let host: String = me[i + scheme.len()..].chars().take_while(|c| c.is_ascii_alphanumeric() || *c == '.' || *c == '-').collect();
            if !host.is_empty() {
                assert!(HOSTS.contains(&host.as_str()), "{host} isn't in HOSTS");
                seen += 1;
            }
        }
        assert!(seen >= 5, "the scan found the addresses");
    }

    #[test]
    fn settings() {
        assert_eq!(parse_prefs(""), Prefs { mode: Mode::Always, place: String::new() }, "always, by default");
        let p = parse_prefs("# x\ninternet=ask\nplace=  Chennai \n");
        assert_eq!(p, Prefs { mode: Mode::Ask, place: "Chennai".into() });
        assert_eq!(parse_prefs(&prefs_text(&p)), p);
        assert_eq!(parse_prefs("internet=bogus\n").mode, Mode::Always);
        assert_eq!(parse_prefs("internet=off").mode, Mode::Off);
        assert_eq!(zone_city("Asia/Kolkata"), "Kolkata");
        assert_eq!(zone_city("Asia/Calcutta"), "Kolkata");
        assert_eq!(zone_city("America/New_York"), "New York");
        assert_eq!(zone_city("UTC"), "");
        assert_eq!(zone_city("Etc/UTC"), "");
        assert_eq!(region("Asia/Kolkata"), "IN");
        assert_eq!(region("Australia/Sydney"), "AU");
        assert_eq!(region("Europe/Berlin"), "US");
        assert_eq!(clean_place("Chen\nnai"), "Chennai");
    }

    #[test]
    fn text() {
        assert_eq!(entities("Tom &amp; Jerry&#39;s &#x27;show&#x27; &lt;b&gt; &bogus; &"), "Tom & Jerry's 'show' <b> &bogus; &");
        assert_eq!(plain("Defining <b>Traits</b>  &amp;\n more"), "Defining Traits & more");
        assert_eq!(url_decode("https%3A%2F%2Fdoc.rust%2Dlang.org%2Fbook"), "https://doc.rust-lang.org/book");
        assert_eq!(url_decode("a+b%2"), "a b%2");
        assert_eq!(sentences("One. Two. Three.", 2, 100), "One. Two.");
        assert_eq!(sentences("J. R. R. Tolkien was a writer. He wrote books.", 1, 100), "J. R. R. Tolkien was a writer.");
        assert_eq!(sentences("a b c d e f", 1, 7), "a b c…");
        assert_eq!(weekday("2026-10-25"), "Sunday");
        assert_eq!(weekday("2024-02-29"), "Thursday");
        assert_eq!(weekday("2000-01-01"), "Saturday");
    }

    fn forecast() -> Value {
        json!({
            "current": {"temperature_2m": 28.5, "apparent_temperature": 33.2, "weather_code": 51},
            "daily": {"time": ["2026-10-25", "2026-10-26", "2026-10-27"], "weather_code": [95, 53, 3], "temperature_2m_max": [28.7, 30.1, 32.0], "temperature_2m_min": [24.7, 24.5, 23.8], "precipitation_probability_max": [90, 71, 10]}
        })
    }

    #[test]
    fn the_weather() {
        let geo = json!({"results": [{"name": "Chennai", "latitude": 13.08784, "longitude": 80.27847, "country": "India"}]});
        assert_eq!(place_of(&geo), Some(Place { name: "Chennai".into(), lat: 13.08784, lon: 80.27847 }));
        assert_eq!(place_of(&json!({"generationtime_ms": 0.1})), None, "no such place");
        let v = forecast();
        assert_eq!(weather_found("Chennai", &v, "today").unwrap().text, "In Chennai it's 29°C with light drizzle, feels like 33°C. Today: 25°C to 29°C, 90% chance of rain.");
        assert_eq!(weather_found("Chennai", &v, "tomorrow").unwrap().text, "Tomorrow in Chennai: drizzle, 25°C to 30°C, 71% chance of rain.");
        let week = weather_found("Chennai", &v, "week").unwrap();
        assert_eq!(week.text, "This week in Chennai: 24 to 32°C, rain likely today and tomorrow.");
        assert_eq!(week.links.len(), 3);
        assert_eq!(week.links[2], Link { title: "Tuesday".into(), detail: "cloudy, 24°C to 32°C".into(), url: None });
        assert!(weather_found("Chennai", &json!({}), "today").is_none());
    }

    #[test]
    fn the_news() {
        let xml = r#"<rss><channel><title>Top stories</title>
<item><title>Andhra braces for cyclonic storm; Odisha &amp; others alert | LIVE - The Hindu</title><link>https://news.google.com/rss/articles/CBMi1?oc=5</link><pubDate>Sun, 25 Oct 2026 07:16:31 GMT</pubDate><source url="https://www.thehindu.com">The Hindu</source></item>
<item><title><![CDATA[India win the series - ESPNcricinfo]]></title><link>https://news.google.com/rss/articles/CBMi2</link></item>
<item><title>No source here</title><link>javascript:alert(1)</link></item>
</channel></rss>"#;
        let items = rss_items(xml, 8);
        assert_eq!(items.len(), 3);
        assert_eq!(items[0], Link { title: "Andhra braces for cyclonic storm; Odisha & others alert | LIVE".into(), detail: "The Hindu".into(), url: Some("https://news.google.com/rss/articles/CBMi1?oc=5".into()) });
        assert_eq!(items[1].title, "India win the series");
        assert_eq!(items[1].detail, "ESPNcricinfo");
        assert_eq!(items[2].url, None, "only https links");
        assert_eq!(rss_items(xml, 1).len(), 1);
        let f = news_found(Some("cricket"), items[1..2].to_vec());
        assert_eq!(f.text, "The latest on cricket: India win the series.");
        assert_eq!(news_found(None, items[..1].to_vec()).text, "Top headlines: Andhra braces for cyclonic storm; Odisha & others alert.", "without its section");
        assert_eq!(news_found(None, Vec::new()).text, "I couldn't get the headlines.");
        assert_eq!(news_found(Some("x"), Vec::new()).text, "I found no news about x today.");
    }

    #[test]
    fn looking_up() {
        let v = json!({"query": {"pages": {
            "9232": {"index": 1, "title": "Eiffel Tower", "extract": "The Eiffel Tower (  EYE-fəl; French: Tour Eiffel [tuʁ ɛfɛl] ) is a lattice tower on the Champ de Mars in Paris, France. It is named after the engineer Gustave Eiffel (1832–1923). Third sentence.", "fullurl": "https://en.wikipedia.org/wiki/Eiffel_Tower"},
            "1": {"index": 2, "title": "Other", "extract": "x"}
        }}});
        let (title, text, url) = wiki_answer(&v).unwrap();
        assert_eq!(title, "Eiffel Tower");
        assert_eq!(text, "The Eiffel Tower is a lattice tower on the Champ de Mars in Paris, France. It is named after the engineer Gustave Eiffel (1832–1923).");
        assert_eq!(url, "https://en.wikipedia.org/wiki/Eiffel_Tower");
        assert!(wiki_answer(&json!({"query": {"pages": {"1": {"index": 1, "title": "Mercury", "extract": "Mercury may refer to:"}}}})).is_none());
        assert!(wiki_answer(&json!({"batchcomplete": ""})).is_none(), "nothing found");

        let html = r#"<div class="result results_links results_links_deep web-result result--ad"><a rel="nofollow" class="result__a" href="https://duckduckgo.com/y.js?ad_domain=x">Ad</a></div>
<a rel="nofollow" class="result__a" href="//duckduckgo.com/l/?uddg=https%3A%2F%2Fdoc.rust%2Dlang.org%2Fbook%2Fch10%2D02%2Dtraits.html&amp;rut=b2">Defining Shared Behavior with Traits - The Rust Programming Language</a>
<a class="result__snippet" href="//duckduckgo.com/l/?uddg=x">Defining Shared Behavior with <b>Traits</b> A <b>trait</b> defines the functionality a particular type has. We can use it.</a>
<a rel="nofollow" class="result__a" href="//duckduckgo.com/l/?uddg=https%3A%2F%2Fdoc.rust%2Dlang.org%2Freference%2Fitems%2Ftraits.html&amp;rut=fb">Traits - The Rust Reference</a>"#;
        let r = ddg_results(html, 6);
        assert_eq!(r.len(), 2, "the ad is left out: {r:?}");
        assert_eq!(r[0].url.as_deref(), Some("https://doc.rust-lang.org/book/ch10-02-traits.html"));
        assert_eq!(r[0].title, "Defining Shared Behavior with Traits - The Rust Programming Language");
        assert_eq!(r[0].detail, "Defining Shared Behavior with Traits A trait defines the functionality a particular type has. We can use it.");
        assert_eq!(r[1].detail, "");
        let f = results_found("rust traits", r);
        assert!(f.text.starts_with("Here's what I found for “rust traits”: Defining Shared Behavior"), "{}", f.text);
        assert_eq!(f.links.last().unwrap().url.as_deref(), Some("https://www.google.com/search?q=rust+traits"));
        assert_eq!(results_found("zzz", Vec::new()).text, "I found nothing for “zzz”.");
    }
}
