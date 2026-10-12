//! Periodic resource stats (Phase 3b): one log line every `KOMPACT_STATS_SECS` seconds (default
//! 60, `0` turns it off), so a long session or soak test shows whether memory and file
//! descriptors plateau or keep climbing.
//!
//! RSS comes from `/proc/self/statm`; file descriptors are counted by kind from `/proc/self/fd`.
//! `dmabuf` fds are GPU buffers shared with clients or the GBM swapchain, so a buffer-pool leak
//! (Phase 3d) shows up there. The parsing is pure and covered by `cargo test`.

use std::time::Duration;

pub const DEFAULT_INTERVAL: Duration = Duration::from_secs(60);

/// Interval from `KOMPACT_STATS_SECS`, or the default. `None` = disabled.
pub fn interval_from_env(value: Option<&str>) -> Option<Duration> {
    match value.map(str::trim) {
        None | Some("") => Some(DEFAULT_INTERVAL),
        Some(v) => match v.parse::<u64>() {
            Ok(0) => None,
            Ok(secs) => Some(Duration::from_secs(secs)),
            Err(_) => Some(DEFAULT_INTERVAL),
        },
    }
}

/// Resident set size in KiB from the contents of `/proc/<pid>/statm` (second field, in pages).
pub fn rss_kib_from_statm(statm: &str, page_size: u64) -> Option<u64> {
    let resident: u64 = statm.split_whitespace().nth(1)?.parse().ok()?;
    Some(resident * page_size / 1024)
}

#[derive(Debug, Default, PartialEq, Eq, Clone, Copy)]
pub struct FdCounts {
    pub total: usize,
    pub dmabuf: usize,
    pub dri: usize,
    pub socket: usize,
    pub other: usize,
}

/// Classify the targets of `/proc/self/fd/*` symlinks.
pub fn count_fds<'a>(targets: impl IntoIterator<Item = &'a str>) -> FdCounts {
    let mut c = FdCounts::default();
    for t in targets {
        c.total += 1;
        if t.starts_with("anon_inode:dmabuf") || t.starts_with("/dmabuf:") {
            c.dmabuf += 1;
        } else if t.starts_with("/dev/dri/") {
            c.dri += 1;
        } else if t.starts_with("socket:") {
            c.socket += 1;
        } else {
            c.other += 1;
        }
    }
    c
}

/// Window/surface counts gathered by the caller from compositor state.
#[derive(Debug, Default, Clone, Copy)]
pub struct Counts {
    pub visible: usize,
    pub parked: usize,
    pub layers: usize,
    pub popups: usize,
    pub workspace: usize,
}

/// The log line. Stable `key=value` format so `tools/soak.sh` (or grep/awk) can pick it apart.
/// `timing` (render passes in the last second, from the debug overlay) is appended only while
/// the overlay is on, since passes are only timed then. `frames` (page-flip timing, Phase 4a) is
/// appended on the DRM backend whenever anything was rendered in the interval.
pub fn format_line(
    c: Counts,
    rss_kib: Option<u64>,
    fds: Option<FdCounts>,
    timing: Option<&crate::overlay::Timing>,
    frames: Option<&crate::perf::Summary>,
) -> String {
    let rss = rss_kib.map_or("?".to_string(), |k| k.to_string());
    let fd = fds.map_or("fds=?".to_string(), |f| {
        format!(
            "fds={} dmabuf={} dri={} sockets={} other_fds={}",
            f.total, f.dmabuf, f.dri, f.socket, f.other
        )
    });
    let ms = |d: Duration| d.as_secs_f64() * 1000.0;
    let timing = timing.map_or(String::new(), |t| {
        format!(
            " passes={} gap_ms={:.1} build_ms={:.2} build_max_ms={:.2}",
            t.passes,
            ms(t.max_gap),
            ms(t.avg_build),
            ms(t.max_build)
        )
    });
    let frames = frames.map_or(String::new(), crate::perf::Summary::fragment);
    format!(
        "stats: rss_kib={rss} {fd} windows={} parked={} popups={} layers={} workspace={}{timing}{frames}",
        c.visible,
        c.parked,
        c.popups,
        c.layers,
        c.workspace + 1
    )
}

/// Read RSS and fd counts for this process (Linux /proc). `None` where /proc isn't readable.
pub fn sample_self() -> (Option<u64>, Option<FdCounts>) {
    let page = page_size(); // statm counts pages
    let rss = std::fs::read_to_string("/proc/self/statm")
        .ok()
        .and_then(|s| rss_kib_from_statm(&s, page));
    let fds = std::fs::read_dir("/proc/self/fd").ok().map(|dir| {
        let targets: Vec<String> = dir
            .filter_map(|e| e.ok())
            .filter_map(|e| std::fs::read_link(e.path()).ok())
            .map(|p| p.to_string_lossy().into_owned())
            .collect();
        count_fds(targets.iter().map(String::as_str))
    });
    (rss, fds)
}

/// Page size from the auxiliary vector exposed in /proc/self/auxv (AT_PAGESZ = 6), falling back
/// to 4 KiB.
fn page_size() -> u64 {
    const AT_PAGESZ: u64 = 6;
    std::fs::read("/proc/self/auxv")
        .ok()
        .and_then(|bytes| {
            bytes.chunks_exact(16).find_map(|kv| {
                let k = u64::from_ne_bytes(kv[..8].try_into().ok()?);
                let v = u64::from_ne_bytes(kv[8..].try_into().ok()?);
                (k == AT_PAGESZ).then_some(v)
            })
        })
        .filter(|&v| v > 0)
        .unwrap_or(4096)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interval_parsing() {
        assert_eq!(interval_from_env(None), Some(DEFAULT_INTERVAL));
        assert_eq!(interval_from_env(Some("")), Some(DEFAULT_INTERVAL));
        assert_eq!(interval_from_env(Some("10")), Some(Duration::from_secs(10)));
        assert_eq!(interval_from_env(Some(" 5 ")), Some(Duration::from_secs(5)));
        assert_eq!(interval_from_env(Some("0")), None);
        assert_eq!(interval_from_env(Some("abc")), Some(DEFAULT_INTERVAL));
    }

    #[test]
    fn statm_rss() {
        // size resident shared text lib data dt
        assert_eq!(rss_kib_from_statm("51200 12800 3000 100 0 9000 0\n", 4096), Some(51200));
        assert_eq!(rss_kib_from_statm("garbage", 4096), None);
        assert_eq!(rss_kib_from_statm("", 4096), None);
    }

    #[test]
    fn fd_classification() {
        let c = count_fds([
            "anon_inode:dmabuf",
            "/dmabuf:",
            "/dev/dri/card0",
            "/dev/dri/renderD128",
            "socket:[12345]",
            "socket:[12346]",
            "/dev/null",
            "anon_inode:[eventfd]",
        ]);
        assert_eq!(c, FdCounts { total: 8, dmabuf: 2, dri: 2, socket: 2, other: 2 });
    }

    #[test]
    fn line_format_is_stable() {
        let line = format_line(
            Counts { visible: 3, parked: 2, layers: 1, popups: 0, workspace: 1 },
            Some(51200),
            Some(FdCounts { total: 40, dmabuf: 6, dri: 1, socket: 20, other: 13 }),
            None,
            None,
        );
        assert_eq!(
            line,
            "stats: rss_kib=51200 fds=40 dmabuf=6 dri=1 sockets=20 other_fds=13 windows=3 parked=2 popups=0 layers=1 workspace=2"
        );
        let unknown = format_line(Counts::default(), None, None, None, None);
        assert!(unknown.starts_with("stats: rss_kib=? fds=? windows=0"));
    }

    #[test]
    fn timing_appended_when_overlay_on() {
        let t = crate::overlay::Timing {
            passes: 60,
            max_gap: Duration::from_micros(16_700),
            avg_build: Duration::from_micros(310),
            max_build: Duration::from_micros(920),
        };
        let line = format_line(Counts::default(), Some(1), None, Some(&t), None);
        assert!(line.ends_with(" passes=60 gap_ms=16.7 build_ms=0.31 build_max_ms=0.92"), "{line}");
        let frames = crate::perf::Summary {
            renders: 61,
            empty: 61,
            flips: 0,
            missed: 0,
            no_redraw: 0,
            cpu: None,
            wait: None,
            frame: None,
            key: None,
            pointer: None,
        };
        let line = format_line(Counts::default(), Some(1), None, Some(&t), Some(&frames));
        assert!(
            line.ends_with(" build_max_ms=0.92 renders=61 empty=61 flips=0 missed=0 cpu_ms=- wait_ms=- frame_ms=- key_ms=- ptr_ms=- no_redraw=0"),
            "{line}"
        );
    }

    #[test]
    fn sampling_this_process_works() {
        let (rss, fds) = sample_self();
        assert!(rss.unwrap() > 0);
        assert!(fds.unwrap().total > 0);
        assert!(page_size() >= 4096);
    }
}
