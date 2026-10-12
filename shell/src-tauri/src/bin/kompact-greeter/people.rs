//! 14c: who can log in and into what, worked out from plain files (pure, tested): the people
//! (real accounts in /etc/passwd: uid 1000–59999 and a login shell), the sessions
//! (`/usr/share/wayland-sessions/*.desktop`), what the greeter remembers (the last user, each
//! user's last session), and each user's published look (their background and message, which a
//! session copies to a shared folder, since homes aren't readable by the greeter).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Person {
    pub login: String,
    pub name: String,
    pub uid: u32,
}

/// Shells that mean "can't log in".
fn no_login(shell: &str) -> bool {
    shell.is_empty() || shell.ends_with("/nologin") || shell.ends_with("/false") || shell.ends_with("/sync")
}

/// The real accounts in `passwd`, by name: uid 1000–59999 (Debian's range for people) and a
/// shell listed in `shells` (/etc/shells), not nologin/false.
pub fn people(passwd: &str, shells: &[&str]) -> Vec<Person> {
    let mut out: Vec<Person> = passwd
        .lines()
        .filter_map(|l| {
            let f: Vec<&str> = l.split(':').collect();
            let [login, _, uid, _, gecos, _, shell] = f[..] else { return None };
            let uid: u32 = uid.parse().ok()?;
            let shell_ok = !no_login(shell) && (shells.is_empty() || shells.contains(&shell));
            ((1000..60000).contains(&uid) && shell_ok && !login.is_empty()).then(|| Person {
                login: login.to_string(),
                name: crate::look::display_name(gecos, login),
                uid,
            })
        })
        .collect();
    out.sort_by_key(|p| p.name.to_lowercase());
    out
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Session {
    /// The desktop file's name without `.desktop` (`kompact`, `ubuntu`).
    pub id: String,
    pub name: String,
    pub cmd: Vec<String>,
    /// `DesktopNames=` as `XDG_CURRENT_DESKTOP` wants it (`ubuntu:GNOME`).
    pub desktop: String,
}

/// One session file; `None` if it's hidden or has no name or command.
pub fn session(id: &str, text: &str) -> Option<Session> {
    let mut in_entry = false;
    let mut keys: BTreeMap<&str, &str> = BTreeMap::new();
    for line in text.lines().map(str::trim) {
        if line.starts_with('[') {
            in_entry = line == "[Desktop Entry]";
        } else if in_entry {
            if let Some((k, v)) = line.split_once('=') {
                if !k.contains('[') {
                    keys.entry(k.trim()).or_insert(v.trim());
                }
            }
        }
    }
    let flag = |k: &str| keys.get(k).is_some_and(|v| v.eq_ignore_ascii_case("true"));
    if flag("Hidden") || flag("NoDisplay") {
        return None;
    }
    let name = keys.get("Name").filter(|n| !n.is_empty())?.to_string();
    let cmd = crate::apps::exec_argv(keys.get("Exec")?)?;
    let desktop = keys.get("DesktopNames").map(|d| d.split(';').filter(|s| !s.is_empty()).collect::<Vec<_>>().join(":")).unwrap_or_default();
    Some(Session { id: id.to_string(), name, cmd, desktop })
}

/// The sessions, kompact first, then by name.
pub fn sessions(files: &[(String, String)]) -> Vec<Session> {
    let mut out: Vec<Session> = files.iter().filter_map(|(id, t)| session(id, t)).collect();
    out.sort_by_key(|s| (s.id != "kompact", s.name.to_lowercase()));
    out.dedup_by(|a, b| a.id == b.id);
    out
}

/// The session's environment for greetd's `start_session`.
pub fn session_env(s: &Session) -> Vec<String> {
    let mut env = vec!["XDG_SESSION_TYPE=wayland".to_string(), format!("XDG_SESSION_DESKTOP={}", s.id)];
    if !s.desktop.is_empty() {
        env.push(format!("XDG_CURRENT_DESKTOP={}", s.desktop));
    }
    env
}

/// What the greeter remembers.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Memory {
    pub last: Option<String>,
    /// User → their last session id.
    pub sessions: BTreeMap<String, String>,
}

fn word(s: &str) -> bool {
    !s.is_empty() && s.len() <= 64 && s.bytes().all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
}

impl Memory {
    pub fn parse(text: &str) -> Memory {
        let mut m = Memory::default();
        for line in text.lines().map(str::trim).filter(|l| !l.starts_with('#')) {
            let Some((k, v)) = line.split_once('=') else { continue };
            let (k, v) = (k.trim(), v.trim());
            if k == "last" && word(v) {
                m.last = Some(v.to_string());
            } else if let Some(user) = k.strip_prefix("session.") {
                if word(user) && word(v) {
                    m.sessions.insert(user.to_string(), v.to_string());
                }
            }
        }
        m
    }

    pub fn to_text(&self) -> String {
        let mut s = String::from("# kompact login screen: the last user and each user's last session\n");
        if let Some(l) = &self.last {
            s.push_str(&format!("last={l}\n"));
        }
        for (u, id) in &self.sessions {
            s.push_str(&format!("session.{u}={id}\n"));
        }
        s
    }

    /// After a login: remember both.
    pub fn logged_in(&mut self, user: &str, session: &str) {
        if word(user) {
            self.last = Some(user.to_string());
            if word(session) {
                self.sessions.insert(user.to_string(), session.to_string());
            }
        }
    }
}

/// Who to pick first: the last user if they're still there, else the first.
pub fn first_person(people: &[Person], m: &Memory) -> usize {
    m.last.as_ref().and_then(|l| people.iter().position(|p| &p.login == l)).unwrap_or(0)
}

/// Which session for `user`: theirs from last time, else kompact, else the first.
pub fn session_for<'a>(sessions: &'a [Session], m: &Memory, user: &str) -> Option<&'a Session> {
    m.sessions
        .get(user)
        .and_then(|id| sessions.iter().find(|s| &s.id == id))
        .or_else(|| sessions.iter().find(|s| s.id == "kompact"))
        .or_else(|| sessions.first())
}

/// The folder a session publishes each user's look to (14d makes it): `<user>.conf`
/// (`login_prefs` lines, `image=` a copy in this folder).
pub const SHARED_DIR: &str = "/var/lib/kompact/login";

pub fn shared_conf(dir: &Path, login: &str) -> Option<PathBuf> {
    word(login).then(|| dir.join(format!("{login}.conf")))
}

/// A published look: its picture (only one inside `dir`, so a conf can't point the greeter at
/// another file) and whether to blur it.
pub fn published_look(prefs: &crate::login_prefs::LoginPrefs, dir: &Path) -> (Option<PathBuf>, bool) {
    let image = prefs.image.clone().filter(|p| p.parent() == Some(dir));
    (image, prefs.background == crate::login_prefs::Background::DesktopBlur)
}

#[cfg(test)]
mod tests {
    use super::*;

    const PASSWD: &str = "root:x:0:0:root:/root:/bin/bash\n\
        daemon:x:1:1:daemon:/usr/sbin:/usr/sbin/nologin\n\
        sam:x:1000:1000:Sam Lee,,,:/home/sam:/bin/bash\n\
        ana:x:1001:1001:Ana Souza:/home/ana:/usr/bin/zsh\n\
        build:x:1002:1002::/home/build:/usr/sbin/nologin\n\
        nobody:x:65534:65534:nobody:/nonexistent:/usr/sbin/nologin\n\
        weird:x:1003\n";

    #[test]
    fn who_can_log_in() {
        let p = people(PASSWD, &["/bin/bash", "/usr/bin/zsh"]);
        assert_eq!(p.iter().map(|p| p.login.as_str()).collect::<Vec<_>>(), ["ana", "sam"]);
        assert_eq!(p[1].name, "Sam Lee");
        // A shell not in /etc/shells: not a login account.
        assert_eq!(people(PASSWD, &["/bin/bash"]).len(), 1);
    }

    #[test]
    fn sessions_found() {
        let files = vec![
            ("ubuntu".to_string(), "[Desktop Entry]\nName=Ubuntu\nExec=/usr/bin/gnome-session --session=ubuntu\nDesktopNames=ubuntu;GNOME\n".to_string()),
            ("kompact".to_string(), "[Desktop Entry]\nName=kompact\nExec=/usr/local/bin/kompact-session\nDesktopNames=kompact\n".to_string()),
            ("old".to_string(), "[Desktop Entry]\nName=Old\nExec=old\nHidden=true\n".to_string()),
            ("broken".to_string(), "[Desktop Entry]\nName=Broken\n".to_string()),
        ];
        let s = sessions(&files);
        assert_eq!(s.iter().map(|s| s.id.as_str()).collect::<Vec<_>>(), ["kompact", "ubuntu"]);
        assert_eq!(s[1].cmd, ["/usr/bin/gnome-session", "--session=ubuntu"]);
        assert_eq!(s[1].desktop, "ubuntu:GNOME");
        assert_eq!(session_env(&s[1]), ["XDG_SESSION_TYPE=wayland", "XDG_SESSION_DESKTOP=ubuntu", "XDG_CURRENT_DESKTOP=ubuntu:GNOME"]);
    }

    #[test]
    fn memory() {
        let people = people(PASSWD, &[]);
        let files = vec![("ubuntu".to_string(), "[Desktop Entry]\nName=Ubuntu\nExec=gnome-session\n".to_string())];
        let sessions = sessions(&files);
        let mut m = Memory::default();
        assert_eq!(first_person(&people, &m), 0);
        assert_eq!(session_for(&sessions, &m, "sam").map(|s| s.id.as_str()), Some("ubuntu"));
        m.logged_in("sam", "ubuntu");
        assert_eq!(Memory::parse(&m.to_text()), m);
        assert_eq!(people[first_person(&people, &m)].login, "sam");
        // Junk is ignored.
        let j = Memory::parse("last=../../etc\nsession.a b=x\nsession.ana=kompact\n");
        assert_eq!(j.last, None);
        assert_eq!(j.sessions.len(), 1);
        // A remembered session that's gone: kompact or the first.
        m.logged_in("ana", "gone");
        assert_eq!(session_for(&sessions, &m, "ana").map(|s| s.id.as_str()), Some("ubuntu"));
    }

    #[test]
    fn published() {
        let dir = Path::new("/var/lib/kompact/login");
        assert_eq!(shared_conf(dir, "sam"), Some(dir.join("sam.conf")));
        assert_eq!(shared_conf(dir, "../x"), None);
        let mut p = crate::login_prefs::LoginPrefs::parse("background=desktop-blur\nimage=/var/lib/kompact/login/sam.jpg\nmessage=hi\n");
        assert_eq!(published_look(&p, dir), (Some(dir.join("sam.jpg")), true));
        p.image = Some("/etc/shadow".into());
        assert_eq!(published_look(&p, dir).0, None, "only pictures in the shared folder");
    }
}
