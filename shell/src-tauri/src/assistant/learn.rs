//! 19j: Siva learns your voice. Retraining whisper isn't possible on this PC, but its mistakes
//! with one person's accent repeat ("open terminal" heard as "open permanent", "volume down" as
//! "volume under button"), so what it hears from *you* is put right before anything reads it:
//!
//! - **phrases** you corrected: the HUD's ✎ on what it heard, or a training session in Settings
//!   (you read phrases, it keeps what it heard for each);
//! - **words** taken from those when the two differ in a word or two ("permanent" → "terminal"),
//!   so other requests with that word are put right too;
//! - **your words** (names you use) in whisper's hints, so it spells them your way.
//!
//! `~/.config/kompact/voice-learnt`: `heard: <as heard> => <meant>` and `word: <yours>` lines.
//! Pure and tested but for the file.

use std::path::PathBuf;

use serde::Serialize;

use super::rules::normalize;

/// The training session's phrases: the commands said most, and the words that are hard to hear.
pub const TRAINING: [&str; 22] = [
    "open terminal",
    "open firefox",
    "open files",
    "close firefox",
    "volume up",
    "volume down",
    "volume fifty",
    "mute the sound",
    "next song",
    "pause the music",
    "switch to firefox",
    "maximize firefox",
    "minimize the terminal",
    "put firefox on the left",
    "put the terminal on the right",
    "bring firefox back",
    "dark mode",
    "do not disturb",
    "go to desktop two",
    "take a screenshot",
    "set up for coding",
    "why is my computer slow",
];

/// At most this many phrases are kept (the newest win).
pub const KEEP: usize = 300;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Learnt {
    /// `(as heard, meant)`, both normalized, newest last.
    pub phrases: Vec<(String, String)>,
    /// Your words, as you typed them.
    pub words: Vec<String>,
}

impl Learnt {
    pub fn parse(text: &str) -> Learnt {
        let mut l = Learnt::default();
        for line in text.lines().map(str::trim) {
            if let Some(rest) = line.strip_prefix("heard:") {
                if let Some((h, m)) = rest.split_once("=>") {
                    l.add(h, m);
                }
            } else if let Some(w) = line.strip_prefix("word:") {
                l.add_word(w);
            }
        }
        l
    }

    pub fn text(&self) -> String {
        let mut s = String::from("# kompact: what Siva learnt about your voice (System Settings → Assistant → Learn my voice)\n");
        for (h, m) in &self.phrases {
            s.push_str(&format!("heard: {h} => {m}\n"));
        }
        for w in &self.words {
            s.push_str(&format!("word: {w}\n"));
        }
        s
    }

    /// Learn that `heard` meant `meant`. False if there's nothing to learn (it heard right, or
    /// either is empty).
    pub fn add(&mut self, heard: &str, meant: &str) -> bool {
        let (h, m) = (normalize(heard), normalize(meant));
        if h.is_empty() || m.is_empty() || h == m {
            return false;
        }
        self.phrases.retain(|(old, _)| *old != h);
        self.phrases.push((h, m));
        let extra = self.phrases.len().saturating_sub(KEEP);
        self.phrases.drain(..extra);
        true
    }

    pub fn add_word(&mut self, word: &str) -> bool {
        let w = word.split_whitespace().collect::<Vec<_>>().join(" ");
        if w.is_empty() || w.chars().count() > 40 || self.words.iter().any(|o| o.eq_ignore_ascii_case(&w)) {
            return false;
        }
        self.words.push(w);
        true
    }

    /// Single words learnt from phrases of the same length that differ in one or two places
    /// ("open permanent" / "open terminal" → permanent → terminal); a word two phrases disagree
    /// about isn't used.
    pub fn word_fixes(&self) -> Vec<(String, String)> {
        let mut out: Vec<(String, String)> = Vec::new();
        let mut doubtful: Vec<String> = Vec::new();
        for (h, m) in &self.phrases {
            let (hw, mw): (Vec<&str>, Vec<&str>) = (h.split_whitespace().collect(), m.split_whitespace().collect());
            if hw.len() != mw.len() {
                continue;
            }
            let diff: Vec<(&str, &str)> = hw.iter().zip(&mw).filter(|(a, b)| a != b).map(|(a, b)| (*a, *b)).collect();
            if diff.is_empty() || diff.len() > 2 {
                continue;
            }
            for (a, b) in diff {
                // Short words ("a", "to") are too common to swap everywhere.
                if a.len() < 3 {
                    continue;
                }
                match out.iter().find(|(x, _)| x == a) {
                    Some((_, y)) if y != b => doubtful.push(a.to_string()),
                    Some(_) => {}
                    None => out.push((a.to_string(), b.to_string())),
                }
            }
        }
        out.retain(|(a, _)| !doubtful.contains(a));
        out
    }

    /// What you meant by `heard`: the phrase learnt for all of it, else phrases learnt inside
    /// it, else its words put right. None if nothing learnt applies.
    pub fn apply(&self, heard: &str) -> Option<String> {
        let h = normalize(heard);
        if h.is_empty() {
            return None;
        }
        if let Some((_, m)) = self.phrases.iter().rev().find(|(x, _)| *x == h) {
            return Some(m.clone());
        }
        // Longer learnt phrases first, as whole words inside what was heard.
        let mut text = format!(" {h} ");
        let mut by_len: Vec<&(String, String)> = self.phrases.iter().filter(|(x, _)| x.contains(' ')).collect();
        by_len.sort_by_key(|(x, _)| std::cmp::Reverse(x.len()));
        for (x, m) in by_len {
            text = text.replace(&format!(" {x} "), &format!(" {m} "));
        }
        let words: Vec<String> = text
            .split_whitespace()
            .map(|w| self.word_fixes().into_iter().find(|(a, _)| a == w).map(|(_, b)| b).unwrap_or_else(|| w.to_string()))
            .collect();
        let fixed = words.join(" ");
        (fixed != h).then_some(fixed)
    }
}

// ---- the file ----

fn file() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
    Some(base.join("kompact").join("voice-learnt"))
}

pub fn load() -> Learnt {
    file().and_then(|f| std::fs::read_to_string(f).ok()).map(|t| Learnt::parse(&t)).unwrap_or_default()
}

pub fn save(l: &Learnt) -> Result<(), String> {
    let f = file().ok_or("no home folder")?;
    if let Some(dir) = f.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    std::fs::write(&f, l.text()).map_err(|e| format!("can't save {}: {e}", f.display()))
}

/// Learn one correction and keep it (the HUD's ✎, the training session).
pub fn remember(heard: &str, meant: &str) -> bool {
    let mut l = load();
    if !l.add(heard, meant) {
        return false;
    }
    match save(&l) {
        Ok(()) => {
            tracing::info!("voice: learnt {:?} means {:?}", normalize(heard), normalize(meant));
            true
        }
        Err(e) => {
            tracing::warn!("voice: {e}");
            false
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct LearntState {
    pub phrases: Vec<(String, String)>,
    pub word_fixes: Vec<(String, String)>,
    pub words: Vec<String>,
    pub training: Vec<&'static str>,
}

#[tauri::command]
pub fn voice_learnt() -> LearntState {
    let l = load();
    LearntState { word_fixes: l.word_fixes(), phrases: l.phrases, words: l.words, training: TRAINING.to_vec() }
}

/// Settings: forget one phrase (by what was heard), set your words, or clear it all.
#[tauri::command]
pub fn voice_learnt_change(forget: Option<String>, words: Option<Vec<String>>, clear: Option<bool>) -> Result<LearntState, String> {
    let mut l = load();
    if clear == Some(true) {
        l = Learnt::default();
    }
    if let Some(h) = forget {
        l.phrases.retain(|(x, _)| *x != normalize(&h));
    }
    if let Some(ws) = words {
        l.words.clear();
        for w in ws {
            l.add_word(&w);
        }
    }
    save(&l)?;
    tracing::info!("voice: learnt list changed ({} phrases, {} words)", l.phrases.len(), l.words.len());
    Ok(voice_learnt())
}

/// The training session: what you meant to say, and what was heard.
#[tauri::command]
pub fn voice_learn(heard: String, meant: String) -> bool {
    remember(&heard, &meant)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn taught(pairs: &[(&str, &str)]) -> Learnt {
        let mut l = Learnt::default();
        for (h, m) in pairs {
            l.add(h, m);
        }
        l
    }

    #[test]
    fn putting_right_what_was_heard() {
        // From the user's log, 2026-10-24.
        let l = taught(&[("Open permanent", "open terminal"), ("Volume under button.", "volume down"), ("Print Firefox back", "bring firefox back")]);
        assert_eq!(l.apply("Open permanent").as_deref(), Some("open terminal"), "the whole phrase");
        assert_eq!(l.apply("Volume under button").as_deref(), Some("volume down"));
        assert_eq!(l.apply("put permanent on the left").as_deref(), Some("put terminal on the left"), "the word, elsewhere");
        assert_eq!(l.apply("print the page").as_deref(), Some("bring the page"), "print → bring was learnt as a word");
        assert_eq!(l.apply("open firefox"), None, "nothing learnt applies");
        assert_eq!(l.apply("so, volume under button now").as_deref(), Some("so volume down now"), "a phrase inside");
    }

    #[test]
    fn what_is_worth_learning() {
        let mut l = Learnt::default();
        assert!(!l.add("Open terminal.", "open terminal"), "heard right");
        assert!(!l.add("", "open terminal"));
        assert!(l.add("open table", "open terminal"));
        assert!(l.add("open table", "open tables app"), "the newest wins");
        assert_eq!(l.phrases, vec![("open table".into(), "open tables app".into())]);
        // Words only from phrases of the same length, one or two words apart, 3 letters or more.
        let l = taught(&[("a bc", "the de"), ("totally different words here", "open terminal"), ("go to desk top", "go to desktop")]);
        assert_eq!(l.word_fixes(), vec![]);
        // Two phrases that disagree about a word: neither is used.
        let l = taught(&[("open table", "open terminal"), ("table mode", "dark mode")]);
        assert_eq!(l.word_fixes(), vec![]);
    }

    #[test]
    fn the_file() {
        let mut l = taught(&[("open permanent", "open terminal")]);
        l.add_word("Ptyxis");
        l.add_word("ptyxis");
        l.add_word("  Code  OSS ");
        assert_eq!(l.words, vec!["Ptyxis".to_string(), "Code OSS".to_string()]);
        assert_eq!(Learnt::parse(&l.text()), l);
        assert_eq!(Learnt::parse("junk\nheard: no arrow\nword:\n"), Learnt::default());
        let mut many = Learnt::default();
        for i in 0..KEEP + 5 {
            many.add(&format!("heard {i}"), "meant");
        }
        assert_eq!(many.phrases.len(), KEEP);
        assert_eq!(many.phrases[0].0, "heard 5", "the oldest go");
    }

    #[test]
    fn training_phrases_are_understood() {
        for p in TRAINING {
            assert!(!matches!(super::super::rules::parse(p), super::super::rules::Parsed::Unknown(_)) || super::super::routines::invoked(p, &super::super::routines::defaults()).is_some(), "{p}");
        }
    }
}
