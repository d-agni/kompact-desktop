//! 21d: Siva sees inside apps: a window's contents read through accessibility (AT-SPI, the
//! same tree screen readers use), as a short outline of what's on it: headings, text, fields
//! and their values, buttons, links, boxes ticked or not. `read_window` (Tier 0).
//!
//! The tree comes from the accessibility bus (`org.a11y.Bus.GetAddress`), walked over D-Bus
//! with `zbus`: only what's showing, at most `MAX_NODES` items or `MAX_TIME`; a password
//! field's contents are never read, and the shell's own windows aren't read at all. What's read
//! is someone else's text (a web page, a message): to a task (`agent`) it's information, never
//! instructions, and it counts as from outside (`agent::taints`).
//!
//! Settings → Assistant → Permissions → Read inside apps (on by default): while on, Siva also
//! sets the bus's `IsEnabled`, which Firefox and Chromium-based apps wait for before they
//! build their tree (GTK 4 apps always have one); they see it when they start.
//!
//! Turning the tree into the outline is pure and tested.

use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use tracing::info;
use zbus::zvariant::{OwnedObjectPath, OwnedValue};

/// At most this many items are read, and for this long.
pub const MAX_NODES: usize = 3000;
pub const MAX_TIME: Duration = Duration::from_secs(3);
const MAX_DEPTH: usize = 40;
/// At most this much of one item's text, and of the whole outline.
const MAX_TEXT: i32 = 4000;
pub const MAX_OUTLINE: usize = 6000;

// ---- AT-SPI's numbers (checked against libatspi 2.0's enums, 2026-10-25) ----

pub mod role {
    pub const ALERT: u32 = 2;
    pub const CHECK_BOX: u32 = 7;
    pub const COMBO_BOX: u32 = 11;
    pub const DIALOG: u32 = 16;
    pub const FRAME: u32 = 23;
    pub const LABEL: u32 = 29;
    pub const LIST_ITEM: u32 = 32;
    pub const MENU_ITEM: u32 = 35;
    pub const PAGE_TAB: u32 = 37;
    pub const PASSWORD_TEXT: u32 = 40;
    pub const PUSH_BUTTON: u32 = 43;
    pub const RADIO_BUTTON: u32 = 44;
    pub const SCROLL_BAR: u32 = 48;
    pub const SEPARATOR: u32 = 50;
    pub const SPIN_BUTTON: u32 = 52;
    pub const TABLE_CELL: u32 = 56;
    pub const TERMINAL: u32 = 60;
    pub const TOGGLE_BUTTON: u32 = 62;
    pub const ENTRY: u32 = 79;
    pub const DOCUMENT_FRAME: u32 = 82;
    pub const HEADING: u32 = 83;
    pub const LINK: u32 = 88;
    pub const TREE_ITEM: u32 = 91;
    pub const DOCUMENT_TEXT: u32 = 94;
    pub const DOCUMENT_WEB: u32 = 95;
    pub const DOCUMENT_EMAIL: u32 = 96;
}

pub mod state {
    pub const BUSY: u32 = 3;
    pub const CHECKED: u32 = 4;
    pub const FOCUSED: u32 = 12;
    pub const SELECTED: u32 = 23;
    pub const SHOWING: u32 = 25;
    pub const VISIBLE: u32 = 30;
}

/// One item of a window, as read.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Node {
    pub role: u32,
    pub name: String,
    /// Its text (the Text interface), if it has one; U+FFFC marks where a child sits in it.
    pub text: Option<String>,
    /// AT-SPI's state set: two 32-bit words.
    pub states: [u32; 2],
    pub children: Vec<Node>,
}

impl Node {
    pub fn has(&self, s: u32) -> bool {
        self.states[(s / 32) as usize % 2] & (1 << (s % 32)) != 0
    }
}

// ---- the outline (pure) ----

fn clean(t: &str) -> String {
    t.replace('\u{fffc}', " ").split_whitespace().collect::<Vec<_>>().join(" ")
}

/// A text item's words with each U+FFFC replaced by its child's words (the k-th mark is the
/// k-th child), as screen readers read a paragraph with links in it.
fn inline(n: &Node) -> String {
    let Some(text) = &n.text else { return flat(n) };
    let mut kids = n.children.iter();
    let mut out = String::new();
    for c in text.chars() {
        if c == '\u{fffc}' {
            if let Some(k) = kids.next() {
                out.push(' ');
                out.push_str(&flat(k));
                out.push(' ');
            }
        } else {
            out.push(c);
        }
    }
    clean(&out)
}

/// An item's words, children and all, on one line.
fn flat(n: &Node) -> String {
    if n.role == role::PASSWORD_TEXT {
        return String::new();
    }
    if n.text.is_some() {
        return inline(n);
    }
    let mut parts = vec![clean(&n.name)];
    parts.extend(n.children.iter().map(flat));
    clean(&parts.join(" "))
}

/// The lines of the outline for `n` (only what's showing; the root is shown whatever it says).
fn lines(n: &Node, out: &mut Vec<String>, root: bool) {
    if !root && !n.has(state::SHOWING) {
        return;
    }
    let name = clean(&n.name);
    let line = match n.role {
        role::SCROLL_BAR | role::SEPARATOR => return,
        role::PASSWORD_TEXT => Some(format!("[password field{}] (not read)", if name.is_empty() { String::new() } else { format!(": {name}") })),
        role::PUSH_BUTTON | role::TOGGLE_BUTTON => {
            let pressed = n.role == role::TOGGLE_BUTTON && n.has(state::CHECKED);
            let label = if name.is_empty() { flat(n) } else { name.clone() };
            (!label.is_empty()).then(|| format!("[button: {label}]{}", if pressed { " (on)" } else { "" }))
        }
        role::CHECK_BOX | role::RADIO_BUTTON => Some(format!("[{}] {}", if n.has(state::CHECKED) { "x" } else { " " }, if name.is_empty() { flat(n) } else { name.clone() })),
        role::LINK => {
            let label = if name.is_empty() { flat(n) } else { name.clone() };
            (!label.is_empty()).then(|| format!("[link: {label}]"))
        }
        role::HEADING => Some(format!("# {}", if name.is_empty() { flat(n) } else { name.clone() })),
        role::PAGE_TAB => Some(format!("[tab: {name}]{}", if n.has(state::SELECTED) { " (open)" } else { "" })),
        role::MENU_ITEM => (!name.is_empty()).then(|| format!("[menu: {name}]")),
        role::LIST_ITEM | role::TREE_ITEM => {
            let t = flat(n);
            (!t.is_empty()).then(|| format!("- {t}{}", if n.has(state::SELECTED) { " (selected)" } else { "" }))
        }
        role::ENTRY | role::COMBO_BOX | role::SPIN_BUTTON => {
            let value = n.text.as_deref().map(clean).unwrap_or_default();
            let what = if name.is_empty() { "field".to_string() } else { format!("field “{name}”") };
            Some(format!("[{what}: {value}]{}", if n.has(state::FOCUSED) { " (typing here)" } else { "" }))
        }
        role::TERMINAL => n.text.as_deref().map(|t| {
            // A terminal's text, its lines kept (the last ones matter most).
            let kept: Vec<&str> = t.lines().map(str::trim_end).filter(|l| !l.is_empty()).collect();
            format!("[terminal]\n{}", kept.join("\n"))
        }),
        role::TABLE_CELL => {
            let t = flat(n);
            (!t.is_empty()).then_some(t)
        }
        _ if n.text.is_some() => {
            // A label, a paragraph, a document's text: its words, links and all.
            let t = inline(n);
            (!t.is_empty()).then_some(t)
        }
        role::DIALOG | role::ALERT if !name.is_empty() => Some(format!("## {name}")),
        _ => None,
    };
    if let Some(l) = line {
        // A label that repeats the line before it (a title shown twice) once.
        if out.last() != Some(&l) {
            out.push(l);
        }
        // What the line said for its children is said; buttons, links and text are whole.
        let whole = matches!(n.role, role::PUSH_BUTTON | role::TOGGLE_BUTTON | role::LINK | role::HEADING | role::LIST_ITEM | role::TREE_ITEM | role::CHECK_BOX | role::RADIO_BUTTON | role::TABLE_CELL | role::TERMINAL | role::PASSWORD_TEXT | role::ENTRY | role::SPIN_BUTTON)
            || n.text.is_some();
        if whole {
            return;
        }
    }
    for c in &n.children {
        lines(c, out, false);
    }
}

/// The window `n` (its frame) as an outline of at most `max` characters, its title first.
pub fn outline(n: &Node, max: usize) -> String {
    let mut out = Vec::new();
    let title = clean(&n.name);
    if !title.is_empty() {
        out.push(format!("Window: {title}"));
    }
    for c in &n.children {
        lines(c, &mut out, false);
    }
    let text = out.join("\n");
    if text.chars().count() <= max {
        return text;
    }
    // A terminal's last lines matter most; elsewhere the start does.
    if n.children.len() == 1 && text.contains("[terminal]\n") {
        return super::work::tail(&text, max);
    }
    super::work::head(&text, max)
}

// ---- 21f: reading a page out (pure) ----

/// What `read_as` gives: the outline (for a task), a page's headlines, or a page's text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Want {
    Outline,
    Headlines,
    Page,
}

/// The page in a window (a browser's tab, a document), without the app around it: the first
/// document that's showing; the whole window if there's none.
fn document(n: &Node) -> Option<&Node> {
    if matches!(n.role, role::DOCUMENT_WEB | role::DOCUMENT_FRAME | role::DOCUMENT_TEXT | role::DOCUMENT_EMAIL) && n.has(state::SHOWING) {
        return Some(n);
    }
    n.children.iter().find_map(document)
}

/// Whether `t` reads like a headline, not a menu or a button: a few words at least.
fn headline_like(t: &str) -> bool {
    t.split_whitespace().count() >= 4 && t.chars().count() >= 30
}

/// A headline as said: without "- 2 hours ago" and "- By …" after its source (Google News puts
/// them in the link); None for a page's own links ("See more headlines and perspectives").
fn tidy_headline(t: &str) -> Option<String> {
    let lower = t.to_lowercase();
    if ["see more", "more headlines", "full coverage", "view full coverage", "more stories"].iter().any(|p| lower.starts_with(p)) {
        return None;
    }
    let parts: Vec<&str> = t.split(" - ").collect();
    let kept: Vec<&str> = parts
        .iter()
        .enumerate()
        .filter(|(i, p)| *i == 0 || !(p.starts_with("By ") || p.ends_with(" ago") || **p == "LIVE"))
        .map(|(_, p)| *p)
        .collect();
    Some(kept.join(" - "))
}

fn collect_headlines(n: &Node, out: &mut Vec<String>, max: usize) {
    if out.len() >= max || !n.has(state::SHOWING) || n.role == role::PASSWORD_TEXT {
        return;
    }
    if matches!(n.role, role::HEADING | role::LINK) {
        let t = if n.name.trim().is_empty() { flat(n) } else { clean(&n.name) };
        // The same story twice (its picture's link, its title's) once.
        if let Some(t) = tidy_headline(&t).filter(|t| headline_like(t)) {
            if !out.iter().any(|o| o.eq_ignore_ascii_case(&t)) {
                out.push(t);
            }
        }
        return;
    }
    for c in &n.children {
        collect_headlines(c, out, max);
    }
}

/// The headlines on the page in window `n`: its headings and links that read like one, in
/// order, at most `max`.
pub fn headlines(n: &Node, max: usize) -> Vec<String> {
    let mut out = Vec::new();
    collect_headlines(document(n).unwrap_or(n), &mut out, max);
    out
}

/// The page in window `n` as words to read out, at most `max` characters: its headings and
/// text, a line each, without buttons, fields or short links (menus).
pub fn page_text(n: &Node, max: usize) -> String {
    let doc = document(n).unwrap_or(n);
    let mut all = Vec::new();
    if doc.text.is_some() {
        lines(doc, &mut all, true);
    } else {
        for c in &doc.children {
            lines(c, &mut all, false);
        }
    }
    let kept: Vec<String> = all
        .into_iter()
        .filter_map(|l| {
            if let Some(h) = l.strip_prefix("# ") {
                return Some(h.to_string());
            }
            if let Some(link) = l.strip_prefix("[link: ").and_then(|r| r.strip_suffix(']')) {
                return headline_like(link).then(|| link.to_string());
            }
            if let Some(item) = l.strip_prefix("- ") {
                return Some(item.to_string());
            }
            (!l.starts_with('[')).then_some(l)
        })
        .collect();
    super::work::head(&kept.join("\n"), max)
}

/// What window `n` gives as `want`, and how much that is (for "the fullest read").
pub fn as_wanted(n: &Node, want: Want, max: usize) -> (String, usize) {
    match want {
        Want::Outline => {
            let t = outline(n, max);
            let size = t.chars().count();
            (t, size)
        }
        Want::Headlines => {
            let h = headlines(n, HEADLINES);
            let size = h.len();
            (h.join("\n"), size)
        }
        Want::Page => {
            let t = page_text(n, max);
            let size = t.chars().count();
            (t, size)
        }
    }
}

/// How many headlines are read out.
pub const HEADLINES: usize = 10;

/// How well an app's frame fits a window of kompact's (`title`, `app_id`, `name`): 3 the same
/// title, 1 the same app, 0 not it.
pub fn fit(app_name: &str, frame: &str, title: &str, app_id: &str, name: &str) -> u32 {
    let norm = |s: &str| s.to_lowercase().replace([' ', '-', '_'], "");
    let app = norm(app_name);
    let id_tail = app_id.rsplit('.').next().unwrap_or(app_id);
    let same_app = !app.is_empty() && [norm(app_id), norm(id_tail), norm(name)].iter().any(|k| !k.is_empty() && (app.contains(k.as_str()) || k.contains(app.as_str())));
    if !title.is_empty() && clean(frame) == clean(title) {
        3
    } else if same_app {
        1
    } else {
        0
    }
}

// ---- Settings: Read inside apps (on by default) ----

fn prefs_file() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from).or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
    Some(base.join("kompact").join("assistant-apps"))
}

pub fn parse_prefs(text: &str) -> bool {
    text.lines().filter_map(|l| l.trim().strip_prefix("read=")).map(|v| v.trim() != "off").last().unwrap_or(true)
}

pub fn allowed() -> bool {
    parse_prefs(&prefs_file().and_then(|f| std::fs::read_to_string(f).ok()).unwrap_or_default())
}

// ---- reading (D-Bus) ----

static BUS: Mutex<Option<zbus::Connection>> = Mutex::new(None);

async fn bus() -> Result<zbus::Connection, String> {
    if let Some(c) = BUS.lock().unwrap_or_else(|e| e.into_inner()).clone() {
        return Ok(c);
    }
    let session = zbus::Connection::session().await.map_err(|e| format!("session bus: {e}"))?;
    let reply = session.call_method(Some("org.a11y.Bus"), "/org/a11y/bus", Some("org.a11y.Bus"), "GetAddress", &()).await.map_err(|e| format!("no accessibility bus: {e}"))?;
    let address: String = reply.body().deserialize().map_err(|e| e.to_string())?;
    let c = zbus::connection::Builder::address(address.as_str()).map_err(|e| e.to_string())?.build().await.map_err(|e| format!("accessibility bus: {e}"))?;
    *BUS.lock().unwrap_or_else(|e| e.into_inner()) = Some(c.clone());
    Ok(c)
}

/// The accessibility flag apps like Firefox wait for (on the session bus).
pub async fn set_enabled(on: bool) -> Result<(), String> {
    let session = zbus::Connection::session().await.map_err(|e| format!("session bus: {e}"))?;
    session
        .call_method(Some("org.a11y.Bus"), "/org/a11y/bus", Some("org.freedesktop.DBus.Properties"), "Set", &("org.a11y.Status", "IsEnabled", zbus::zvariant::Value::from(on)))
        .await
        .map_err(|e| format!("IsEnabled: {e}"))?;
    info!("inside: accessibility {}", if on { "on (apps build their tree)" } else { "off" });
    Ok(())
}

/// At the shell's start: the flag as Settings say.
pub fn apply_at_start() {
    if allowed() {
        tauri::async_runtime::spawn(async {
            if let Err(e) = set_enabled(true).await {
                info!("inside: {e}");
            }
        });
    }
}

type Ref = (String, OwnedObjectPath);

const ACCESSIBLE: &str = "org.a11y.atspi.Accessible";

async fn children(c: &zbus::Connection, (dest, path): &Ref) -> Vec<Ref> {
    match c.call_method(Some(dest.as_str()), path.as_str(), Some(ACCESSIBLE), "GetChildren", &()).await {
        Ok(m) => m.body().deserialize::<Vec<Ref>>().unwrap_or_default(),
        Err(_) => Vec::new(),
    }
}

async fn name(c: &zbus::Connection, (dest, path): &Ref) -> String {
    let Ok(m) = c.call_method(Some(dest.as_str()), path.as_str(), Some("org.freedesktop.DBus.Properties"), "Get", &(ACCESSIBLE, "Name")).await else { return String::new() };
    m.body().deserialize::<OwnedValue>().ok().and_then(|v| String::try_from(v).ok()).unwrap_or_default()
}

async fn get<T: serde::de::DeserializeOwned + zbus::zvariant::Type>(c: &zbus::Connection, (dest, path): &Ref, iface: &str, method: &str) -> Option<T> {
    c.call_method(Some(dest.as_str()), path.as_str(), Some(iface), method, &()).await.ok()?.body().deserialize::<T>().ok()
}

async fn text_of(c: &zbus::Connection, (dest, path): &Ref) -> Option<String> {
    let m = c.call_method(Some(dest.as_str()), path.as_str(), Some("org.a11y.atspi.Text"), "GetText", &(0i32, MAX_TEXT)).await.ok()?;
    m.body().deserialize::<String>().ok()
}

/// One item and what's under it (only what's showing), within the budget. `in_page`: it's in
/// the page that's showing (21f: there, the part of the page below the screen is read too:
/// Firefox calls only what's on screen "showing"; visible is shown on the page).
async fn read(c: &zbus::Connection, r: &Ref, depth: usize, budget: &mut usize, until: Instant, in_page: bool) -> Node {
    *budget = budget.saturating_sub(1);
    let states: Vec<u32> = get(c, r, ACCESSIBLE, "GetState").await.unwrap_or_default();
    let mut n = Node { states: [states.first().copied().unwrap_or(0), states.get(1).copied().unwrap_or(0)], ..Node::default() };
    if in_page && n.has(state::VISIBLE) {
        n.states[(state::SHOWING / 32) as usize] |= 1 << (state::SHOWING % 32);
    }
    if depth > 0 && !n.has(state::SHOWING) {
        return n;
    }
    n.role = get(c, r, ACCESSIBLE, "GetRole").await.unwrap_or(0);
    let in_page = in_page || matches!(n.role, role::DOCUMENT_WEB | role::DOCUMENT_FRAME | role::DOCUMENT_TEXT | role::DOCUMENT_EMAIL);
    n.name = name(c, r).await;
    if n.role == role::SCROLL_BAR || n.role == role::SEPARATOR {
        return n;
    }
    let ifaces: Vec<String> = get(c, r, ACCESSIBLE, "GetInterfaces").await.unwrap_or_default();
    // Never a password field's contents.
    if n.role != role::PASSWORD_TEXT && ifaces.iter().any(|i| i == "org.a11y.atspi.Text") {
        n.text = text_of(c, r).await.filter(|t| !t.trim().is_empty() || n.role == role::ENTRY);
    }
    if depth >= MAX_DEPTH {
        return n;
    }
    for k in children(c, r).await {
        if *budget == 0 || Instant::now() > until {
            break;
        }
        n.children.push(Box::pin(read(c, &k, depth + 1, budget, until, in_page)).await);
    }
    n
}

/// 21e-2: how long a page just opened may take to load, and how little is "next to nothing".
const LOADING: Duration = Duration::from_secs(6);
const THIN: usize = 800;

/// Whether anything in `n` says it's still loading.
pub fn busy(n: &Node) -> bool {
    n.has(state::BUSY) || n.children.iter().any(busy)
}

/// The window best fitting `title` / `app_id` / `name` (kompact's) as `want` (an outline, its
/// headlines or its page's text) of at most `max` characters. `fresh`: a page was just opened in it (the one before may still show
/// for a moment); with what was read, in how long.
pub async fn read_as(title: &str, app_id: &str, app_name: &str, want: Want, max: usize, fresh: bool) -> Result<String, String> {
    if !allowed() {
        return Err("Reading inside apps is off in Settings → Assistant → Permissions.".into());
    }
    let started = Instant::now();
    let c = bus().await?;
    let root: Ref = ("org.a11y.atspi.Registry".into(), OwnedObjectPath::try_from("/org/a11y/atspi/accessible/root").map_err(|e| e.to_string())?);
    let mut best: Option<(u32, Ref, String)> = None;
    for app in children(&c, &root).await {
        let an = name(&c, &app).await;
        // The shell's own windows (the HUD, Settings) aren't read.
        if an == "kompact-shell" || an.starts_with("kompact") {
            continue;
        }
        for frame in children(&c, &app).await {
            let fname = name(&c, &frame).await;
            let score = fit(&an, &fname, title, app_id, app_name);
            if score > 0 && best.as_ref().is_none_or(|(s, ..)| score > *s) {
                best = Some((score, frame, an.clone()));
            }
        }
    }
    let Some((score, frame, an)) = best else {
        return Err(format!("I can't see inside {app_name}: it doesn't share its contents (Firefox and Chrome-based apps do after a restart, once “Read inside apps” is on)."));
    };
    // 21e-2: a page just opened is still loading (busy, or next to nothing in it): read again,
    // up to `LOADING`, and keep the fullest read.
    let enough = |size: usize| match want {
        Want::Headlines => size >= 3,
        _ => size >= THIN,
    };
    let until = started + if fresh { LOADING * 2 } else { LOADING };
    if fresh {
        tokio::time::sleep(Duration::from_millis(1200)).await;
    }
    let mut budget = MAX_NODES;
    let mut node = read(&c, &frame, 0, &mut budget, Instant::now() + MAX_TIME, false).await;
    let (mut text, mut size) = as_wanted(&node, want, max);
    while (busy(&node) || !enough(size)) && Instant::now() < until {
        tokio::time::sleep(Duration::from_millis(700)).await;
        budget = MAX_NODES;
        node = read(&c, &frame, 0, &mut budget, Instant::now() + MAX_TIME, false).await;
        let (again, more) = as_wanted(&node, want, max);
        if more >= size {
            (text, size) = (again, more);
        }
    }
    info!("inside: read {an}'s window as {want:?} (fit {score}) in {} ms, {} items, {} characters", started.elapsed().as_millis(), MAX_NODES - budget, text.chars().count());
    Ok(text)
}

/// Settings → Assistant → Permissions → Read inside apps.
#[tauri::command]
pub async fn set_read_apps(on: bool) -> Result<bool, String> {
    let file = prefs_file().ok_or("no home folder")?;
    if let Some(dir) = file.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    std::fs::write(&file, format!("# Siva reading inside apps (written by System Settings → Assistant)\nread={}\n", if on { "on" } else { "off" })).map_err(|e| format!("can't save {}: {e}", file.display()))?;
    set_enabled(on).await?;
    Ok(on)
}

#[tauri::command]
pub fn read_apps_state() -> bool {
    allowed()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn showing() -> [u32; 2] {
        [1 << state::SHOWING, 0]
    }

    fn n(role: u32, name: &str, children: Vec<Node>) -> Node {
        Node { role, name: name.into(), text: None, states: showing(), children }
    }

    fn t(role: u32, name: &str, text: &str) -> Node {
        Node { role, name: name.into(), text: Some(text.into()), states: showing(), children: Vec::new() }
    }

    const PANEL: u32 = 39;

    #[test]
    fn a_window_as_an_outline() {
        let mut ticked = n(role::CHECK_BOX, "Remember me", vec![]);
        ticked.states[0] |= 1 << state::CHECKED;
        let mut hidden = t(role::LABEL, "", "a hidden label");
        hidden.states = [0, 0];
        let mut field = t(role::ENTRY, "Search", "rust traits");
        field.states[0] |= 1 << state::FOCUSED;
        // A paragraph with a link in it: the link's words where its mark is.
        let mut para = t(73, "", "Read the \u{fffc} first.");
        para.children.push(t(role::LINK, "", "guide"));
        let w = n(
            role::FRAME,
            "Sign in — Mozilla Firefox",
            vec![n(
                PANEL,
                "",
                vec![
                    t(role::HEADING, "", "Welcome back"),
                    para,
                    field,
                    n(role::PASSWORD_TEXT, "Password", vec![]),
                    ticked,
                    n(role::PUSH_BUTTON, "Sign in", vec![]),
                    n(role::LINK, "Forgot your password?", vec![]),
                    hidden,
                    n(role::SCROLL_BAR, "", vec![]),
                ],
            )],
        );
        assert_eq!(
            outline(&w, 2000),
            "Window: Sign in — Mozilla Firefox\n# Welcome back\nRead the guide first.\n[field “Search”: rust traits] (typing here)\n[password field: Password] (not read)\n[x] Remember me\n[button: Sign in]\n[link: Forgot your password?]"
        );
    }

    #[test]
    fn a_password_is_never_read() {
        let mut p = t(role::PASSWORD_TEXT, "Password", "hunter2");
        p.children.push(t(role::LABEL, "", "hunter2"));
        let w = n(role::FRAME, "x", vec![p]);
        assert!(!outline(&w, 2000).contains("hunter2"));
        assert!(!flat(&n(PANEL, "", vec![t(role::PASSWORD_TEXT, "", "hunter2")])).contains("hunter2"));
    }

    #[test]
    fn a_terminal_keeps_its_last_lines() {
        let lines: Vec<String> = (1..=400).map(|i| format!("line {i}")).collect();
        let w = n(role::FRAME, "Terminal", vec![t(role::TERMINAL, "Terminal", &lines.join("\n"))]);
        let o = outline(&w, 200);
        assert!(o.ends_with("line 400"), "{o}");
        assert!(o.starts_with('…'));
        assert!(o.chars().count() <= 202);
    }

    /// A real window through the accessibility bus (`LIVE_APP=ptyxis`, by default the
    /// terminal): its first lines and how long it took.
    #[test]
    #[ignore]
    fn live_read() {
        let app = std::env::var("LIVE_APP").unwrap_or_else(|_| "org.gnome.Ptyxis".into());
        let started = Instant::now();
        // 21f: `LIVE_WANT=headlines|page`; then only how much (`LIVE_SHOW=1` for the lines).
        let want = match std::env::var("LIVE_WANT").as_deref() {
            Ok("headlines") => Want::Headlines,
            Ok("page") => Want::Page,
            _ => Want::Outline,
        };
        let show = want == Want::Outline || std::env::var_os("LIVE_SHOW").is_some();
        let r = tauri::async_runtime::block_on(read_as("", &app, &app, want, MAX_OUTLINE, false));
        match r {
            Ok(t) if !show => println!("after {} ms: {} lines, {} characters", started.elapsed().as_millis(), t.lines().count(), t.chars().count()),
            Ok(t) => println!("after {} ms, {} characters:\n{}", started.elapsed().as_millis(), t.chars().count(), t.lines().take(if want == Want::Outline { 6 } else { 12 }).collect::<Vec<_>>().join("\n")),
            Err(e) => println!("failed: {e}"),
        }
    }

    #[test]
    fn a_page_still_loading() {
        let mut doc = n(95, "Google News", vec![]);
        assert!(!busy(&n(role::FRAME, "x", vec![doc.clone()])));
        doc.states[0] |= 1 << state::BUSY;
        assert!(busy(&n(role::FRAME, "x", vec![n(PANEL, "", vec![doc])])));
    }

    #[test]
    fn headlines_on_a_page() {
        let mut background = n(role::DOCUMENT_WEB, "Old tab", vec![n(role::LINK, "A story in a tab that isn't showing now", vec![])]);
        background.states = [0, 0];
        let story = |title: &str| n(109, "", vec![n(role::LINK, "", vec![]), n(role::LINK, title, vec![]), t(role::LABEL, "", "The Hindu · 1 hour ago")]);
        let page = n(
            role::DOCUMENT_WEB,
            "Google News",
            vec![
                n(role::LINK, "Home", vec![]),
                n(role::LINK, "For you", vec![]),
                t(role::HEADING, "", "Top stories"),
                story("Cyclone Arnab to make landfall near Kakinada tonight, IMD says"),
                story("Sensex closes 600 points higher on a bank rally"),
                n(role::LINK, "Cyclone Arnab to make landfall near Kakinada tonight, IMD says", vec![]),
                n(role::PUSH_BUTTON, "Full coverage of the cyclone and the rains", vec![]),
            ],
        );
        let w = n(role::FRAME, "Google News — Mozilla Firefox", vec![n(PANEL, "", vec![n(role::LINK, "A bookmark in the toolbar with a long name", vec![])]), background, page]);
        assert_eq!(headlines(&w, 10), ["Cyclone Arnab to make landfall near Kakinada tonight, IMD says", "Sensex closes 600 points higher on a bank rally"]);
        assert_eq!(headlines(&w, 1).len(), 1);
        // From Google News (2026-10-27).
        assert_eq!(tidy_headline("'Very severe cyclone' Arnab to hit Andhra coast on Oct 28 - Rediff - 1 hour ago - By Utkarsh Mishra").as_deref(), Some("'Very severe cyclone' Arnab to hit Andhra coast on Oct 28 - Rediff"));
        assert_eq!(tidy_headline("Odisha and Andhra Pradesh on alert for Cyclone Arnab").as_deref(), Some("Odisha and Andhra Pradesh on alert for Cyclone Arnab"));
        assert_eq!(tidy_headline("See more headlines and perspectives - Odisha and Andhra Pradesh on alert"), None);
        let text = page_text(&w, 2000);
        assert_eq!(text, "Top stories\nCyclone Arnab to make landfall near Kakinada tonight, IMD says\nThe Hindu · 1 hour ago\nSensex closes 600 points higher on a bank rally\nThe Hindu · 1 hour ago\nCyclone Arnab to make landfall near Kakinada tonight, IMD says");
        assert_eq!(as_wanted(&w, Want::Headlines, 2000).1, 2);
    }

    #[test]
    fn which_window() {
        assert_eq!(fit("ptyxis", "◐ build — kompact", "◐ build — kompact", "org.gnome.Ptyxis", "Terminal"), 3);
        assert_eq!(fit("Firefox", "Docs — Mozilla Firefox", "Other tab — Mozilla Firefox", "firefox", "Firefox"), 1);
        assert_eq!(fit("gnome-text-editor", "notes.txt", "", "org.gnome.TextEditor", "Text Editor"), 1);
        assert_eq!(fit("ptyxis", "x", "Docs — Mozilla Firefox", "firefox", "Firefox"), 0);
        assert!(parse_prefs(""), "on by default");
        assert!(!parse_prefs("read=off\n"));
        assert!(parse_prefs("read=off\nread=on\n"));
    }
}
