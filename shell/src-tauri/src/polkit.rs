//! 12e: our own polkit authentication agent (GNOME Shell has one built in; kompact sessions had
//! none, so admin actions that need a password, like the time zone, a system-wide network or
//! mounting a disk, simply failed). The shell registers
//! `org.freedesktop.PolicyKit1.AuthenticationAgent` for this login session; when polkit asks
//! (`BeginAuthentication`) the overlay shows a password dialog (`Mode::Auth`).
//!
//! The password is checked by polkit's own helper, never by us: polkit 127 runs it per request
//! through the systemd socket `/run/polkit/agent-helper.socket` (the setuid helper of older
//! versions, run with the user name, is the fallback). We send the user name and the request's
//! cookie, answer the PAM conversation (`PAM_PROMPT_ECHO_OFF …` → the password) and read
//! `SUCCESS` / `FAILURE`; on success the helper itself tells polkit. A wrong password can be
//! tried again (`MAX_TRIES`); Cancel, or polkit's `CancelAuthentication`, cancels.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::sync::Mutex;
use std::time::Duration;

use serde::Serialize;
use serde_json::json;
use tauri::{AppHandle, Emitter, Manager};
use tracing::{info, warn};
use zbus::zvariant::{OwnedValue, Value as ZValue};

const AGENT_PATH: &str = "/org/kompact/PolicyKit1/AuthenticationAgent";
const HELPER_SOCKET: &str = "/run/polkit/agent-helper.socket";
const HELPER: &str = "/usr/lib/polkit-1/polkit-agent-helper-1";
/// Wrong passwords allowed before the request fails.
pub const MAX_TRIES: u32 = 3;

// ---- The pure part ----

/// One line from the helper.
#[derive(Debug, PartialEq, Eq)]
pub enum HelperLine {
    /// Wants a secret (`echo` false: a password) or plain text; the prompt shown.
    Prompt { echo: bool, text: String },
    /// Something to show (an error or information from PAM).
    Message(String),
    Success,
    Failure,
    Other,
}

pub fn parse_line(line: &str) -> HelperLine {
    let line = line.trim_end_matches(['\n', '\r']);
    if let Some(t) = line.strip_prefix("PAM_PROMPT_ECHO_OFF ") {
        HelperLine::Prompt { echo: false, text: t.trim().to_string() }
    } else if let Some(t) = line.strip_prefix("PAM_PROMPT_ECHO_ON ") {
        HelperLine::Prompt { echo: true, text: t.trim().to_string() }
    } else if let Some(t) = line.strip_prefix("PAM_ERROR_MSG ").or_else(|| line.strip_prefix("PAM_TEXT_INFO ")) {
        HelperLine::Message(t.trim().to_string())
    } else if line == "SUCCESS" {
        HelperLine::Success
    } else if line == "FAILURE" {
        HelperLine::Failure
    } else {
        HelperLine::Other
    }
}

/// How the conversation went.
#[derive(Debug, PartialEq, Eq)]
pub enum Outcome {
    Success,
    /// Wrong password (or PAM said no), with what PAM said, if anything.
    Failure(String),
}

/// Talk the helper's protocol over `io`: our lines are the answers. The password goes to the
/// first password prompt only; a second one (PAM asking again) gets an empty line.
pub fn converse(reader: &mut dyn BufRead, writer: &mut dyn Write, password: &str) -> Result<Outcome, String> {
    let mut sent = false;
    let mut said = String::new();
    let mut line = String::new();
    loop {
        line.clear();
        let n = reader.read_line(&mut line).map_err(|e| format!("polkit helper: {e}"))?;
        if n == 0 {
            return Err("polkit's helper went away".into());
        }
        // The helper's side only (prompts, PAM's messages, the result): never our answers.
        info!("polkit helper: {}", line.trim_end());
        match parse_line(&line) {
            HelperLine::Prompt { echo: false, .. } if !sent => {
                writer.write_all(format!("{password}\n").as_bytes()).map_err(|e| e.to_string())?;
                writer.flush().map_err(|e| e.to_string())?;
                sent = true;
            }
            HelperLine::Prompt { .. } => {
                writer.write_all(b"\n").map_err(|e| e.to_string())?;
                writer.flush().map_err(|e| e.to_string())?;
            }
            HelperLine::Message(m) => said = m,
            HelperLine::Success => return Ok(Outcome::Success),
            HelperLine::Failure => return Ok(Outcome::Failure(said)),
            HelperLine::Other => {}
        }
    }
}

/// The user to authenticate as, from polkit's list of `(kind, details)` identities: you, if you
/// may; else the first user listed (often root); `users` maps uid → name (`/etc/passwd`).
pub fn pick_user(identities: &[(String, u32)], me: &str, users: &HashMap<u32, String>) -> Option<String> {
    let names: Vec<&String> = identities.iter().filter(|(kind, _)| kind == "unix-user").filter_map(|(_, uid)| users.get(uid)).collect();
    names.iter().find(|n| n.as_str() == me).or(names.first()).map(|n| n.to_string())
}

/// uid → name from `/etc/passwd`-style text.
pub fn passwd_users(text: &str) -> HashMap<u32, String> {
    text.lines()
        .filter_map(|l| {
            let f: Vec<&str> = l.split(':').collect();
            Some((f.get(2)?.parse().ok()?, f.first()?.to_string()))
        })
        .collect()
}

// ---- The agent ----

#[derive(Debug, Clone, Serialize)]
struct Ask {
    id: u64,
    message: String,
    action: String,
    user: String,
    /// What PAM said last time (a wrong password), for the dialog.
    error: Option<String>,
}

struct Pending {
    ask: Ask,
    cookie: String,
    tries: u32,
    tx: Option<tokio::sync::oneshot::Sender<Result<(), String>>>,
}

static PENDING: Mutex<Vec<Pending>> = Mutex::new(Vec::new());
static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

fn pending<T>(f: impl FnOnce(&mut Vec<Pending>) -> T) -> T {
    f(&mut PENDING.lock().unwrap_or_else(|e| e.into_inner()))
}

#[derive(Debug, zbus::DBusError)]
#[zbus(prefix = "org.freedesktop.PolicyKit1.Error")]
enum AgentError {
    #[zbus(error)]
    ZBus(zbus::Error),
    Failed(String),
    Cancelled(String),
}

struct Agent {
    app: AppHandle,
}

fn show_dialog(app: &AppHandle) {
    let handle = app.clone();
    let _ = app.run_on_main_thread(move || {
        let overlay = handle.state::<crate::overlay::Overlay>();
        overlay.open_auth(&handle);
    });
}

#[zbus::interface(name = "org.freedesktop.PolicyKit1.AuthenticationAgent")]
impl Agent {
    async fn begin_authentication(
        &self,
        action_id: String,
        message: String,
        _icon_name: String,
        _details: HashMap<String, String>,
        cookie: String,
        identities: Vec<(String, HashMap<String, OwnedValue>)>,
    ) -> Result<(), AgentError> {
        let ids: Vec<(String, u32)> = identities
            .iter()
            .map(|(kind, d)| (kind.clone(), d.get("uid").and_then(|v| u32::try_from(v.clone()).ok()).unwrap_or(u32::MAX)))
            .collect();
        let users = passwd_users(&std::fs::read_to_string("/etc/passwd").unwrap_or_default());
        let me = std::env::var("USER").unwrap_or_default();
        let Some(user) = pick_user(&ids, &me, &users) else {
            return Err(AgentError::Failed("no user to authenticate as".into()));
        };
        let id = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let (tx, rx) = tokio::sync::oneshot::channel();
        let ask = Ask { id, message, action: action_id, user, error: None };
        info!("polkit: {} asks for {}'s password ({})", ask.action, ask.user, ask.message);
        pending(|p| p.push(Pending { ask: ask.clone(), cookie, tries: 0, tx: Some(tx) }));
        let _ = self.app.emit("kompact-auth", ());
        show_dialog(&self.app);
        // At most 5 minutes; polkit cancels sooner if the app gives up.
        let result = tokio::time::timeout(Duration::from_secs(300), rx).await;
        pending(|p| p.retain(|x| x.ask.id != id));
        let _ = self.app.emit("kompact-auth", ());
        match result {
            Ok(Ok(Ok(()))) => Ok(()),
            Ok(Ok(Err(e))) => Err(AgentError::Failed(e)),
            _ => Err(AgentError::Cancelled("cancelled".into())),
        }
    }

    async fn cancel_authentication(&self, cookie: String) {
        info!("polkit: request cancelled by polkit");
        let tx = pending(|p| p.iter_mut().find(|x| x.cookie == cookie).and_then(|x| x.tx.take()));
        if let Some(tx) = tx {
            let _ = tx.send(Err("cancelled".into()));
        }
    }
}

/// Check `password` for the request's user with polkit's helper.
fn authenticate(user: &str, cookie: &str, password: &str) -> Result<Outcome, String> {
    match std::os::unix::net::UnixStream::connect(HELPER_SOCKET) {
        Ok(stream) => {
            stream.set_read_timeout(Some(Duration::from_secs(30))).map_err(|e| e.to_string())?;
            let mut writer = stream.try_clone().map_err(|e| e.to_string())?;
            writer.write_all(format!("{user}\n{cookie}\n").as_bytes()).map_err(|e| format!("polkit helper: {e}"))?;
            converse(&mut BufReader::new(stream), &mut writer, password)
        }
        Err(e) => {
            // Older polkit: the setuid helper.
            info!("polkit: no helper socket ({e}), running {HELPER}");
            let mut child = std::process::Command::new(HELPER)
                .arg(user)
                .stdin(std::process::Stdio::piped())
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::null())
                .spawn()
                .map_err(|e| format!("{HELPER}: {e}"))?;
            let mut stdin = child.stdin.take().ok_or("no helper stdin")?;
            stdin.write_all(format!("{cookie}\n").as_bytes()).map_err(|e| e.to_string())?;
            let stdout = child.stdout.take().ok_or("no helper stdout")?;
            let out = converse(&mut BufReader::new(stdout), &mut stdin, password);
            let _ = child.wait();
            out
        }
    }
}

/// The dialog's request, if any (for the overlay page).
#[tauri::command]
pub fn polkit_request() -> Option<serde_json::Value> {
    pending(|p| p.first().map(|x| json!(x.ask)))
}

/// The password typed in the dialog. `Ok(true)`: done; `Ok(false)`: wrong, try again (the
/// request now carries the error).
#[tauri::command]
pub async fn polkit_answer(app: AppHandle, id: u64, password: String) -> Result<bool, String> {
    let Some((user, cookie)) = pending(|p| p.iter().find(|x| x.ask.id == id).map(|x| (x.ask.user.clone(), x.cookie.clone()))) else {
        return Err("that request has gone".into());
    };
    let outcome = tauri::async_runtime::spawn_blocking(move || authenticate(&user, &cookie, &password)).await.map_err(|e| e.to_string())?;
    match outcome {
        Ok(Outcome::Success) => {
            info!("polkit: authenticated");
            if let Some(tx) = pending(|p| p.iter_mut().find(|x| x.ask.id == id).and_then(|x| x.tx.take())) {
                let _ = tx.send(Ok(()));
            }
            Ok(true)
        }
        Ok(Outcome::Failure(said)) => {
            let tries = pending(|p| {
                let x = p.iter_mut().find(|x| x.ask.id == id)?;
                x.tries += 1;
                x.ask.error = Some(if said.is_empty() { "Sorry, that didn't work. Try again.".into() } else { said.clone() });
                Some(x.tries)
            })
            .unwrap_or(MAX_TRIES);
            info!("polkit: wrong password ({tries} of {MAX_TRIES})");
            if tries >= MAX_TRIES {
                if let Some(tx) = pending(|p| p.iter_mut().find(|x| x.ask.id == id).and_then(|x| x.tx.take())) {
                    let _ = tx.send(Err("too many wrong passwords".into()));
                }
                return Ok(true);
            }
            let _ = app.emit("kompact-auth", ());
            Ok(false)
        }
        Err(e) => {
            warn!("polkit: {e}");
            if let Some(tx) = pending(|p| p.iter_mut().find(|x| x.ask.id == id).and_then(|x| x.tx.take())) {
                let _ = tx.send(Err(e.clone()));
            }
            Err(e)
        }
    }
}

#[tauri::command]
pub fn polkit_cancel(id: u64) {
    info!("polkit: cancelled in the dialog");
    if let Some(tx) = pending(|p| p.iter_mut().find(|x| x.ask.id == id).and_then(|x| x.tx.take())) {
        let _ = tx.send(Err("cancelled".into()));
    }
}

/// Serve the agent and register it for this login session.
pub fn start(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        let conn = match zbus::Connection::system().await {
            Ok(c) => c,
            Err(e) => return warn!("polkit: system bus: {e}; no authentication agent"),
        };
        if let Err(e) = conn.object_server().at(AGENT_PATH, Agent { app: app.clone() }).await {
            return warn!("polkit: can't serve the agent: {e}");
        }
        let Some(session) = std::env::var("XDG_SESSION_ID").ok().filter(|s| !s.is_empty()) else {
            return warn!("polkit: no XDG_SESSION_ID; no authentication agent");
        };
        let authority = match zbus::Proxy::new(&conn, "org.freedesktop.PolicyKit1", "/org/freedesktop/PolicyKit1/Authority", "org.freedesktop.PolicyKit1.Authority").await {
            Ok(p) => p,
            Err(e) => return warn!("polkit: {e}"),
        };
        let mut details: HashMap<&str, ZValue> = HashMap::new();
        details.insert("session-id", ZValue::from(session.clone()));
        let subject = ("unix-session", details);
        let locale = std::env::var("LANG").unwrap_or_else(|_| "en_US.UTF-8".into());
        // `(sa{sv})ss`: the agent's path goes as a plain string, not an object path.
        match authority.call::<_, _, ()>("RegisterAuthenticationAgent", &(subject, locale, AGENT_PATH)).await {
            Ok(()) => info!("polkit: authentication agent registered for session {session}"),
            Err(e) => warn!("polkit: can't register the agent for session {session}: {e}"),
        }
        // Keep the connection (and with it the registration) for the shell's life.
        std::future::pending::<()>().await;
        drop(conn);
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn helper_lines() {
        assert_eq!(parse_line("PAM_PROMPT_ECHO_OFF Password: \n"), HelperLine::Prompt { echo: false, text: "Password:".into() });
        assert_eq!(parse_line("PAM_PROMPT_ECHO_ON Login:"), HelperLine::Prompt { echo: true, text: "Login:".into() });
        assert_eq!(parse_line("PAM_ERROR_MSG Authentication failure"), HelperLine::Message("Authentication failure".into()));
        assert_eq!(parse_line("SUCCESS\n"), HelperLine::Success);
        assert_eq!(parse_line("FAILURE"), HelperLine::Failure);
        assert_eq!(parse_line("noise"), HelperLine::Other);
    }

    #[test]
    fn a_conversation() {
        let mut out = Vec::new();
        let mut helper = std::io::Cursor::new(b"PAM_TEXT_INFO hi\nPAM_PROMPT_ECHO_OFF Password: \nSUCCESS\n".to_vec());
        assert_eq!(converse(&mut helper, &mut out, "s3cret"), Ok(Outcome::Success));
        assert_eq!(out, b"s3cret\n", "the password once, nothing else");
        let mut out = Vec::new();
        let mut helper = std::io::Cursor::new(b"PAM_PROMPT_ECHO_OFF Password: \nPAM_ERROR_MSG Sorry\nFAILURE\n".to_vec());
        assert_eq!(converse(&mut helper, &mut out, "wrong"), Ok(Outcome::Failure("Sorry".into())));
        // A second prompt gets an empty answer, not the password again.
        let mut out = Vec::new();
        let mut helper = std::io::Cursor::new(b"PAM_PROMPT_ECHO_OFF Password: \nPAM_PROMPT_ECHO_OFF Again: \nFAILURE\n".to_vec());
        assert_eq!(converse(&mut helper, &mut out, "pw"), Ok(Outcome::Failure(String::new())));
        assert_eq!(out, b"pw\n\n");
        // The helper hung up.
        let mut helper = std::io::Cursor::new(Vec::new());
        assert!(converse(&mut helper, &mut Vec::new(), "pw").is_err());
    }

    #[test]
    fn who_to_ask_for() {
        let users = passwd_users("root:x:0:0:root:/root:/bin/bash\nsam:x:1000:1000::/home/sam:/bin/bash\nbad line\n");
        assert_eq!(users.get(&1000).map(String::as_str), Some("sam"));
        let ids = vec![("unix-user".to_string(), 0), ("unix-user".to_string(), 1000), ("unix-group".to_string(), 27)];
        assert_eq!(pick_user(&ids, "sam", &users).as_deref(), Some("sam"), "you, when you're allowed");
        assert_eq!(pick_user(&ids, "someone", &users).as_deref(), Some("root"), "else the first user");
        assert_eq!(pick_user(&[("unix-group".into(), 27)], "sam", &users), None);
    }
}
