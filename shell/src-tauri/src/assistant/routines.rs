//! 19e: routines: one name for several requests ("coding" → open a terminal, open Firefox, put
//! them side by side, Do Not Disturb on). "Set up for coding", "start coding", "coding mode" and
//! the like run one; every step still goes through `policy` like any other (a Tier 2 step still
//! asks), at most `policy::MAX_STEPS` steps, and one Undo takes back the whole routine.
//!
//! `~/.config/kompact/routines`, one per line, `name = phrase; phrase; …` (Settings → Assistant
//! → Routines writes it). Without the file: `defaults()`. Pure and tested but for the file.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use super::rules::{self, Parsed, Step};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Routine {
    pub name: String,
    /// Each a request as you'd type it ("open firefox", "volume 20").
    pub phrases: Vec<String>,
}

fn routine(name: &str, phrases: &[&str]) -> Routine {
    Routine { name: name.into(), phrases: phrases.iter().map(|p| p.to_string()).collect() }
}

/// The routines there are until you change them.
pub fn defaults() -> Vec<Routine> {
    vec![
        routine("coding", &["open terminal", "open firefox", "put terminal and firefox side by side", "do not disturb on"]),
        routine("focus", &["do not disturb on", "volume 20"]),
        routine("movie", &["do not disturb on", "dark mode", "volume 70", "keep the screen awake"]),
    ]
}

/// Names are kept as you say them: lower case, single spaces, no punctuation.
pub fn tidy_name(name: &str) -> String {
    rules::normalize(name)
}

pub fn parse_file(text: &str) -> Vec<Routine> {
    text.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .filter_map(|l| {
            let (name, steps) = l.split_once('=')?;
            let name = tidy_name(name);
            let phrases: Vec<String> = steps.split(';').map(|p| p.trim().to_string()).filter(|p| !p.is_empty()).collect();
            (!name.is_empty() && !phrases.is_empty()).then_some(Routine { name, phrases })
        })
        .collect()
}

pub fn file_text(list: &[Routine]) -> String {
    let mut s = String::from("# kompact's assistant routines (System Settings → Assistant → Routines): name = request; request; …\n");
    for r in list {
        let phrases: Vec<String> = r.phrases.iter().map(|p| p.replace([';', '\n'], " ")).collect();
        s.push_str(&format!("{} = {}\n", r.name, phrases.join("; ")));
    }
    s
}

/// The routine `text` asks for, if it asks for one: its name alone, or "set up for", "set me up
/// for", "start", "run", "begin", "time for", "let's", "ready for", "routine" before it, or
/// "mode", "time", "routine" after it.
pub fn invoked<'a>(text: &str, list: &'a [Routine]) -> Option<&'a Routine> {
    let c = rules::normalize(text);
    const BEFORE: [&str; 10] = ["set up for ", "set me up for ", "get ready for ", "ready for ", "start ", "run ", "begin ", "time for ", "lets ", "routine "];
    const AFTER: [&str; 3] = [" mode", " time", " routine"];
    let mut name = c.as_str();
    for b in BEFORE {
        if let Some(rest) = name.strip_prefix(b) {
            name = rest;
            break;
        }
    }
    for a in AFTER {
        if let Some(rest) = name.strip_suffix(a) {
            name = rest;
            break;
        }
    }
    let name = name.strip_prefix("the ").unwrap_or(name);
    list.iter().find(|r| r.name == name)
}

/// The routine's steps, as one request ("open files; put it on the left": "it" works across
/// phrases), or the first phrase that isn't understood.
pub fn expand(r: &Routine) -> Result<Vec<Step>, String> {
    for p in &r.phrases {
        if !matches!(rules::parse(p), Parsed::Steps(_)) {
            return Err(p.clone());
        }
    }
    match rules::parse(&r.phrases.join("; ")) {
        Parsed::Steps(steps) => Ok(steps),
        Parsed::Unknown(part) => Err(part),
        _ => Err(r.phrases.join("; ")),
    }
}

/// What's wrong with a routine, for Settings (none: fine).
pub fn problem(r: &Routine, all: &[Routine]) -> Option<String> {
    let name = tidy_name(&r.name);
    if name.is_empty() {
        return Some("Give it a name.".into());
    }
    if all.iter().filter(|o| tidy_name(&o.name) == name).count() > 1 {
        return Some(format!("There are two routines called “{name}”."));
    }
    // "mute" as a routine would take the command's place.
    if !matches!(rules::parse(&name), Parsed::Unknown(_)) {
        return Some(format!("“{name}” is already something Siva does; pick another name."));
    }
    if r.phrases.iter().all(|p| p.trim().is_empty()) {
        return Some("Add at least one request.".into());
    }
    let steps = match expand(r) {
        Ok(s) => s,
        Err(p) => return Some(format!("Siva doesn't understand “{p}”.")),
    };
    if steps.len() > super::policy::MAX_STEPS {
        return Some(format!("That's {} steps; a routine can have at most {}.", steps.len(), super::policy::MAX_STEPS));
    }
    None
}

// ---- the file ----

fn file() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
    Some(base.join("kompact").join("routines"))
}

pub fn load() -> Vec<Routine> {
    match file().map(std::fs::read_to_string) {
        Some(Ok(text)) => parse_file(&text),
        _ => defaults(),
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Problem {
    pub index: usize,
    pub text: String,
}

#[tauri::command]
pub fn routines_state() -> Vec<Routine> {
    load()
}

/// Saved only if every routine is fine; otherwise what's wrong with which.
#[tauri::command]
pub fn routines_save(routines: Vec<Routine>) -> Result<Vec<Routine>, Vec<Problem>> {
    let list: Vec<Routine> = routines
        .into_iter()
        .map(|r| Routine { name: tidy_name(&r.name), phrases: r.phrases.into_iter().map(|p| p.trim().to_string()).filter(|p| !p.is_empty()).collect() })
        .collect();
    let problems: Vec<Problem> = list.iter().enumerate().filter_map(|(index, r)| problem(r, &list).map(|text| Problem { index, text })).collect();
    if !problems.is_empty() {
        return Err(problems);
    }
    let path = file().ok_or_else(|| vec![Problem { index: 0, text: "No home folder.".into() }])?;
    let saved = path
        .parent()
        .map(std::fs::create_dir_all)
        .transpose()
        .and_then(|_| std::fs::write(&path, file_text(&list)));
    saved.map_err(|e| vec![Problem { index: 0, text: format!("Can't save {}: {e}", path.display()) }])?;
    tracing::info!("assistant: {} routines saved", list.len());
    Ok(list)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_examples_work() {
        let list = defaults();
        for r in &list {
            assert_eq!(problem(r, &list), None, "{}", r.name);
        }
        let coding: Vec<&str> = expand(&list[0]).unwrap().iter().map(|s| s.tool).collect();
        assert_eq!(coding, ["open_app", "open_app", "place_window", "place_window", "do_not_disturb"]);
    }

    #[test]
    fn saying_a_routine() {
        let list = defaults();
        for t in ["coding", "Set up for coding.", "Hey Siva, start coding", "coding mode", "Time for coding", "let's code"] {
            let want = if t == "let's code" { None } else { Some("coding") };
            assert_eq!(invoked(t, &list).map(|r| r.name.as_str()), want, "{t}");
        }
        assert_eq!(invoked("movie time", &list).map(|r| r.name.as_str()), Some("movie"));
        assert_eq!(invoked("start the focus routine", &list).map(|r| r.name.as_str()), Some("focus"));
        assert_eq!(invoked("dark mode", &list), None, "a command, not a routine");
        assert_eq!(invoked("start firefox", &list), None);
    }

    #[test]
    fn the_file() {
        let list = defaults();
        assert_eq!(parse_file(&file_text(&list)), list);
        let read = parse_file("# x\n Work Day = open files ; volume 30;\nbroken line\n= nothing\nempty =\n");
        assert_eq!(read, vec![routine("work day", &["open files", "volume 30"])]);
    }

    #[test]
    fn what_settings_refuses() {
        let ok = routine("work", &["open files", "volume 30"]);
        assert_eq!(problem(&ok, std::slice::from_ref(&ok)), None);
        let p = |r: Routine| problem(&r, std::slice::from_ref(&r)).unwrap();
        assert!(p(routine("", &["volume 30"])).contains("name"));
        assert!(p(routine("mute", &["volume 30"])).contains("already"));
        assert!(p(routine("work", &["volume 30", "make me a sandwich"])).contains("make me a sandwich"));
        assert!(p(routine("work", &[])).contains("at least one"));
        assert!(p(routine("big", &["volume 30"; 9])).contains("at most 8"));
        let twice = [routine("work", &["volume 30"]), routine("Work", &["mute"])];
        assert!(problem(&twice[0], &twice).unwrap().contains("two routines"));
    }
}
