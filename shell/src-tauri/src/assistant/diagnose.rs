//! 19g: answers from the desktop's own state, for "why is my computer slow?", "how much disk
//! space is left?" and "why does the sound sound bad?". Read only (Tier 0): `/proc`, the disks'
//! sizes, and PipeWire's state as `pw-dump` gives it. The reading and the wording are pure and
//! tested; `sample`, `disks` and `sound_dump` look at the real thing.

use std::collections::HashMap;
use std::time::Duration;

use serde_json::Value;

// ---- the CPU and memory (pure) ----

/// `(busy, total)` jiffies from `/proc/stat`'s first line.
pub fn cpu_times(stat: &str) -> Option<(u64, u64)> {
    let line = stat.lines().next()?.strip_prefix("cpu ")?;
    let v: Vec<u64> = line.split_whitespace().filter_map(|x| x.parse().ok()).collect();
    if v.len() < 4 {
        return None;
    }
    let total: u64 = v.iter().take(8).sum();
    // idle + iowait aren't busy.
    let idle = v[3] + v.get(4).copied().unwrap_or(0);
    Some((total - idle, total))
}

/// `(name, utime + stime)` from a `/proc/<pid>/stat` line (the name may hold spaces and
/// parentheses, so it's up to the last ')').
pub fn proc_ticks(line: &str) -> Option<(String, u64)> {
    let open = line.find('(')?;
    let close = line.rfind(')')?;
    let name = line[open + 1..close].to_string();
    let rest: Vec<&str> = line[close + 1..].split_whitespace().collect();
    // After the name: state(0) … utime(11) stime(12).
    let utime: u64 = rest.get(11)?.parse().ok()?;
    let stime: u64 = rest.get(12)?.parse().ok()?;
    Some((name, utime + stime))
}

/// `(total, available)` kB from `/proc/meminfo`.
pub fn meminfo(text: &str) -> Option<(u64, u64)> {
    let field = |name: &str| text.lines().find_map(|l| l.strip_prefix(name)?.trim().strip_suffix("kB")?.trim().parse::<u64>().ok());
    Some((field("MemTotal:")?, field("MemAvailable:")?))
}

/// What a process is, in words people use (Firefox runs as many processes with other names).
pub fn friendly(comm: &str) -> String {
    match comm {
        "firefox" | "Isolated Web Co" | "Web Content" | "Privileged Cont" | "WebExtensions" | "RDD Process" | "Socket Process" | "Utility Process" | "GPU Process" => "Firefox".into(),
        "WebKitWebProces" | "WebKitNetworkPr" | "kompact-shell" => "the desktop shell".into(),
        "kompact" => "the desktop itself (kompact)".into(),
        "whisper-cli" => "my hearing (whisper)".into(),
        "llama-completion" => "my thinking (the local model)".into(),
        "piper" => "my voice (Piper)".into(),
        "pipewire" | "pipewire-pulse" | "wireplumber" => "the sound system".into(),
        "nautilus" => "Files".into(),
        "ptyxis" | "ptyxis-agent" | "gnome-terminal-" | "foot" => "the terminal".into(),
        "code" | "Code" => "VS Code".into(),
        c if c.starts_with("kworker") => "the kernel".into(),
        c => c.to_string(),
    }
}

/// Per app (by `friendly` name), what share of the whole machine it used between two samples
/// of `/proc/<pid>/stat` ticks, the busiest first.
pub fn busiest(before: &HashMap<u32, (String, u64)>, after: &HashMap<u32, (String, u64)>, total_ticks: u64) -> Vec<(String, f64)> {
    let mut by_app: HashMap<String, u64> = HashMap::new();
    for (pid, (name, t)) in after {
        let was = before.get(pid).map(|(_, b)| *b).unwrap_or(*t);
        *by_app.entry(friendly(name)).or_default() += t.saturating_sub(was);
    }
    let mut list: Vec<(String, f64)> = by_app.into_iter().filter(|(_, d)| *d > 0).map(|(n, d)| (n, 100.0 * d as f64 / total_ticks.max(1) as f64)).collect();
    list.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
    list
}

pub struct Load {
    /// Of the whole machine (all cores), 0–100.
    pub cpu: f64,
    pub top: Vec<(String, f64)>,
    pub mem_total_kb: u64,
    pub mem_free_kb: u64,
}

fn gb(kb: u64) -> String {
    format!("{:.1} GB", kb as f64 / 1024.0 / 1024.0)
}

/// "Your computer is busy: 72% of the CPU, most of it Firefox (45%) and the desktop shell (12%).
/// Memory: 5.1 GB of 7.2 GB in use."
pub fn load_text(l: &Load) -> String {
    let named: Vec<String> = l.top.iter().filter(|(_, p)| *p >= 3.0).take(3).map(|(n, p)| format!("{n} ({p:.0}%)")).collect();
    let used = l.mem_total_kb.saturating_sub(l.mem_free_kb);
    let mem_pct = 100.0 * used as f64 / l.mem_total_kb.max(1) as f64;
    let cpu = if l.cpu >= 60.0 {
        format!("Your computer is busy: {:.0}% of the CPU", l.cpu)
    } else if l.cpu >= 25.0 {
        format!("Your computer is fairly busy: {:.0}% of the CPU", l.cpu)
    } else {
        format!("Your computer isn't busy: {:.0}% of the CPU", l.cpu)
    };
    let who = match named.as_slice() {
        [] => String::new(),
        [one] => format!(", mostly {one}"),
        [a, rest @ ..] => format!(", mostly {a} and {}", rest.join(" and ")),
    };
    let mem = format!("Memory: {} of {} in use", gb(used), gb(l.mem_total_kb));
    let tight = if mem_pct >= 90.0 { "; that's nearly all of it, so things may slow down while it swaps" } else { "" };
    format!("{cpu}{who}. {mem}{tight}.")
}

// ---- the disks (pure) ----

pub struct Disk {
    pub what: &'static str,
    pub free: u64,
    pub size: u64,
    pub device: u64,
}

fn size(bytes: u64) -> String {
    let g = bytes as f64 / 1e9;
    if g >= 10.0 { format!("{g:.0} GB") } else { format!("{g:.1} GB") }
}

/// "The system disk has 23 GB free of 98 GB (your home folder is on it)." and a warning when
/// it's nearly full.
pub fn disks_text(disks: &[Disk]) -> String {
    let mut out = Vec::new();
    for (i, d) in disks.iter().enumerate() {
        if i > 0 && disks[..i].iter().any(|o| o.device == d.device) {
            continue;
        }
        let shared = disks.iter().skip(i + 1).any(|o| o.device == d.device);
        let pct = 100.0 * d.free as f64 / d.size.max(1) as f64;
        let mut s = format!("The {} has {} free of {}", d.what, size(d.free), size(d.size));
        if shared {
            s.push_str(" (your home folder is on it)");
        }
        if pct < 10.0 {
            s.push_str(&format!(": only {pct:.0}% left, which can slow things down"));
        }
        out.push(format!("{s}."));
    }
    out.join(" ")
}

// ---- the sound (pure) ----

/// Why the sound may be bad, from a `pw-dump` (and the CPU load, if known).
pub fn sound_text(dump: &Value, cpu: Option<f64>) -> String {
    let objects = dump.as_array().map(Vec::as_slice).unwrap_or(&[]);
    let sink_name = crate::sound::default_name(dump, "default.audio.sink");
    let sink = objects.iter().find(|o| o["type"] == "PipeWire:Interface:Node" && o["info"]["props"]["media.class"] == "Audio/Sink" && Some(o["info"]["props"]["node.name"].as_str().unwrap_or("")) == sink_name);
    let Some(sink) = sink else { return "There's no sound output chosen; pick one in Settings → Sound.".into() };
    let props = &sink["info"]["props"];
    let label = props["node.description"].as_str().or(props["node.nick"].as_str()).unwrap_or("the speakers");
    let muted = sink["info"]["params"]["Props"].as_array().and_then(|p| p.iter().find_map(|x| x["mute"].as_bool())).unwrap_or(false);
    let busy = cpu.filter(|c| *c >= 80.0).map(|c| format!(" The CPU is very busy ({c:.0}%), which can make sound stutter.")).unwrap_or_default();
    if muted {
        return format!("The sound is muted on {label}.{busy}");
    }
    // Bluetooth by its card (the device): the default output of a headset is a loopback node
    // that carries the card's id but not `device.api` (seen 2026-10-24).
    let device = props["device.id"].as_u64();
    let card = objects.iter().find(|o| o["type"] == "PipeWire:Interface:Device" && device.is_some() && o["id"].as_u64() == device);
    if card.map(|c| &c["info"]["props"]["device.api"]) != Some(&Value::from("bluez5")) {
        return format!("The sound plays on {label}; nothing looks wrong with it.{busy}");
    }
    let profile = card.and_then(|d| d["info"]["params"]["Profile"].as_array()?.first()?["name"].as_str()).unwrap_or("");
    let codec = objects
        .iter()
        .filter(|o| o["info"]["props"]["device.id"].as_u64() == device)
        .find_map(|o| o["info"]["props"]["api.bluez5.codec"].as_str())
        .unwrap_or("");
    if profile.starts_with("headset") {
        let users: Vec<String> = crate::sound::recording(dump).into_iter().filter(|a| a != "Siva").collect();
        let why = match users.as_slice() {
            [] => "because a microphone was just in use; it goes back to music quality a moment after".to_string(),
            [one] => format!("because {one} is using its microphone"),
            many => format!("because {} are using its microphone", many.join(" and ")),
        };
        return format!("{label} is in headset mode, which sounds like a phone call, {why}.{busy}");
    }
    let codec = if codec.is_empty() { String::new() } else { format!(" ({})", codec.to_uppercase()) };
    format!("{label} plays in music quality{codec}. If it cuts out, it's usually Bluetooth range: walls, or 2.4 GHz Wi-Fi nearby.{busy}")
}

// ---- the real thing ----

fn read_procs() -> HashMap<u32, (String, u64)> {
    let mut out = HashMap::new();
    for e in std::fs::read_dir("/proc").into_iter().flatten().flatten() {
        let Ok(pid) = e.file_name().to_string_lossy().parse::<u32>() else { continue };
        if let Some(t) = std::fs::read_to_string(e.path().join("stat")).ok().as_deref().and_then(proc_ticks) {
            out.insert(pid, t);
        }
    }
    out
}

/// The CPU over half a second, and memory now (blocking).
pub fn sample() -> Option<Load> {
    let (b1, t1) = cpu_times(&std::fs::read_to_string("/proc/stat").ok()?)?;
    let p1 = read_procs();
    std::thread::sleep(Duration::from_millis(500));
    let (b2, t2) = cpu_times(&std::fs::read_to_string("/proc/stat").ok()?)?;
    let p2 = read_procs();
    let total = t2.saturating_sub(t1);
    let (mem_total_kb, mem_free_kb) = meminfo(&std::fs::read_to_string("/proc/meminfo").ok()?)?;
    Some(Load { cpu: 100.0 * b2.saturating_sub(b1) as f64 / total.max(1) as f64, top: busiest(&p1, &p2, total), mem_total_kb, mem_free_kb })
}

fn disk(what: &'static str, path: &str) -> Option<Disk> {
    use std::os::unix::fs::MetadataExt;
    let c = std::ffi::CString::new(path).ok()?;
    let mut s: libc::statvfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::statvfs(c.as_ptr(), &mut s) } != 0 {
        return None;
    }
    let device = std::fs::metadata(path).ok()?.dev();
    Some(Disk { what, free: s.f_bavail as u64 * s.f_frsize as u64, size: s.f_blocks as u64 * s.f_frsize as u64, device })
}

/// The system disk and the home folder's.
pub fn disks() -> Vec<Disk> {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/home".into());
    [disk("system disk", "/"), disk("home folder's disk", &home)].into_iter().flatten().collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn reading_proc() {
        assert_eq!(cpu_times("cpu  13568 10354 23783 623020 597 0 152 0 0 0\ncpu0 1 2 3 4\n"), Some((13568 + 10354 + 23783 + 152, 13568 + 10354 + 23783 + 623020 + 597 + 152)));
        assert_eq!(cpu_times("intr 1 2"), None);
        assert_eq!(proc_ticks("14643 (cat) R 14640 14643 14640 0 -1 4194304 494 0 0 0 7 3 0 0 20 0 1 0"), Some(("cat".into(), 10)));
        assert_eq!(proc_ticks("9 (Isolated Web Co) S 1 1 1 0 -1 0 0 0 0 0 120 30 0 0").map(|t| t.0), Some("Isolated Web Co".into()), "a name with spaces");
        assert_eq!(proc_ticks("9 (a) b) S 1 1 1 0 -1 0 0 0 0 0 5 5 0 0").map(|t| t.1), Some(10), "a ')' in the name");
        assert_eq!(meminfo("MemTotal:        7521380 kB\nMemFree: 1 kB\nMemAvailable:    5668928 kB\n"), Some((7521380, 5668928)));
    }

    #[test]
    fn who_is_busy() {
        let snap = |v: &[(u32, &str, u64)]| v.iter().map(|(p, n, t)| (*p, (n.to_string(), *t))).collect::<HashMap<_, _>>();
        let before = snap(&[(1, "firefox", 100), (2, "Isolated Web Co", 50), (3, "kompact", 10), (4, "bash", 5)]);
        let after = snap(&[(1, "firefox", 140), (2, "Isolated Web Co", 70), (3, "kompact", 20), (4, "bash", 5), (5, "new", 3)]);
        let top = busiest(&before, &after, 200);
        assert_eq!(top[0], ("Firefox".into(), 30.0), "Firefox's processes together");
        assert_eq!(top[1], ("the desktop itself (kompact)".into(), 5.0));
        assert_eq!(top.len(), 2, "idle ones and ones that only just started don't count");
    }

    #[test]
    fn saying_how_busy() {
        let l = |cpu, top: Vec<(&str, f64)>, free| Load { cpu, top: top.into_iter().map(|(n, p)| (n.to_string(), p)).collect(), mem_total_kb: 7_521_380, mem_free_kb: free };
        assert_eq!(
            load_text(&l(72.0, vec![("Firefox", 45.2), ("the desktop shell", 12.0), ("bash", 1.0)], 2_000_000)),
            "Your computer is busy: 72% of the CPU, mostly Firefox (45%) and the desktop shell (12%). Memory: 5.3 GB of 7.2 GB in use."
        );
        assert_eq!(load_text(&l(6.0, vec![], 5_668_928)), "Your computer isn't busy: 6% of the CPU. Memory: 1.8 GB of 7.2 GB in use.");
        assert!(load_text(&l(30.0, vec![("Firefox", 20.0)], 300_000)).contains("nearly all of it"));
    }

    #[test]
    fn saying_how_much_space() {
        let d = |what, free: u64, size: u64, device| Disk { what, free: free * 1_000_000_000, size: size * 1_000_000_000, device };
        assert_eq!(disks_text(&[d("system disk", 23, 98, 1), d("home folder's disk", 23, 98, 1)]), "The system disk has 23 GB free of 98 GB (your home folder is on it).");
        assert_eq!(
            disks_text(&[d("system disk", 5, 98, 1), d("home folder's disk", 300, 1000, 2)]),
            "The system disk has 5.0 GB free of 98 GB: only 5% left, which can slow things down. The home folder's disk has 300 GB free of 1000 GB."
        );
    }

    fn dump(profile: &str, recording: Option<&str>, muted: bool) -> Value {
        let mut v = vec![
            json!({"id": 41, "type": "PipeWire:Interface:Metadata", "props": {"metadata.name": "default"},
                "metadata": [{"subject": 0, "key": "default.audio.sink", "value": {"name": "bluez_output.0B"}}]}),
            json!({"id": 62, "type": "PipeWire:Interface:Device", "info": {"props": {"device.api": "bluez5"}, "params": {"Profile": [{"name": profile}]}}}),
            // As PipeWire has it: the default output is a loopback without `device.api`; the codec
            // is on the card's internal node.
            json!({"id": 69, "type": "PipeWire:Interface:Node", "info": {"props": {"media.class": "Audio/Sink", "node.name": "bluez_output.0B", "node.description": "SnapUp Boom X Pro", "bluez5.sink-loopback": true, "device.id": 62}, "params": {"Props": [{"mute": muted}]}}}),
            json!({"id": 76, "type": "PipeWire:Interface:Node", "info": {"props": {"media.class": "Audio/Sink/Internal", "device.api": "bluez5", "device.id": 62, "api.bluez5.codec": if profile == "a2dp-sink" { "aac" } else { "cvsd" }}}}),
        ];
        if let Some(app) = recording {
            v.push(json!({"id": 80, "type": "PipeWire:Interface:Node", "info": {"state": "running", "props": {"media.class": "Stream/Input/Audio", "application.name": app}}}));
        }
        Value::Array(v)
    }

    /// This PC now: `cargo test diagnose_live -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn diagnose_live() {
        let load = sample().unwrap();
        println!("{}", load_text(&load));
        println!("{}", disks_text(&disks()));
        println!("{}", sound_text(&crate::sound::dump_now().unwrap(), Some(load.cpu)));
    }

    #[test]
    fn why_the_sound_is_bad() {
        assert_eq!(
            sound_text(&dump("headset-head-unit", Some("Firefox"), false), None),
            "SnapUp Boom X Pro is in headset mode, which sounds like a phone call, because Firefox is using its microphone."
        );
        assert!(sound_text(&dump("headset-head-unit", Some("Siva"), false), None).contains("a moment after"), "Siva's own listening doesn't count");
        assert!(sound_text(&dump("a2dp-sink", None, false), Some(95.0)).starts_with("SnapUp Boom X Pro plays in music quality (AAC)."));
        assert!(sound_text(&dump("a2dp-sink", None, false), Some(95.0)).contains("very busy (95%)"));
        assert_eq!(sound_text(&dump("a2dp-sink", None, true), None), "The sound is muted on SnapUp Boom X Pro.");
        assert!(sound_text(&json!([]), None).contains("no sound output"));
        // A wired output.
        let wired = json!([
            {"id": 41, "type": "PipeWire:Interface:Metadata", "props": {"metadata.name": "default"}, "metadata": [{"subject": 0, "key": "default.audio.sink", "value": {"name": "alsa_output.hdmi"}}]},
            {"id": 48, "type": "PipeWire:Interface:Device", "info": {"props": {"device.api": "alsa"}}},
            {"id": 50, "type": "PipeWire:Interface:Node", "info": {"props": {"media.class": "Audio/Sink", "node.name": "alsa_output.hdmi", "node.description": "HDMI", "device.id": 48}}}
        ]);
        assert_eq!(sound_text(&wired, None), "The sound plays on HDMI; nothing looks wrong with it.");
    }
}
