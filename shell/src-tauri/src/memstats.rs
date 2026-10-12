//! 6m-d5: how much memory the shell really uses. Most of it isn't in kompact-shell itself but in
//! the WebKit processes it starts (one web process per page, plus a network process), which no
//! log showed. Every `KOMPACT_SHELL_STATS_SECS` (default 60, 0 = off) this logs the resident
//! memory of kompact-shell and its own helper processes, grouped by name, and apart from that
//! the apps launched from the shell (the dock, Launchpad), each with all its processes. Those
//! are our children too, but not the shell's cost (6e: Firefox and friends made a 2.9 GB "shell"):
//!
//! `shell stats: rss_kib shell=… kompact-shell=… WebKitWebProces=…(4) …; apps=…(2) firefox=… nautilus=…`
//!
//! (Names are the kernel's, cut at 15 characters.)

use std::collections::BTreeMap;

use tracing::info;

pub fn interval_from_env(value: Option<&str>) -> u32 {
    value.and_then(|v| v.trim().parse().ok()).unwrap_or(60)
}

/// `(pid, name, ppid)` from a `/proc/<pid>/stat` line. The name is in parentheses and may itself
/// contain spaces or parentheses, so it ends at the last `)`.
pub fn parse_stat(stat: &str) -> Option<(u32, String, u32)> {
    let open = stat.find('(')?;
    let close = stat.rfind(')')?;
    let pid = stat[..open].trim().parse().ok()?;
    let name = stat.get(open + 1..close)?.to_string();
    let mut rest = stat.get(close + 1..)?.split_whitespace();
    let _state = rest.next()?;
    let ppid = rest.next()?.parse().ok()?;
    Some((pid, name, ppid))
}

/// `VmRSS` in KiB from `/proc/<pid>/status` (absent for kernel threads and zombies).
pub fn parse_rss_kib(status: &str) -> Option<u64> {
    let line = status.lines().find(|l| l.starts_with("VmRSS:"))?;
    line.split_whitespace().nth(1)?.parse().ok()
}

/// `root` and every process below it, from `(pid, ppid)` pairs.
pub fn descendants(root: u32, procs: &[(u32, u32)]) -> Vec<u32> {
    let mut out = vec![root];
    let mut i = 0;
    while i < out.len() {
        let parent = out[i];
        out.extend(procs.iter().filter(|(_, pp)| *pp == parent).map(|(p, _)| *p));
        i += 1;
    }
    out
}

/// Processes that are part of the shell: itself, WebKit's (web pages, network) and what those
/// start (the bubblewrap sandbox and its D-Bus proxy, glycin image decoders).
pub fn is_shell_part(name: &str) -> bool {
    name == "kompact-shell"
        || name.starts_with("WebKit")
        || name.starts_with("bwrap")
        || name.starts_with("glycin")
        || name.starts_with("xdg-dbus-proxy")
}

/// From `root` (kompact-shell) down: the shell's own processes, and each launched app's root
/// with every process below it. `procs` are `(pid, name, ppid)`.
pub fn split(root: u32, procs: &[(u32, String, u32)]) -> (Vec<u32>, Vec<(u32, Vec<u32>)>) {
    let children = |p: u32| procs.iter().filter(move |(_, _, pp)| *pp == p).map(|(c, n, _)| (*c, n.as_str()));
    let pairs: Vec<(u32, u32)> = procs.iter().map(|(p, _, pp)| (*p, *pp)).collect();
    let mut shell = vec![root];
    let mut apps = Vec::new();
    let mut i = 0;
    while i < shell.len() {
        for (child, name) in children(shell[i]) {
            if is_shell_part(name) {
                shell.push(child);
            } else {
                apps.push((child, descendants(child, &pairs)));
            }
        }
        i += 1;
    }
    (shell, apps)
}

/// The log line: `shell` is `(name, rss_kib)` per process, `apps` `(name, rss_kib)` per app (all
/// its processes together).
pub fn format_line(shell: &[(String, u64)], apps: &[(String, u64)]) -> String {
    let group = |list: &[(String, u64)]| {
        let mut by_name: BTreeMap<String, (u64, usize)> = BTreeMap::new();
        for (name, kib) in list {
            let e = by_name.entry(name.clone()).or_default();
            e.0 += kib;
            e.1 += 1;
        }
        by_name
            .into_iter()
            .map(|(name, (kib, n))| if n == 1 { format!(" {name}={kib}") } else { format!(" {name}={kib}({n})") })
            .collect::<String>()
    };
    let total = |list: &[(String, u64)]| list.iter().map(|(_, k)| k).sum::<u64>();
    let mut line = format!("shell stats: rss_kib shell={}{}", total(shell), group(shell));
    if !apps.is_empty() {
        line.push_str(&format!("; apps={}({}){}", total(apps), apps.len(), group(apps)));
    }
    line
}

fn sample() -> String {
    let mut all = Vec::new();
    if let Ok(dir) = std::fs::read_dir("/proc") {
        for entry in dir.flatten() {
            let Some(pid) = entry.file_name().to_str().and_then(|s| s.parse::<u32>().ok()) else { continue };
            if let Some(p) = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok().as_deref().and_then(parse_stat) {
                all.push(p);
            }
        }
    }
    let rss = |pid: u32| parse_rss_kib(&std::fs::read_to_string(format!("/proc/{pid}/status")).ok()?);
    let name = |pid: u32| all.iter().find(|(p, _, _)| *p == pid).map(|(_, n, _)| n.clone()).unwrap_or_default();
    let (shell, apps) = split(std::process::id(), &all);
    let shell: Vec<(String, u64)> = shell.into_iter().filter_map(|pid| Some((name(pid), rss(pid)?))).collect();
    let apps: Vec<(String, u64)> = apps
        .into_iter()
        .map(|(root, pids)| (name(root), pids.into_iter().filter_map(rss).sum()))
        .collect();
    format_line(&shell, &apps)
}

/// Log a line now (a few seconds after start, once the pages are up) and then every interval.
pub fn start() {
    let secs = interval_from_env(std::env::var("KOMPACT_SHELL_STATS_SECS").ok().as_deref());
    if secs == 0 {
        return;
    }
    gtk::glib::timeout_add_seconds_local_once(10, move || {
        info!("{}", sample());
        gtk::glib::timeout_add_seconds_local(secs, || {
            info!("{}", sample());
            gtk::glib::ControlFlow::Continue
        });
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stat_lines() {
        assert_eq!(parse_stat("4242 (kompact-shell) S 1 4242 4242 0"), Some((4242, "kompact-shell".into(), 1)));
        assert_eq!(parse_stat("77 (Web (Content)) S 4242 1"), Some((77, "Web (Content)".into(), 4242)));
        assert_eq!(parse_stat("garbage"), None);
    }

    #[test]
    fn rss() {
        assert_eq!(parse_rss_kib("Name:\tx\nVmPeak:\t 10 kB\nVmRSS:\t  81388 kB\n"), Some(81388));
        assert_eq!(parse_rss_kib("Name:\tkthreadd\n"), None);
    }

    #[test]
    fn tree() {
        let procs = [(10, 1), (11, 10), (12, 10), (13, 11), (20, 1)];
        let mut d = descendants(10, &procs);
        d.sort();
        assert_eq!(d, [10, 11, 12, 13]);
    }

    #[test]
    fn apps_are_not_the_shell() {
        let p = |pid, name: &str, ppid| (pid, name.to_string(), ppid);
        let procs = [
            p(10, "kompact-shell", 1),
            p(11, "WebKitWebProces", 10),
            p(12, "bwrap", 10),
            p(13, "WebKitNetworkPr", 12),
            p(20, "firefox", 10),
            p(21, "Web Content", 20),
            p(22, "bwrap", 20), // an app's own sandbox counts as the app
            p(30, "nautilus", 10),
            p(40, "unrelated", 1),
        ];
        let (shell, apps) = split(10, &procs);
        assert_eq!(shell, [10, 11, 12, 13]);
        assert_eq!(apps.len(), 2);
        let (root, mut pids) = apps[0].clone();
        pids.sort();
        assert_eq!((root, pids), (20, vec![20, 21, 22]));
        assert_eq!(apps[1], (30, vec![30]));
    }

    #[test]
    fn line() {
        let shell = [
            ("kompact-shell".to_string(), 100),
            ("WebKitWebProces".to_string(), 50),
            ("WebKitWebProces".to_string(), 70),
        ];
        assert_eq!(format_line(&shell, &[]), "shell stats: rss_kib shell=220 WebKitWebProces=120(2) kompact-shell=100");
        let apps = [("firefox".to_string(), 900), ("nautilus".to_string(), 300)];
        assert_eq!(
            format_line(&shell, &apps),
            "shell stats: rss_kib shell=220 WebKitWebProces=120(2) kompact-shell=100; apps=1200(2) firefox=900 nautilus=300"
        );
        assert_eq!(interval_from_env(None), 60);
        assert_eq!(interval_from_env(Some("0")), 0);
    }
}
