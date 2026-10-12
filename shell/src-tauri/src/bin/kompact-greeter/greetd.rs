//! 14c: greetd's IPC (`$GREETD_SOCK`, a Unix socket): each message is a native-endian u32 length
//! and that much JSON. The greeter asks to `create_session` for a user, answers the PAM
//! questions greetd passes on (`auth_message` → `post_auth_message_response`) until `success`,
//! then `start_session` with the session's command; greetd starts it once the greeter exits.
//! A wrong password is an `error` with `error_type: auth_error`, and the half-made session must
//! be cancelled (`cancel_session`) before the next try. Encoding and the conversation's next step
//! are pure and tested; `login` does the socket work.

use std::io::{Read, Write};
use std::os::unix::net::UnixStream;

use serde_json::{json, Value};

pub fn encode(msg: &Value) -> Vec<u8> {
    let body = msg.to_string().into_bytes();
    let mut out = (body.len() as u32).to_ne_bytes().to_vec();
    out.extend(body);
    out
}

/// Longest reply taken (greetd's are tiny).
const MAX_REPLY: u32 = 64 * 1024;

fn read_msg(s: &mut UnixStream) -> Result<Value, String> {
    let mut len = [0u8; 4];
    s.read_exact(&mut len).map_err(|e| format!("greetd: {e}"))?;
    let len = u32::from_ne_bytes(len);
    if len > MAX_REPLY {
        return Err(format!("greetd: a {len}-byte reply"));
    }
    let mut body = vec![0u8; len as usize];
    s.read_exact(&mut body).map_err(|e| format!("greetd: {e}"))?;
    serde_json::from_slice(&body).map_err(|e| format!("greetd: bad reply: {e}"))
}

fn ask(s: &mut UnixStream, msg: &Value) -> Result<Value, String> {
    s.write_all(&encode(msg)).map_err(|e| format!("greetd: {e}"))?;
    read_msg(s)
}

/// What to do with a reply during the login.
#[derive(Debug, PartialEq, Eq)]
pub enum Step {
    /// Answer this hidden question with the password (only once).
    Secret,
    /// A question to show (`visible`): kompact's page only asks for passwords, so this login fails
    /// with greetd's text (a one-time code, say, would need its own field).
    Visible(String),
    /// Info or error text from PAM: show it and answer nothing.
    Note(String),
    /// Signed in: start the session.
    Done,
    /// The password was wrong.
    Wrong(String),
    /// Something else went wrong.
    Failed(String),
}

pub fn step(reply: &Value) -> Step {
    let text = |k: &str| reply[k].as_str().unwrap_or("").to_string();
    match reply["type"].as_str() {
        Some("success") => Step::Done,
        Some("auth_message") => match reply["auth_message_type"].as_str() {
            Some("secret") => Step::Secret,
            Some("visible") => Step::Visible(text("auth_message")),
            _ => Step::Note(text("auth_message")),
        },
        Some("error") if reply["error_type"] == "auth_error" => Step::Wrong(text("description")),
        Some("error") => Step::Failed(text("description")),
        other => Step::Failed(format!("greetd said {other:?}")),
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum Outcome {
    /// greetd will start the session when the greeter exits.
    Started,
    Wrong(String),
    Failed(String),
}

/// Log `user` in with `password` and start `cmd` with `env`. Blocks: call it off the GTK thread.
pub fn login(sock: &str, user: &str, password: &str, cmd: &[String], env: &[String], note: &dyn Fn(&str)) -> Outcome {
    let mut s = match UnixStream::connect(sock) {
        Ok(s) => s,
        Err(e) => return Outcome::Failed(format!("can't reach greetd at {sock}: {e}")),
    };
    let cancel = |s: &mut UnixStream| {
        let _ = ask(s, &json!({"type": "cancel_session"}));
    };
    let mut reply = match ask(&mut s, &json!({"type": "create_session", "username": user})) {
        Ok(r) => r,
        Err(e) => return Outcome::Failed(e),
    };
    let mut answered = false;
    // A handful of questions at most (password, maybe an expiry note).
    for _ in 0..8 {
        let answer = match step(&reply) {
            Step::Secret if !answered => {
                answered = true;
                json!({"type": "post_auth_message_response", "response": password})
            }
            Step::Secret => {
                // Asked again: PAM wants a second secret (a new password after expiry).
                cancel(&mut s);
                return Outcome::Failed("The password must be changed first: log in on a text console once.".into());
            }
            Step::Visible(q) => {
                cancel(&mut s);
                return Outcome::Failed(if q.is_empty() { "The login asked for more than a password.".into() } else { q });
            }
            Step::Note(t) => {
                note(&t);
                json!({"type": "post_auth_message_response", "response": null})
            }
            Step::Done => {
                return match ask(&mut s, &json!({"type": "start_session", "cmd": cmd, "env": env})).map(|r| step(&r)) {
                    Ok(Step::Done) => Outcome::Started,
                    Ok(Step::Failed(e) | Step::Wrong(e)) => {
                        cancel(&mut s);
                        Outcome::Failed(format!("Couldn't start the session: {e}"))
                    }
                    Ok(other) => {
                        cancel(&mut s);
                        Outcome::Failed(format!("Couldn't start the session ({other:?})"))
                    }
                    Err(e) => Outcome::Failed(e),
                };
            }
            Step::Wrong(e) => {
                cancel(&mut s);
                return Outcome::Wrong(e);
            }
            Step::Failed(e) => {
                cancel(&mut s);
                return Outcome::Failed(e);
            }
        };
        reply = match ask(&mut s, &answer) {
            Ok(r) => r,
            Err(e) => return Outcome::Failed(e),
        };
    }
    cancel(&mut s);
    Outcome::Failed("The login asked too many questions.".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wire_format() {
        let b = encode(&json!({"type": "cancel_session"}));
        let body = br#"{"type":"cancel_session"}"#;
        assert_eq!(&b[..4], &(body.len() as u32).to_ne_bytes());
        assert_eq!(&b[4..], body);
    }

    #[test]
    fn steps() {
        assert_eq!(step(&json!({"type": "success"})), Step::Done);
        assert_eq!(step(&json!({"type": "auth_message", "auth_message_type": "secret", "auth_message": "Password: "})), Step::Secret);
        assert_eq!(step(&json!({"type": "auth_message", "auth_message_type": "visible", "auth_message": "Code: "})), Step::Visible("Code: ".into()));
        assert_eq!(step(&json!({"type": "auth_message", "auth_message_type": "info", "auth_message": "Hi"})), Step::Note("Hi".into()));
        assert_eq!(step(&json!({"type": "error", "error_type": "auth_error", "description": "pam_authenticate: AUTH_ERR"})), Step::Wrong("pam_authenticate: AUTH_ERR".into()));
        assert_eq!(step(&json!({"type": "error", "error_type": "error", "description": "no such user"})), Step::Failed("no such user".into()));
        assert!(matches!(step(&json!({"type": "huh"})), Step::Failed(_)));
    }

    /// Against `kompact/tools/fake-greetd.py` (no screen): a wrong then the right test password.
    /// `cargo test --bin kompact-greeter -- --ignored`.
    #[test]
    #[ignore]
    fn against_the_stand_in() {
        let dir = std::env::temp_dir().join(format!("kompact-greetd-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let sock = dir.join("greetd.sock");
        let script = concat!(env!("CARGO_MANIFEST_DIR"), "/../../kompact/tools/fake-greetd.py");
        let mut fake = std::process::Command::new("python3").arg(script).arg(&sock).spawn().unwrap();
        for _ in 0..50 {
            if sock.exists() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        let s = sock.to_str().unwrap();
        let cmd = ["kompact-session".to_string()];
        let env = ["XDG_SESSION_TYPE=wayland".to_string()];
        assert!(matches!(login(s, "sam", "nope", &cmd, &env, &|_| {}), Outcome::Wrong(_)));
        assert_eq!(login(s, "sam", "kompact", &cmd, &env, &|_| {}), Outcome::Started);
        assert!(fake.wait().unwrap().success(), "the stand-in exits after start_session");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
