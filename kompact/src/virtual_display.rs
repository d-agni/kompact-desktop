//! Virtual displays (Phase 22): a DRM device driven by VKMS, the kernel's virtual display
//! driver, becomes an output like a monitor. Tools such as demirror plug one in to give the
//! desktop an extra monitor that is shown elsewhere (a tablet, a phone, another PC).
//!
//! A VKMS device has no GPU. The GT 710 draws the output's area off screen (the screenshot
//! way, `screenshot::capture_with`, read back as BGRA), the pixels are copied into one of two
//! dumb buffers on the virtual device, and the virtual CRTC flips to it. The GT 710's own KMS
//! state is never touched for a virtual display, and an error on one only shuts that output
//! down.
//!
//! This module is the pure part (22a): which devices are virtual displays, when a frame is
//! drawn, the two buffers, the output's name, copying a frame into a buffer. `backend_udev.rs`
//! does the KMS calls (22b). `KOMPACT_VIRTUAL_DISPLAYS=off` leaves virtual displays unused.

use std::time::{Duration, Instant};

use smithay::utils::Rectangle;

/// `KOMPACT_VIRTUAL_DISPLAYS`: on unless `off` (or `0`, `no`, `false`).
pub fn enabled_from_env(value: Option<&str>) -> bool {
    crate::screenshot::enabled_from_env(value)
}

/// Whether a DRM device with this driver name (`drmGetVersion`) is a virtual display.
pub fn is_virtual_driver(name: &str) -> bool {
    name == "vkms"
}

/// At most this many frames a second on a virtual display. Each one is a read back on the
/// main loop (~16 ms for 1440x900 on the GT 710, 9e-a2), so this keeps the monitors smooth.
pub const MAX_FPS: u32 = 30;

/// When a virtual display's next frame is drawn: the first one always; after that only when
/// something on it changed, and not sooner than `1 / max_fps` after the last one. (The flip
/// must also be done, see `Swap`.)
#[derive(Debug)]
pub struct Pacer {
    interval: Duration,
    last: Option<Instant>,
    /// The last frame couldn't be shown: the next is drawn after the interval, changed or not.
    missed: bool,
}

impl Pacer {
    pub fn new(max_fps: u32) -> Pacer {
        Pacer { interval: Duration::from_secs(1) / max_fps.max(1), last: None, missed: false }
    }

    /// `changed` is only asked once the interval has passed (finding out costs building the
    /// output's elements).
    pub fn due(&self, now: Instant, changed: impl FnOnce() -> bool) -> bool {
        match self.last {
            None => true,
            Some(last) => now.duration_since(last) >= self.interval && (self.missed || changed()),
        }
    }

    pub fn drawn(&mut self, now: Instant) {
        self.last = Some(now);
        self.missed = false;
    }

    /// Drawn but not shown (the seat was busy): try again after the interval, not sooner.
    pub fn missed(&mut self, now: Instant) {
        self.last = Some(now);
        self.missed = true;
    }

    /// The next frame is drawn whatever changed (VT return, a new mode).
    pub fn reset(&mut self) {
        self.last = None;
    }
}

/// The two dumb buffers of a virtual display: a frame goes into the back one, the CRTC flips to
/// it, and no frame is drawn until the kernel says the flip is done.
#[derive(Debug, Default)]
pub struct Swap {
    front: Option<usize>,
    flipping_to: Option<usize>,
}

impl Swap {
    /// The buffer to draw the next frame into, or `None` while a flip is pending.
    pub fn back(&self) -> Option<usize> {
        if self.flipping_to.is_some() {
            return None;
        }
        Some(if self.front == Some(0) { 1 } else { 0 })
    }

    /// Shown straight away (the first frame, set with the mode; no flip event comes).
    pub fn shown(&mut self, buffer: usize) {
        self.front = Some(buffer);
        self.flipping_to = None;
    }

    /// A page flip to `buffer` was queued.
    pub fn flipping(&mut self, buffer: usize) {
        self.flipping_to = Some(buffer);
    }

    /// The kernel's page-flip event: the buffer flipped to is now on screen.
    pub fn flip_done(&mut self) {
        if let Some(buffer) = self.flipping_to.take() {
            self.front = Some(buffer);
        }
    }

    /// Nothing is pending any more (VT switch, an error): draw again when due.
    pub fn forget_flip(&mut self) {
        self.flipping_to = None;
    }

    pub fn is_shown(&self) -> bool {
        self.front.is_some()
    }
}

/// The output name for a virtual display's connector: its own name (`Virtual-1`), unless an
/// output already has it (each VKMS device numbers its connectors from 1), then the next free
/// number (`Virtual-2`).
pub fn unique_name(name: &str, taken: &[String]) -> String {
    if !taken.iter().any(|t| t == name) {
        return name.to_string();
    }
    let (prefix, first) = match name.rsplit_once('-') {
        Some((prefix, n)) if n.parse::<u32>().is_ok() => (prefix, n.parse::<u32>().unwrap_or(1)),
        _ => (name, 1),
    };
    (first + 1..)
        .map(|n| format!("{prefix}-{n}"))
        .find(|candidate| !taken.contains(candidate))
        .expect("an unused number exists")
}

/// Whether a `w` x `h` XRGB8888 frame fits a buffer of `len` bytes with `pitch` bytes a row.
pub fn fits(w: i32, h: i32, pitch: usize, len: usize) -> bool {
    w > 0 && h > 0 && pitch >= w as usize * 4 && pitch.saturating_mul(h as usize) <= len
}

/// Copy a frame read back as BGRA (B, G, R, A: XRGB8888's byte order already), `w` x `h`, top
/// row first, into a dumb buffer with `pitch` bytes a row. `false` (nothing copied) if it
/// doesn't fit: the mode changed meanwhile.
pub fn copy_frame(bgra: &[u8], w: i32, h: i32, dst: &mut [u8], pitch: usize) -> bool {
    if bgra.len() < (w.max(0) as usize) * (h.max(0) as usize) * 4 || !fits(w, h, pitch, dst.len()) {
        return false;
    }
    crate::screencopy::copy_region_rows(bgra, w, Rectangle::new((0, 0).into(), (w, h).into()), dst, pitch);
    true
}

/// `virtual-mode`'s `<w>x<h>` (Settings → Displays), within what VKMS offers.
pub fn parse_mode(text: &str) -> Option<(u32, u32)> {
    let (w, h) = text.split_once('x')?;
    let (w, h) = (w.parse().ok()?, h.parse().ok()?);
    ((320..=8192).contains(&w) && (200..=8192).contains(&h)).then_some((w, h))
}

/// Which of a connector's modes (`sizes[i]`, in the kernel's order) to use: the one Settings
/// asked for if the device has it, else the device's preferred one.
pub fn choose_mode(sizes: &[(u32, u32)], preferred: usize, wanted: Option<(u32, u32)>) -> usize {
    wanted.and_then(|w| sizes.iter().position(|s| *s == w)).unwrap_or(preferred)
}

/// 22d: where tools that plug virtual displays in say the size one is viewed at:
/// `<dir>/<device name>` holds `<w>x<h>` (demirror writes it; VKMS can't advertise a
/// preferred mode itself). The device name is the one sysfs gives the DRM device.
pub const MODE_HINT_DIR: &str = "/run/vkms-mode";

/// 22e: input hints: `<dir>/<device name>` lists (one a line) the input devices that point at
/// that virtual display (demirror's pointer for a viewer): their absolute positions are
/// positions on it, not on the output under the pointer.
pub const INPUT_HINT_DIR: &str = "/run/vkms-input";

/// 22e: an input hint file's device names.
pub fn parse_input_hint(text: &str) -> Vec<String> {
    text.lines().map(str::trim).filter(|l| !l.is_empty() && l.len() <= 200).map(str::to_string).collect()
}

/// 22d: a mode hint file's contents.
pub fn parse_hint(text: &str) -> Option<(u32, u32)> {
    parse_mode(text.trim())
}

/// 22d: the offered mode that best fills a viewer of `wanted` size once scaled to fit it:
/// the most of the viewer's screen used, then one that fits without shrinking, then the
/// largest (sharpest). The same size, when offered, always wins.
pub fn best_fit(sizes: &[(u32, u32)], wanted: (u32, u32)) -> Option<usize> {
    let (ww, wh) = (wanted.0.max(1) as f64, wanted.1.max(1) as f64);
    let score = |&(w, h): &(u32, u32)| {
        let (mw, mh) = (w.max(1) as f64, h.max(1) as f64);
        let scale = (ww / mw).min(wh / mh);
        let used = (mw * scale * mh * scale) / (ww * wh);
        let fits = w <= wanted.0 && h <= wanted.1;
        let area = w as i64 * h as i64;
        ((used * 100.0).round() as i64, fits, if fits { area } else { -area })
    };
    (0..sizes.len()).max_by_key(|&i| (sizes[i] == wanted, score(&sizes[i])))
}

/// 22d: a virtual display's mode: a resolution chosen in Settings if the device offers it,
/// else the best fit for the size it's viewed at (mode hint), else the device's preferred one.
pub fn pick_mode(sizes: &[(u32, u32)], preferred: usize, setting: Option<(u32, u32)>, hint: Option<(u32, u32)>) -> usize {
    if setting.is_some() {
        return choose_mode(sizes, preferred, setting);
    }
    hint.and_then(|h| best_fit(sizes, h)).unwrap_or(preferred)
}

/// 22f: a size the device doesn't offer, for a mode made to measure: the viewer's size
/// (mode hint), unless Settings chose a resolution or the device has that size anyway. VKMS
/// takes any mode up to 8192x8192, so a phone held upright gets a tall display that fills it,
/// rather than the closest of VKMS's fixed (all landscape) sizes.
pub fn custom_size(sizes: &[(u32, u32)], setting: Option<(u32, u32)>, hint: Option<(u32, u32)>) -> Option<(u32, u32)> {
    let hint = hint?;
    (setting.is_none() && !sizes.contains(&hint)).then_some(hint)
}

/// 22f: timings for a mode of `w`x`h` at 60 Hz, the CVT reduced-blanking way (a monitor would
/// need them right; VKMS only uses them for its vblank timer): `(hsync_start, hsync_end, htotal,
/// vsync_start, vsync_end, vtotal, clock in kHz)`.
pub fn timings(w: u32, h: u32) -> (u32, u32, u32, u32, u32, u32, u32) {
    let (htotal, vblank) = (w + 160, (h / 20).max(25));
    let vtotal = h + vblank;
    let clock = (htotal as u64 * vtotal as u64 * 60).div_ceil(1000) as u32;
    (w + 48, w + 80, htotal, h + 3, h + 9, vtotal, clock)
}

/// The sizes Settings offers for a virtual display: each once, largest first.
pub fn offered_sizes(sizes: &[(u32, u32)]) -> Vec<(u32, u32)> {
    let mut list: Vec<(u32, u32)> = Vec::new();
    for s in sizes {
        if !list.contains(s) {
            list.push(*s);
        }
    }
    list.sort_by_key(|(w, h)| std::cmp::Reverse((*w as u64 * *h as u64, *w)));
    list
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn which_devices_are_virtual_displays() {
        assert!(is_virtual_driver("vkms"));
        assert!(!is_virtual_driver("nouveau"));
        assert!(!is_virtual_driver("simpledrm"));
        assert!(enabled_from_env(None));
        assert!(enabled_from_env(Some("on")));
        assert!(!enabled_from_env(Some("off")));
        assert!(!enabled_from_env(Some(" OFF ")));
    }

    #[test]
    fn frames_only_when_changed_and_not_too_often() {
        let start = Instant::now();
        let mut pacer = Pacer::new(MAX_FPS);
        assert!(pacer.due(start, || false), "the first frame always");
        pacer.drawn(start);

        let soon = start + Duration::from_millis(10);
        let mut asked = false;
        assert!(!pacer.due(soon, || {
            asked = true;
            true
        }));
        assert!(!asked, "changes aren't looked for before the interval");

        let later = start + Duration::from_millis(40);
        assert!(!pacer.due(later, || false), "nothing changed");
        assert!(pacer.due(later, || true));

        pacer.missed(later);
        assert!(!pacer.due(later + Duration::from_millis(10), || true), "a missed frame waits the interval");
        assert!(pacer.due(later + Duration::from_millis(40), || false), "then comes again unchanged");
        pacer.drawn(later + Duration::from_millis(40));
        assert!(!pacer.due(later + Duration::from_millis(80), || false));

        pacer.reset();
        assert!(pacer.due(soon, || false), "after a reset, whatever changed");
        assert_eq!(Pacer::new(0).interval, Duration::from_secs(1), "0 fps counts as 1");
    }

    #[test]
    fn two_buffers_take_turns() {
        let mut swap = Swap::default();
        assert!(!swap.is_shown());
        assert_eq!(swap.back(), Some(0));
        swap.shown(0);
        assert!(swap.is_shown());
        assert_eq!(swap.back(), Some(1));

        swap.flipping(1);
        assert_eq!(swap.back(), None, "nothing drawn while a flip is pending");
        swap.flip_done();
        assert_eq!(swap.back(), Some(0));

        swap.flipping(0);
        swap.forget_flip();
        assert_eq!(swap.back(), Some(0), "a forgotten flip leaves buffer 1 in front");
        swap.flip_done();
        assert_eq!(swap.back(), Some(0), "a late event changes nothing");
    }

    #[test]
    fn names_stay_unique() {
        let taken = |names: &[&str]| names.iter().map(|n| n.to_string()).collect::<Vec<_>>();
        assert_eq!(unique_name("Virtual-1", &taken(&["VGA-1"])), "Virtual-1");
        assert_eq!(unique_name("Virtual-1", &taken(&["VGA-1", "Virtual-1"])), "Virtual-2");
        assert_eq!(unique_name("Virtual-1", &taken(&["Virtual-1", "Virtual-2"])), "Virtual-3");
        assert_eq!(unique_name("Virtual", &taken(&["Virtual"])), "Virtual-2");
    }

    #[test]
    fn resolutions() {
        assert_eq!(parse_mode("1920x1080"), Some((1920, 1080)));
        assert_eq!(parse_mode("1920X1080"), None);
        assert_eq!(parse_mode("10x10"), None);
        assert_eq!(parse_mode("1920x"), None);

        let sizes = [(1024, 768), (4096, 2160), (4096, 2160), (1920, 1080), (800, 600)];
        assert_eq!(choose_mode(&sizes, 0, None), 0, "auto: the preferred one");
        assert_eq!(choose_mode(&sizes, 0, Some((1920, 1080))), 3);
        assert_eq!(choose_mode(&sizes, 0, Some((1366, 768))), 0, "not offered: the preferred one");
        assert_eq!(offered_sizes(&sizes), vec![(4096, 2160), (1920, 1080), (1024, 768), (800, 600)]);
    }

    #[test]
    fn modes_for_the_viewer() {
        let sizes = [(1024, 768), (4096, 2160), (1920, 1080), (1280, 720), (1600, 1200), (2560, 1600), (800, 600)];
        assert_eq!(best_fit(&sizes, (1920, 1080)), Some(2), "offered: that one");
        assert_eq!(best_fit(&sizes, (2560, 1440)), Some(2), "16:9 that fits, the largest");
        assert_eq!(best_fit(&sizes, (2560, 1600)), Some(5));
        assert_eq!(best_fit(&sizes, (1366, 768)), Some(3), "16:9 fits: 1280x720 over 1024x768");
        assert_eq!(best_fit(&sizes, (2048, 1536)), Some(4), "4:3: the largest 4:3 that fits");
        assert_eq!(best_fit(&sizes, (640, 480)), Some(6), "nothing fits: the closest shape, smallest");
        assert_eq!(best_fit(&[], (640, 480)), None);
        assert_eq!(parse_hint("1920x1080\n"), Some((1920, 1080)));
        assert_eq!(parse_hint("big"), None);
        assert_eq!(parse_input_hint("demirror-3 pointer\n\n  tablet  \n"), vec!["demirror-3 pointer", "tablet"]);
        assert!(parse_input_hint("").is_empty());

        assert_eq!(pick_mode(&sizes, 0, None, None), 0, "nothing said: the preferred one");
        assert_eq!(pick_mode(&sizes, 0, None, Some((1920, 1080))), 2, "the viewer's size");
        assert_eq!(pick_mode(&sizes, 0, Some((1280, 720)), Some((1920, 1080))), 3, "Settings wins");
        assert_eq!(pick_mode(&sizes, 0, Some((1366, 768)), Some((1920, 1080))), 0, "Settings' size not offered");
        assert_eq!(custom_size(&sizes, None, Some((1080, 2400))), Some((1080, 2400)), "a phone held upright");
        assert_eq!(custom_size(&sizes, None, Some((1920, 1080))), None, "offered: that mode");
        assert_eq!(custom_size(&sizes, Some((1280, 720)), Some((1080, 2400))), None, "Settings wins");
        assert_eq!(custom_size(&sizes, None, None), None);
    }

    #[test]
    fn copying_a_frame_into_a_buffer() {
        // 2x2 frame, buffer rows padded to 12 bytes.
        let bgra: Vec<u8> = (0..16).collect();
        let mut dst = vec![0xaa; 24];
        assert!(copy_frame(&bgra, 2, 2, &mut dst, 12));
        assert_eq!(&dst[..8], &bgra[..8]);
        assert_eq!(&dst[8..12], &[0xaa; 4], "padding untouched");
        assert_eq!(&dst[12..20], &bgra[8..16]);

        assert!(!copy_frame(&bgra, 2, 2, &mut [0; 20], 12), "too small for the pitch");
        assert!(!copy_frame(&bgra, 2, 2, &mut [0; 24], 4), "pitch shorter than a row");
        assert!(!copy_frame(&bgra[..8], 2, 2, &mut [0; 24], 12), "frame shorter than its size");
        assert!(!copy_frame(&[], 0, 0, &mut [0; 24], 12));
    }

    #[test]
    fn made_to_measure_modes_run_at_60_hz() {
        for (w, h) in [(1080, 2400), (1920, 864), (8192, 8192), (320, 200)] {
            let (hs, he, ht, vs, ve, vt, clock) = timings(w, h);
            assert!(w < hs && hs < he && he < ht && h < vs && vs < ve && ve < vt, "{w}x{h}");
            assert!(ht <= u16::MAX as u32 && vt <= u16::MAX as u32, "{w}x{h} fits drm_mode_modeinfo");
            let hz = clock as f64 * 1000.0 / (ht as f64 * vt as f64);
            assert!((59.9..60.1).contains(&hz), "{w}x{h}: {hz} Hz");
        }
    }
}
