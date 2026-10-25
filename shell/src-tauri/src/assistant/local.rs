//! 21c: the work loop on this PC, for when the cloud AI can't be used (no key, Internet not
//! Always, or it failed): Ubuntu's `llama-server` with Qwen2.5-Coder 3B (fetched by
//! `kompact-assistant-setup work`; Qwen RESEARCH LICENSE, non-commercial use), on a UNIX socket in
//! `$XDG_RUNTIME_DIR/kompact-siva/` (only yours; no network port). It speaks the OpenAI chat
//! format with tools, so `agent` talks to it as to the cloud AI.
//!
//! Started for the first task (the model loads in a few seconds), at `nice 5`; it keeps the
//! start of the conversation it has read, so each step reads only what's new. Stopped after
//! `IDLE` without a task, and when the shell goes (`PR_SET_PDEATHSIG`): 2.4 GB isn't kept for
//! nothing.

use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use tracing::{info, warn};

pub const MODEL: &str = "qwen2.5-coder-3b-instruct-q4_k_m.gguf";
pub const NAME: &str = "Qwen2.5-Coder 3B";
/// Without a task this long, the server stops.
const IDLE: Duration = Duration::from_secs(300);
/// The longest wait for the model to load.
const LOADING: Duration = Duration::from_secs(90);
/// Room for the conversation (tokens): the tools, the task's steps and their results.
const CONTEXT: &str = "8192";

struct Server {
    pid: i32,
    /// Set by its thread when it has ended.
    ended: Arc<AtomicBool>,
    last: Instant,
}

static SERVER: Mutex<Option<Server>> = Mutex::new(None);

fn data_home() -> Option<PathBuf> {
    std::env::var_os("XDG_DATA_HOME").map(PathBuf::from).or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local").join("share")))
}

fn model_path() -> Option<PathBuf> {
    Some(data_home()?.join("kompact").join("llm").join(MODEL))
}

fn on_path(program: &str) -> bool {
    std::env::var_os("PATH").is_some_and(|paths| std::env::split_paths(&paths).any(|d| d.join(program).is_file()))
}

/// Whether the model and `llama-server` are here.
pub fn ready() -> bool {
    on_path("llama-server") && model_path().is_some_and(|p| p.is_file())
}

fn socket() -> Option<PathBuf> {
    std::env::var_os("XDG_RUNTIME_DIR").map(|d| PathBuf::from(d).join("kompact-siva").join("llm.sock"))
}

/// The CPU's cores (not threads): llama.cpp runs best on those.
fn threads() -> String {
    let n = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4);
    (n / 2).clamp(1, 8).to_string()
}

/// The server, started if it isn't running.
fn ensure() -> Result<PathBuf, String> {
    let sock = socket().ok_or("no runtime folder")?;
    let mut guard = SERVER.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(s) = guard.as_mut() {
        if !s.ended.load(Ordering::Relaxed) {
            s.last = Instant::now();
            return Ok(sock);
        }
        warn!("local: the model server had stopped");
        *guard = None;
    }
    let model = model_path().filter(|p| p.is_file()).ok_or("my model for tasks isn't set up: run “kompact-assistant-setup work”")?;
    if let Some(dir) = sock.parent() {
        use std::os::unix::fs::DirBuilderExt;
        std::fs::DirBuilder::new().recursive(true).mode(0o700).create(dir).map_err(|e| e.to_string())?;
    }
    let _ = std::fs::remove_file(&sock);
    let started = Instant::now();
    let mut cmd = Command::new("nice");
    cmd.args(["-n", "5", "llama-server", "-m"])
        .arg(&model)
        .arg("--host")
        .arg(&sock)
        .args(["--jinja", "-c", CONTEXT, "-np", "1", "-t", &threads(), "--no-webui", "--log-disable"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    unsafe {
        use std::os::unix::process::CommandExt;
        // It goes when the shell goes.
        cmd.pre_exec(|| {
            libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM);
            Ok(())
        });
    }
    // Its own thread starts it and waits for it: PR_SET_PDEATHSIG fires when the *thread* that
    // started it ends (a pool thread would end when idle and take the server with it).
    let ended = Arc::new(AtomicBool::new(false));
    let (tx, rx) = std::sync::mpsc::channel();
    let flag = ended.clone();
    std::thread::Builder::new()
        .name("siva-llm".into())
        .spawn(move || match cmd.spawn() {
            Ok(mut child) => {
                let _ = tx.send(Ok(child.id() as i32));
                let status = child.wait();
                info!("local: {NAME} ended ({status:?})");
                flag.store(true, Ordering::Relaxed);
            }
            Err(e) => {
                let _ = tx.send(Err(format!("I couldn't start llama-server: {e}")));
                flag.store(true, Ordering::Relaxed);
            }
        })
        .map_err(|e| e.to_string())?;
    let pid = rx.recv().map_err(|e| e.to_string())??;
    info!("local: starting {NAME} (pid {pid}, {} threads)", threads());
    *guard = Some(Server { pid, ended: ended.clone(), last: Instant::now() });
    drop(guard);
    // Ready when /health says so.
    while started.elapsed() < LOADING {
        if let Ok((200, _)) = super::web::post_local(&sock, "/health", "", 2) {
            info!("local: {NAME} ready after {} ms", started.elapsed().as_millis());
            watch_idle();
            return Ok(sock);
        }
        if ended.load(Ordering::Relaxed) {
            stop();
            return Err("llama-server stopped while loading my model".into());
        }
        std::thread::sleep(Duration::from_millis(250));
    }
    stop();
    Err("my model took too long to load".into())
}

/// Stops the server once it's been idle for `IDLE` (one watcher a server).
fn watch_idle() {
    std::thread::spawn(|| loop {
        std::thread::sleep(Duration::from_secs(30));
        let mut guard = SERVER.lock().unwrap_or_else(|e| e.into_inner());
        match guard.as_mut() {
            None => return,
            Some(s) if s.last.elapsed() > IDLE => {
                info!("local: {NAME} idle for {} s, stopping it", s.last.elapsed().as_secs());
                drop(guard);
                stop();
                return;
            }
            Some(_) => {}
        }
    });
}

pub fn stop() {
    if let Some(s) = SERVER.lock().unwrap_or_else(|e| e.into_inner()).take() {
        // `nice` has become llama-server (same pid); its thread reaps it.
        unsafe {
            libc::kill(s.pid, libc::SIGTERM);
        }
        let started = Instant::now();
        while !s.ended.load(Ordering::Relaxed) && started.elapsed() < Duration::from_secs(5) {
            std::thread::sleep(Duration::from_millis(50));
        }
    }
    if let Some(sock) = socket() {
        let _ = std::fs::remove_file(sock);
    }
}

/// One chat request (`body`: the OpenAI chat format) to the model: the status and the body.
/// Blocking; the first one waits for the model to load.
pub fn chat(body: &str, seconds: u32) -> Result<(u16, String), String> {
    let sock = ensure()?;
    let r = super::web::post_local(&sock, "/v1/chat/completions", body, seconds);
    if let Some(s) = SERVER.lock().unwrap_or_else(|e| e.into_inner()).as_mut() {
        s.last = Instant::now();
    }
    r
}
