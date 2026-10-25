//! 21a: Siva does real work: files and commands. Files are written, read and listed directly
//! (relative names are in your home folder); commands run in **Siva's terminal**, a window of
//! your terminal app running `RUNNER`: it waits on a FIFO for a command number, shows the command
//! and runs it under util-linux `script`, so you see it live, can answer its questions or press
//! Ctrl+C, and Siva gets its output and exit code from the log. One command at a time.
//!
//! Decided 2026-10-25 (the user): commands run at once; only drastic ones ask first
//! (`command_asks`: sudo, deleting, disks, power, force pushes, secrets, piping downloads into a
//! shell). Files outside your home folder or hidden ones ask too (`file_asks`).
//!
//! The path, command and output handling is pure and tested; the rest starts the terminal.

use std::path::{Component, Path, PathBuf};
use std::time::{Duration, Instant};

use tracing::info;

/// At most this much of a file is read, and a reply shows at most `SHOWN` characters of it.
pub const READ_MAX: usize = 64 * 1024;
pub const SHOWN: usize = 1500;
/// A file bigger than this isn't overwritten without asking (its copy for Undo is kept in memory).
pub const UNDO_MAX: u64 = 2 * 1024 * 1024;
/// How long Siva waits for a command before saying it's still running.
pub const WAIT: Duration = Duration::from_secs(90);
/// The end of a command's output kept for the reply.
pub const OUTPUT_KEPT: usize = 1500;

// ---- paths (pure) ----

/// `raw` as a path: `~` and relative names in `home`, `.` and `..` worked out (never above `/`).
pub fn resolve(raw: &str, home: &Path) -> Result<PathBuf, String> {
    let raw = raw.trim().trim_matches(['"', '\'', '`']);
    if raw.is_empty() || raw.chars().any(char::is_control) {
        return Err("that isn't a file name".into());
    }
    let joined = if raw == "~" {
        home.to_path_buf()
    } else if let Some(rest) = raw.strip_prefix("~/") {
        home.join(rest)
    } else if raw.starts_with('/') {
        PathBuf::from(raw)
    } else {
        home.join(raw)
    };
    let mut out = PathBuf::from("/");
    for c in joined.components() {
        match c {
            Component::Normal(n) => out.push(n),
            Component::ParentDir => {
                out.pop();
            }
            _ => {}
        }
    }
    Ok(out)
}

/// `path` as you'd say it: `~/notes/a.txt` in your home folder.
pub fn shown(path: &Path, home: &Path) -> String {
    match path.strip_prefix(home) {
        Ok(rest) if rest.as_os_str().is_empty() => "your home folder".into(),
        Ok(rest) => format!("~/{}", rest.display()),
        Err(_) => path.display().to_string(),
    }
}

/// Why writing (or reading) `path` asks first, or `None`: outside your home folder, or a hidden
/// (settings) file or folder, where a change can break things or a read can find secrets.
pub fn file_asks(path: &Path, home: &Path) -> Option<&'static str> {
    let Ok(rest) = path.strip_prefix(home) else {
        return Some("That's outside your home folder.");
    };
    rest.components()
        .any(|c| matches!(c, Component::Normal(n) if n.to_string_lossy().starts_with('.')))
        .then_some("That's a hidden settings file.")
}

// ---- commands (pure) ----

/// Programs that ask first, whatever they're given: more power, deleting, disks, the session.
const ASKS: &[(&str, &str)] = &[
    ("sudo", "It runs as the administrator."),
    ("su", "It runs as the administrator."),
    ("pkexec", "It runs as the administrator."),
    ("doas", "It runs as the administrator."),
    ("rm", "It deletes files for good."),
    ("rmdir", "It deletes folders."),
    ("shred", "It destroys files."),
    ("unlink", "It deletes a file."),
    ("dd", "It writes straight to disks."),
    ("mkfs", "It erases a disk."),
    ("fdisk", "It changes disk partitions."),
    ("parted", "It changes disk partitions."),
    ("wipefs", "It erases a disk."),
    ("shutdown", "It turns the computer off."),
    ("reboot", "It restarts the computer."),
    ("poweroff", "It turns the computer off."),
    ("halt", "It turns the computer off."),
    ("killall", "It stops programs."),
    ("pkill", "It stops programs."),
    ("crontab", "It changes what runs on a schedule."),
    ("passwd", "It changes a password."),
];

/// Pieces of a command line that ask first, wherever they are.
const ASKS_ANYWHERE: &[(&str, &str)] = &[
    ("-delete", "It deletes files for good."),
    ("--force", "It forces a change that may lose work."),
    ("push -f", "It forces a change that may lose work."),
    ("reset --hard", "It throws away changes."),
    ("clean -f", "It deletes files for good."),
    ("/dev/sd", "It writes straight to disks."),
    ("/dev/nvme", "It writes straight to disks."),
    (".ssh", "It touches your keys and secrets."),
    (".gnupg", "It touches your keys and secrets."),
    ("id_rsa", "It touches your keys and secrets."),
    ("id_ed25519", "It touches your keys and secrets."),
    ("keyring", "It touches your keys and secrets."),
    ("/etc/shadow", "It touches your keys and secrets."),
    ("chmod -r", "It changes the permissions of many files."),
    ("chown", "It changes who owns files."),
    ("systemctl poweroff", "It turns the computer off."),
    ("systemctl reboot", "It restarts the computer."),
    ("systemctl suspend", "It puts the computer to sleep."),
    ("loginctl terminate", "It ends your session."),
    (":(){", "It would overload the computer."),
];

/// The program each part of a command line runs: split at `;`, `&&`, `||`, `|`, `&`, `$(`,
/// backquotes and line ends, past `VAR=value`, `env`, `command`, `exec`, `nohup`, `time`,
/// `xargs` and `nice`; the folder dropped (`/usr/bin/rm` is `rm`).
pub fn programs(cmd: &str) -> Vec<String> {
    let mut parts = vec![String::new()];
    let mut chars = cmd.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            ';' | '|' | '&' | '`' | '\n' | '(' | ')' | '{' | '}' => parts.push(String::new()),
            '$' if chars.peek() == Some(&'(') => parts.push(String::new()),
            c => parts.last_mut().unwrap().push(c),
        }
    }
    let skip = ["env", "command", "exec", "nohup", "time", "xargs", "nice", "builtin", "then", "do", "else", "if", "while", "!"];
    parts
        .iter()
        .filter_map(|p| {
            p.split_whitespace()
                .map(|w| w.trim_matches(['"', '\'']))
                .find(|w| !w.is_empty() && !w.contains('=') && !skip.contains(w) && !w.starts_with('-') && !w.chars().all(|c| c.is_ascii_digit()))
                .map(|w| w.rsplit('/').next().unwrap_or(w).to_lowercase())
        })
        .collect()
}

/// Why `cmd` asks first, or `None` (it runs at once).
pub fn command_asks(cmd: &str) -> Option<&'static str> {
    for p in programs(cmd) {
        let p = p.strip_prefix("mkfs.").map(|_| "mkfs").unwrap_or(&p);
        if let Some((_, why)) = ASKS.iter().find(|(name, _)| *name == p) {
            return Some(why);
        }
    }
    let lower = cmd.to_lowercase();
    if let Some((_, why)) = ASKS_ANYWHERE.iter().find(|(piece, _)| lower.contains(piece)) {
        return Some(why);
    }
    // A download straight into a shell.
    let progs = programs(cmd);
    if progs.iter().any(|p| p == "curl" || p == "wget") && progs.iter().any(|p| ["sh", "bash", "zsh", "python", "python3"].contains(&p.as_str())) && cmd.contains('|') {
        return Some("It runs something straight from the internet.");
    }
    // Overwriting a file with `>` outside your home folder (`> /etc/…`), not `2>/dev/null`.
    for (i, _) in cmd.match_indices('>') {
        let target = cmd[i + 1..].trim_start_matches('>').trim_start();
        if target.starts_with('/') && !target.starts_with("/dev/null") && !target.starts_with("/tmp") {
            return Some("It writes outside your home folder.");
        }
    }
    None
}

// ---- the output (pure) ----

/// `script`'s log as text: its first and last lines gone, colours and other escapes taken out,
/// `\r` and backspaces worked out.
pub fn clean_output(raw: &[u8]) -> String {
    let text = String::from_utf8_lossy(raw);
    let mut lines: Vec<&str> = text.split('\n').collect();
    if lines.first().is_some_and(|l| l.starts_with("Script started")) {
        lines.remove(0);
    }
    while lines.last().is_some_and(|l| l.trim().is_empty() || l.starts_with("Script done")) {
        lines.pop();
    }
    let mut out = Vec::new();
    for line in lines {
        let mut plain = String::new();
        let mut chars = line.chars().peekable();
        while let Some(c) = chars.next() {
            match c {
                '\u{1b}' => match chars.next() {
                    // CSI: up to a letter.
                    Some('[') => {
                        for d in chars.by_ref() {
                            if d.is_ascii_alphabetic() || d == '~' {
                                break;
                            }
                        }
                    }
                    // OSC: up to BEL or ESC \.
                    Some(']') => {
                        while let Some(d) = chars.next() {
                            if d == '\u{7}' || (d == '\u{1b}' && chars.next_if_eq(&'\\').is_some()) {
                                break;
                            }
                        }
                    }
                    _ => {}
                },
                '\r' => {
                    // A lone \r starts the line again (progress bars); "\r\n" just ends it.
                    if chars.peek().is_some() {
                        plain.clear();
                    }
                }
                '\u{8}' => {
                    plain.pop();
                }
                c if c.is_control() && c != '\t' => {}
                c => plain.push(c),
            }
        }
        out.push(plain.trim_end().to_string());
    }
    out.join("\n").trim_matches('\n').to_string()
}

/// The last `max` characters of `text`, from a line start, with "…" when cut.
pub fn tail(text: &str, max: usize) -> String {
    let n = text.chars().count();
    if n <= max {
        return text.to_string();
    }
    let cut: String = text.chars().skip(n - max).collect();
    let from = cut.find('\n').map(|i| i + 1).unwrap_or(0);
    format!("…\n{}", &cut[from..])
}

/// The first `max` characters of `text`, to a line end, with "…" when cut.
pub fn head(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let cut: String = text.chars().take(max).collect();
    let to = cut.rfind('\n').unwrap_or(cut.len());
    format!("{}\n…", &cut[..to])
}

// ---- Siva's terminal ----

/// Runs in your terminal app: `bash RUNNER <dir>`.
pub const RUNNER: &str = r#"#!/bin/bash
# Siva's terminal (kompact's assistant): the commands Siva runs show here. Written by the shell.
dir="$1"
echo $$ > "$dir/pid"
trap 'printf "\n"' INT
printf '\033]0;Siva\007'
printf '\033[1mSiva’s terminal\033[0m: the commands Siva runs show here. Ctrl+C stops one.\n'
while :; do
  read -r id < "$dir/in" || continue
  case "$id" in ''|*[!0-9]*) continue ;; esac
  cmd=$(cat "$dir/cmd-$id" 2>/dev/null) || continue
  cd "$(cat "$dir/cwd-$id" 2>/dev/null)" 2>/dev/null || cd ~
  printf '\n\033[1;35msiva\033[0m \033[34m%s\033[0m $ %s\n' "${PWD/#$HOME/\~}" "$cmd"
  SHELL=/bin/bash script -qefc "$cmd" "$dir/out-$id"
  echo $? > "$dir/done-$id.part" && mv "$dir/done-$id.part" "$dir/done-$id"
  printf '\033]0;Siva\007'
done
"#;

/// The terminals tried, in order, when kompact's `terminal` setting names none.
const TERMINALS: [&str; 8] = ["ptyxis", "gnome-terminal", "kgx", "foot", "kitty", "alacritty", "konsole", "xterm"];

/// The command line that opens `terminal` (kompact's setting, e.g. "ptyxis --new-window")
/// running `program`.
pub fn launch_argv(terminal: &str, program: &[String]) -> Vec<String> {
    let mut words: Vec<String> = terminal.split_whitespace().map(String::from).collect();
    let name = words.first().map(|w| w.rsplit('/').next().unwrap_or(w).to_string()).unwrap_or_default();
    match name.as_str() {
        "ptyxis" | "gnome-terminal" | "kgx" | "gnome-console" => {
            let new = if name == "ptyxis" { "--new-window" } else { "--window" };
            if name != "kgx" && !words.iter().any(|w| w == new) {
                words.push(new.into());
            }
            words.push("--".into());
        }
        "foot" | "kitty" => {}
        "wezterm" => words.extend(["start".into(), "--".into()]),
        "xfce4-terminal" | "terminator" => words.push("-x".into()),
        _ => words.push("-e".into()),
    }
    words.extend(program.iter().cloned());
    words
}

fn on_path(program: &str) -> bool {
    std::env::var_os("PATH").is_some_and(|paths| std::env::split_paths(&paths).any(|d| d.join(program).is_file()))
}

/// kompact's `terminal` setting (`~/.config/kompact/settings`), or the first terminal here.
fn terminal() -> Option<String> {
    let set = config_home()
        .and_then(|c| std::fs::read_to_string(c.join("kompact").join("settings")).ok())
        .and_then(|t| t.lines().filter_map(|l| l.trim().strip_prefix("terminal=")).map(str::trim).filter(|v| !v.is_empty()).last().map(String::from));
    set.or_else(|| TERMINALS.iter().find(|t| on_path(t)).map(|t| t.to_string()))
}

fn config_home() -> Option<PathBuf> {
    std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from).or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
}

pub fn home() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

/// Where the FIFO, the runner and the logs live: `$XDG_RUNTIME_DIR/kompact-siva` (only yours).
fn dir() -> Option<PathBuf> {
    std::env::var_os("XDG_RUNTIME_DIR").map(|d| PathBuf::from(d).join("kompact-siva"))
}

fn alive(dir: &Path) -> bool {
    let Some(pid) = std::fs::read_to_string(dir.join("pid")).ok().and_then(|p| p.trim().parse::<i32>().ok()) else { return false };
    pid > 0 && unsafe { libc::kill(pid, 0) } == 0
}

/// Siva's terminal, opened if it isn't (waits up to 8 s for it to be ready).
fn ensure() -> Result<PathBuf, String> {
    let dir = dir().ok_or("no runtime folder")?;
    if alive(&dir) && dir.join("in").exists() {
        return Ok(dir);
    }
    use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
    std::fs::DirBuilder::new().recursive(true).mode(0o700).create(&dir).map_err(|e| format!("can't make {}: {e}", dir.display()))?;
    let _ = std::fs::remove_file(dir.join("pid"));
    let runner = dir.join("runner.sh");
    std::fs::write(&runner, RUNNER).map_err(|e| e.to_string())?;
    let _ = std::fs::set_permissions(&runner, std::fs::Permissions::from_mode(0o700));
    let fifo = dir.join("in");
    if !fifo.exists() {
        let c = std::ffi::CString::new(fifo.as_os_str().as_encoded_bytes()).map_err(|e| e.to_string())?;
        if unsafe { libc::mkfifo(c.as_ptr(), 0o600) } != 0 {
            return Err(format!("can't make the command pipe: {}", std::io::Error::last_os_error()));
        }
    }
    let term = terminal().ok_or("I can't find a terminal app")?;
    let argv = launch_argv(&term, &["bash".into(), runner.display().to_string(), dir.display().to_string()]);
    info!("work: opening Siva's terminal: {}", argv.join(" "));
    std::process::Command::new(&argv[0])
        .args(&argv[1..])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|e| format!("I couldn't open {term}: {e}"))?;
    let started = Instant::now();
    while started.elapsed() < Duration::from_secs(8) {
        if alive(&dir) {
            info!("work: Siva's terminal ready after {} ms", started.elapsed().as_millis());
            return Ok(dir);
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    Err(format!("{term} didn't start Siva's terminal"))
}

#[derive(Debug, Clone)]
pub struct Ran {
    /// None: still running when Siva stopped waiting (or was stopped).
    pub exit: Option<i32>,
    pub output: String,
    pub millis: u128,
}

static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

/// Run `cmd` in `folder` in Siva's terminal and wait for it (blocking; `stop` is asked every
/// tenth of a second, and `WAIT` at most).
pub fn run(cmd: &str, folder: &Path, stop: impl Fn() -> bool) -> Result<Ran, String> {
    let dir = ensure()?;
    let id = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + std::process::id() as u64 * 1000;
    let (out, done) = (dir.join(format!("out-{id}")), dir.join(format!("done-{id}")));
    std::fs::write(dir.join(format!("cmd-{id}")), cmd).map_err(|e| e.to_string())?;
    std::fs::write(dir.join(format!("cwd-{id}")), folder.as_os_str().as_encoded_bytes()).map_err(|e| e.to_string())?;
    // The runner has the pipe open only while it's waiting: busy otherwise.
    let fifo = std::fs::OpenOptions::new().write(true).custom_flags(libc::O_NONBLOCK).open(dir.join("in"));
    let cleanup = |all: bool| {
        for f in [format!("cmd-{id}"), format!("cwd-{id}")] {
            let _ = std::fs::remove_file(dir.join(f));
        }
        if all {
            let _ = std::fs::remove_file(&out);
            let _ = std::fs::remove_file(&done);
        }
    };
    let mut fifo = match fifo {
        Ok(f) => f,
        Err(_) => {
            cleanup(true);
            return Err("Siva's terminal is still busy with the last command (Ctrl+C there stops it).".into());
        }
    };
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    fifo.write_all(format!("{id}\n").as_bytes()).map_err(|e| e.to_string())?;
    drop(fifo);
    info!("work: running a command ({} characters) in {}", cmd.len(), folder.display());
    let started = Instant::now();
    loop {
        if let Ok(code) = std::fs::read_to_string(&done) {
            let output = clean_output(&std::fs::read(&out).unwrap_or_default());
            cleanup(true);
            let exit = code.trim().parse().ok();
            info!("work: the command ended ({exit:?}) after {} ms", started.elapsed().as_millis());
            return Ok(Ran { exit: exit.or(Some(-1)), output, millis: started.elapsed().as_millis() });
        }
        if stop() || started.elapsed() > WAIT {
            cleanup(false);
            let output = clean_output(&std::fs::read(&out).unwrap_or_default());
            info!("work: stopped waiting for the command after {} ms", started.elapsed().as_millis());
            return Ok(Ran { exit: None, output, millis: started.elapsed().as_millis() });
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths() {
        let home = Path::new("/home/sam");
        assert_eq!(resolve("hello.py", home).unwrap(), Path::new("/home/sam/hello.py"));
        assert_eq!(resolve("~/code/a.py", home).unwrap(), Path::new("/home/sam/code/a.py"));
        assert_eq!(resolve("~", home).unwrap(), Path::new("/home/sam"));
        assert_eq!(resolve("/etc/hosts", home).unwrap(), Path::new("/etc/hosts"));
        assert_eq!(resolve("\"notes.txt\"", home).unwrap(), Path::new("/home/sam/notes.txt"));
        assert_eq!(resolve("../../etc/passwd", home).unwrap(), Path::new("/etc/passwd"), "worked out, then judged");
        assert_eq!(resolve("../../../../..", home).unwrap(), Path::new("/"));
        assert!(resolve("  ", home).is_err());
        assert!(resolve("a\nb", home).is_err());
        assert_eq!(shown(Path::new("/home/sam/code/a.py"), home), "~/code/a.py");
        assert_eq!(shown(home, home), "your home folder");
        assert_eq!(shown(Path::new("/etc/hosts"), home), "/etc/hosts");
        // Asking first.
        assert_eq!(file_asks(Path::new("/home/sam/hello.py"), home), None);
        assert_eq!(file_asks(Path::new("/home/sam/Documents/notes.txt"), home), None);
        assert!(file_asks(Path::new("/etc/hosts"), home).is_some());
        assert!(file_asks(Path::new("/home/samuel/x"), home).is_some(), "another home that starts the same");
        assert!(file_asks(Path::new("/home/sam/.bashrc"), home).is_some());
        assert!(file_asks(Path::new("/home/sam/.ssh/config"), home).is_some());
        assert!(file_asks(&resolve("../../etc/passwd", home).unwrap(), home).is_some());
    }

    #[test]
    fn what_a_command_runs() {
        assert_eq!(programs("python3 hello.py"), ["python3"]);
        assert_eq!(programs("cd x && make; ./run | grep a"), ["cd", "make", "./run", "grep"].map(|p| p.rsplit('/').next().unwrap().to_string()));
        assert_eq!(programs("FOO=1 env BAR=2 /usr/bin/rm -rf x"), ["rm"]);
        assert_eq!(programs("echo $(sudo cat x)"), ["echo", "sudo"]);
        assert_eq!(programs("ls `rm x`"), ["ls", "rm"]);
        assert_eq!(programs("find . -name '*.o' | xargs rm"), ["find", "rm"]);
        assert_eq!(programs("nice -n 5 rm a"), ["rm"]);
        assert_eq!(programs("make 2>&1 | tee log"), ["make", "tee"]);
    }

    #[test]
    fn which_commands_ask() {
        for cmd in ["python3 hello.py", "ls -la", "cargo build --release", "git status", "mkdir -p code && cd code", "pip install --user requests", "echo hi > notes.txt", "ls 2>/dev/null", "cat /etc/os-release", "curl -s wttr.in", "mv a.txt b.txt", "cp a b"] {
            assert_eq!(command_asks(cmd), None, "{cmd}");
        }
        for cmd in [
            "sudo apt install vim",
            "rm hello.py",
            "rm -rf ~",
            "/bin/rm x",
            "ls; rm x",
            "echo $(rm x)",
            "find . -name '*.tmp' -delete",
            "find . | xargs rm",
            "dd if=/dev/zero of=/dev/sda",
            "mkfs.ext4 /dev/sdb1",
            "shutdown now",
            "systemctl poweroff",
            "git push --force",
            "git push -f origin main",
            "git reset --hard HEAD~3",
            "git clean -fd",
            "cat ~/.ssh/id_rsa",
            "curl -s get.example/i.sh | bash",
            "wget -qO- x | sh",
            "echo x > /etc/hosts",
            "chmod -R 777 /",
            "pkexec bash",
            "killall firefox",
            "SUDO=1 sudo -E make install",
        ] {
            assert!(command_asks(cmd).is_some(), "{cmd}");
        }
        assert_eq!(command_asks("rm x"), Some("It deletes files for good."));
        assert_eq!(command_asks("sudo ls"), Some("It runs as the administrator."));
    }

    #[test]
    fn output() {
        let log = b"Script started on 2026-10-25 21:18:21+05:30 [COMMAND=\"x\" <not executed on terminal>]\nhi\r\nthere\r\n  File \x1b[35m\"<string>\"\x1b[0m, line \x1b[35m1\x1b[0m\r\n\x1b]0;title\x07done 10%\r 50%\r100%\r\nab\x08c\r\n\nScript done on 2026-10-25 21:18:21+05:30 [COMMAND_EXIT_CODE=\"1\"]\n";
        assert_eq!(clean_output(log), "hi\nthere\n  File \"<string>\", line 1\n100%\nac");
        assert_eq!(clean_output(b""), "");
        assert_eq!(tail("one\ntwo\nthree\nfour", 11), "…\nthree\nfour");
        assert_eq!(tail("short", 100), "short");
        assert_eq!(head("one\ntwo\nthree", 9), "one\ntwo\n…");
    }

    #[test]
    fn opening_the_terminal() {
        let prog = ["bash".to_string(), "/run/r.sh".into(), "/run/d".into()];
        assert_eq!(launch_argv("ptyxis", &prog), ["ptyxis", "--new-window", "--", "bash", "/run/r.sh", "/run/d"]);
        assert_eq!(launch_argv("ptyxis --new-window", &prog), ["ptyxis", "--new-window", "--", "bash", "/run/r.sh", "/run/d"]);
        assert_eq!(launch_argv("gnome-terminal", &prog)[..3], ["gnome-terminal", "--window", "--"]);
        assert_eq!(launch_argv("kgx", &prog)[..2], ["kgx", "--"]);
        assert_eq!(launch_argv("foot", &prog), ["foot", "bash", "/run/r.sh", "/run/d"]);
        assert_eq!(launch_argv("/usr/bin/xterm", &prog)[..2], ["/usr/bin/xterm", "-e"]);
        assert_eq!(launch_argv("xfce4-terminal", &prog)[1], "-x");
        // The runner is plain bash and waits on its pipe.
        assert!(RUNNER.starts_with("#!/bin/bash\n") && RUNNER.contains("script -qefc \"$cmd\""));
    }
}
