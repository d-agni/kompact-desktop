//! Frame timing on the DRM backend (Phase 4a). Measurement only: nothing here changes when or
//! how frames are drawn.
//!
//! Per render pass the backend reports how long kompact spent on it (building the elements
//! through `queue_frame`) and whether anything was actually queued for a page flip; per VBlank it
//! reports when the flip completed, with the kernel's own timestamp and sequence number. From
//! that, every stats interval:
//!
//! - `cpu_ms`: compositor time per queued frame,
//! - `wait_ms`: `queue_frame` to its flip completing (≤ one refresh unless the GPU is late),
//! - `frame_ms`: time between consecutive flips while rendering back to back (16.7 ms = 60 fps),
//!   from the kernel timestamps; idle gaps between unrelated frames are left out,
//! - `missed`: vblanks skipped between back-to-back flips, from the kernel sequence numbers,
//! - `key_ms` / `ptr_ms` (Phase 4b): input-to-photon latency, from a key press / pointer event
//!   (motion, button, scroll) to the flip of the first frame queued after it. The kernel stamps
//!   both ends with CLOCK_MONOTONIC (libinput event time, VBlank time), so this includes the time
//!   the event spent queued before kompact read it. It's the latency of *kompact's* side: a key
//!   press whose app hasn't redrawn yet by the next frame (while something else is animating) is
//!   counted at that earlier frame, so `key_ms` is a lower bound when other things are moving.
//!   Modifier keys are left out (they don't change the screen, so they'd be pinned on whatever
//!   frame happens next, seconds later). Likewise any input followed by no frame at all for
//!   `NO_REDRAW_AFTER` (a click on the bare desktop, Escape at an empty prompt) is counted as
//!   `no_redraw=` instead of as a latency sample. A real stall still shows, in `frame_ms`/`missed`.
//!
//! Popups (Phase 4c) get one log line each when they first reach the screen: the time from the
//! last button press (if within 2 s), the client's part (popup created to its first buffer:
//! configure round trip + drawing) and kompact's part (first buffer to the flip showing it).
//!
//! Samples go into fixed-capacity buffers that are reused every interval, so this never grows.

use std::time::{Duration, Instant};

/// Samples kept per kind per interval (about a minute at 60 fps); counts keep going past it.
pub const MAX_SAMPLES: usize = 4096;

/// An input with no frame queued this long after it changed nothing on screen.
pub const NO_REDRAW_AFTER: Duration = Duration::from_millis(500);

/// Assumed refresh period until the backend sets the real one.
const DEFAULT_REFRESH: Duration = Duration::from_micros(16_667);

/// The kernel's data for one completed page flip (from Smithay's `DrmEventMetadata`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VBlank {
    pub time: Duration,
    /// `time` is CLOCK_MONOTONIC (comparable with input event times), not realtime.
    pub monotonic: bool,
    pub sequence: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputKind {
    Key,
    Pointer,
}

/// An input event that hasn't reached the screen yet.
#[derive(Debug, Clone, Copy)]
struct Input {
    received: Instant,
    /// The event's own timestamp (libinput: CLOCK_MONOTONIC, microseconds).
    event_us: Option<u64>,
}

/// A popup on its way to the screen (Phase 4c).
#[derive(Debug, Clone, Copy)]
struct PendingPopup {
    key: u64,
    created: Instant,
    click: Option<Input>,
    buffer: Option<Instant>,
    /// Its buffer is in the frame waiting for a flip.
    in_frame: bool,
}

/// Popups tracked at once; one that never gets a buffer is dropped when a new one needs room.
pub const MAX_PENDING_POPUPS: usize = 16;

/// A button press counts as what opened a popup if it was this recent.
const CLICK_WINDOW: Duration = Duration::from_secs(2);

/// One popup's way to the screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PopupTiming {
    /// Last button press to the flip showing the popup (None: no press within 2 s).
    pub from_click: Option<Duration>,
    /// Popup created to the client's first buffer for it.
    pub client: Duration,
    /// First buffer to the flip showing it.
    pub compositor: Duration,
}

#[derive(Debug, Clone, Copy)]
struct Flip {
    received: Instant,
    vblank: Option<VBlank>,
}

#[derive(Debug, Clone, Copy)]
struct Queued {
    at: Instant,
    /// Started within one refresh of the previous flip: part of a continuous run of frames.
    back_to_back: bool,
    key: Option<Input>,
    pointer: Option<Input>,
}

#[derive(Debug)]
pub struct FrameTimer {
    refresh: Duration,
    last_flip: Option<Flip>,
    queued: Option<Queued>,
    pending_key: Option<Input>,
    pending_pointer: Option<Input>,
    last_button: Option<Input>,
    popups: Vec<PendingPopup>,
    /// Popups shown this interval (for tests; logged as they happen).
    shown_popups: Vec<PopupTiming>,
    renders: u32,
    empty: u32,
    flips: u32,
    missed: u32,
    no_redraw: u32,
    cpu: Vec<Duration>,
    wait: Vec<Duration>,
    frame: Vec<Duration>,
    key: Vec<Duration>,
    pointer: Vec<Duration>,
}

impl Default for FrameTimer {
    fn default() -> Self {
        Self::new()
    }
}

/// p50/p95/max of one kind of sample.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Spread {
    pub p50: Duration,
    pub p95: Duration,
    pub max: Duration,
}

/// One stats interval's worth of frame timing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Summary {
    pub renders: u32,
    pub empty: u32,
    pub flips: u32,
    pub missed: u32,
    pub no_redraw: u32,
    pub cpu: Option<Spread>,
    pub wait: Option<Spread>,
    pub frame: Option<Spread>,
    pub key: Option<Spread>,
    pub pointer: Option<Spread>,
}

impl FrameTimer {
    pub fn new() -> Self {
        Self {
            refresh: DEFAULT_REFRESH,
            last_flip: None,
            queued: None,
            pending_key: None,
            pending_pointer: None,
            last_button: None,
            popups: Vec::with_capacity(MAX_PENDING_POPUPS),
            shown_popups: Vec::with_capacity(MAX_PENDING_POPUPS),
            renders: 0,
            empty: 0,
            flips: 0,
            missed: 0,
            no_redraw: 0,
            cpu: Vec::with_capacity(MAX_SAMPLES),
            wait: Vec::with_capacity(MAX_SAMPLES),
            frame: Vec::with_capacity(MAX_SAMPLES),
            key: Vec::with_capacity(MAX_SAMPLES),
            pointer: Vec::with_capacity(MAX_SAMPLES),
        }
    }

    /// An input event arrived (`event_us` = its own timestamp, if the backend has one). Only the
    /// earliest event per kind since the last queued frame counts: that's the one waiting longest.
    pub fn input(&mut self, kind: InputKind, now: Instant, event_us: Option<u64>) {
        let slot = match kind {
            InputKind::Key => &mut self.pending_key,
            InputKind::Pointer => &mut self.pending_pointer,
        };
        slot.get_or_insert(Input { received: now, event_us });
    }

    /// A pointer button was pressed (a candidate for what opens the next popup).
    pub fn button_pressed(&mut self, now: Instant, event_us: Option<u64>) {
        self.last_button = Some(Input { received: now, event_us });
    }

    /// A popup was created (`key` identifies its surface).
    pub fn popup_created(&mut self, key: u64, now: Instant) {
        let click = self
            .last_button
            .filter(|b| now.saturating_duration_since(b.received) <= CLICK_WINDOW);
        self.popups.retain(|p| p.key != key);
        if self.popups.len() == MAX_PENDING_POPUPS {
            self.popups.remove(0);
        }
        self.popups.push(PendingPopup { key, created: now, click, buffer: None, in_frame: false });
    }

    /// A tracked popup committed with a buffer attached. Only its first buffer counts.
    pub fn popup_buffer(&mut self, key: u64, now: Instant) {
        if let Some(p) = self.popups.iter_mut().find(|p| p.key == key && p.buffer.is_none()) {
            p.buffer = Some(now);
        }
    }

    /// Whether a popup is still waiting for its first buffer (lets the caller skip the buffer
    /// check on every other commit).
    pub fn popup_waiting(&self, key: u64) -> bool {
        self.popups.iter().any(|p| p.key == key && p.buffer.is_none())
    }

    /// Refresh rate of the output in mHz, as in `smithay::output::Mode::refresh`.
    pub fn set_refresh_mhz(&mut self, mhz: i32) {
        if mhz > 0 {
            self.refresh = Duration::from_nanos(1_000_000_000_000 / mhz as u64);
        }
    }

    /// A render pass ran from `start` to `end`; `queued` = a frame was queued for a page flip
    /// (false when the damage tracker found nothing to draw).
    pub fn rendered(&mut self, start: Instant, end: Instant, queued: bool) {
        self.renders = self.renders.saturating_add(1);
        if !queued {
            self.empty = self.empty.saturating_add(1);
            return;
        }
        push(&mut self.cpu, end.saturating_duration_since(start));
        for p in self.popups.iter_mut().filter(|p| p.buffer.is_some()) {
            p.in_frame = true;
        }
        let back_to_back = self
            .last_flip
            .is_some_and(|f| start.saturating_duration_since(f.received) <= self.refresh);
        let mut answered = |slot: &mut Option<Input>| {
            let input = slot.take()?;
            if start.saturating_duration_since(input.received) > NO_REDRAW_AFTER {
                self.no_redraw = self.no_redraw.saturating_add(1);
                return None;
            }
            Some(input)
        };
        let key = answered(&mut self.pending_key);
        let pointer = answered(&mut self.pending_pointer);
        self.queued = Some(Queued { at: end, back_to_back, key, pointer });
    }

    /// A page flip completed (VBlank event received at `now`), with the kernel's timestamp and
    /// sequence number when the driver provides them.
    pub fn flipped(&mut self, now: Instant, vblank: Option<VBlank>) {
        let flip = Flip { received: now, vblank };
        let Some(queued) = self.queued.take() else {
            self.last_flip = Some(flip);
            return;
        };
        self.flips = self.flips.saturating_add(1);
        push(&mut self.wait, now.saturating_duration_since(queued.at));
        if let Some(input) = queued.key {
            push(&mut self.key, latency(input, &flip));
        }
        if let Some(input) = queued.pointer {
            push(&mut self.pointer, latency(input, &flip));
        }
        self.finish_popups(&flip);
        if let (true, Some(prev)) = (queued.back_to_back, self.last_flip) {
            let interval = match (prev.vblank, vblank) {
                (Some(a), Some(b)) if b.time >= a.time => b.time - a.time,
                _ => now.saturating_duration_since(prev.received),
            };
            push(&mut self.frame, interval);
            if let (Some(a), Some(b)) = (prev.vblank, vblank) {
                let delta = b.sequence.wrapping_sub(a.sequence);
                // A huge delta is a counter reset (e.g. after a VT switch), not missed frames.
                if (2..1000).contains(&delta) {
                    self.missed = self.missed.saturating_add(delta - 1);
                }
            }
        }
        self.last_flip = Some(flip);
    }

    /// Forget the in-flight frame and the last flip (session paused: a pending flip may never
    /// complete, and the next flip isn't back to back with the last one). Counts are kept.
    pub fn reset(&mut self) {
        self.queued = None;
        self.last_flip = None;
        self.pending_key = None;
        self.pending_pointer = None;
        self.last_button = None;
        self.popups.clear();
    }

    fn finish_popups(&mut self, flip: &Flip) {
        for p in self.popups.iter().filter(|p| p.in_frame) {
            let Some(buffer) = p.buffer else { continue };
            let t = PopupTiming {
                from_click: p.click.map(|c| latency(c, flip)),
                client: buffer.saturating_duration_since(p.created),
                compositor: flip.received.saturating_duration_since(buffer),
            };
            let ms = |d: Duration| d.as_secs_f64() * 1000.0;
            tracing::info!(
                "popup shown: from_click_ms={} client_ms={:.1} compositor_ms={:.1}",
                t.from_click.map_or("-".to_string(), |d| format!("{:.1}", ms(d))),
                ms(t.client),
                ms(t.compositor)
            );
            if self.shown_popups.len() < MAX_PENDING_POPUPS {
                self.shown_popups.push(t);
            }
        }
        self.popups.retain(|p| !p.in_frame);
    }

    /// The interval's summary, then start a new interval. `None` if nothing was rendered.
    pub fn take(&mut self) -> Option<Summary> {
        if self.renders == 0 && self.flips == 0 {
            return None;
        }
        let summary = Summary {
            renders: self.renders,
            empty: self.empty,
            flips: self.flips,
            missed: self.missed,
            no_redraw: self.no_redraw,
            cpu: spread(&mut self.cpu),
            wait: spread(&mut self.wait),
            frame: spread(&mut self.frame),
            key: spread(&mut self.key),
            pointer: spread(&mut self.pointer),
        };
        self.renders = 0;
        self.empty = 0;
        self.flips = 0;
        self.missed = 0;
        self.no_redraw = 0;
        self.cpu.clear();
        self.wait.clear();
        self.frame.clear();
        self.key.clear();
        self.pointer.clear();
        self.shown_popups.clear();
        Some(summary)
    }
}

/// Modifier keys by xkb keycode (evdev + 8): Ctrl, Shift, Alt, Super (left/right), Caps Lock.
pub fn is_modifier_key(xkb_keycode: u32) -> bool {
    matches!(xkb_keycode.checked_sub(8), Some(29 | 97 | 42 | 54 | 56 | 100 | 125 | 126 | 58))
}

/// Input event to flip: kernel timestamps on both ends when they're comparable and sane, else
/// the times kompact received each.
fn latency(input: Input, flip: &Flip) -> Duration {
    if let (Some(us), Some(v)) = (input.event_us, flip.vblank) {
        let event = Duration::from_micros(us);
        if v.monotonic && v.time >= event && v.time - event < Duration::from_secs(2) {
            return v.time - event;
        }
    }
    flip.received.saturating_duration_since(input.received)
}

fn push(samples: &mut Vec<Duration>, d: Duration) {
    if samples.len() < MAX_SAMPLES {
        samples.push(d);
    }
}

/// Nearest-rank percentiles; sorts in place (the buffer is cleared right after).
fn spread(samples: &mut [Duration]) -> Option<Spread> {
    if samples.is_empty() {
        return None;
    }
    samples.sort_unstable();
    let rank = |p: usize| samples[(samples.len() * p).div_ceil(100).max(1) - 1];
    Some(Spread { p50: rank(50), p95: rank(95), max: samples[samples.len() - 1] })
}

impl Summary {
    /// ` renders=.. empty=.. flips=.. missed=.. cpu_ms=p50/p95/max wait_ms=.. frame_ms=..
    /// key_ms=.. ptr_ms=.. no_redraw=..`,
    /// appended to the `stats:` line; `-` where there were no samples.
    pub fn fragment(&self) -> String {
        let s = |x: Option<Spread>| {
            x.map_or("-".to_string(), |x| {
                let ms = |d: Duration| d.as_secs_f64() * 1000.0;
                format!("{:.2}/{:.2}/{:.2}", ms(x.p50), ms(x.p95), ms(x.max))
            })
        };
        format!(
            " renders={} empty={} flips={} missed={} cpu_ms={} wait_ms={} frame_ms={} key_ms={} ptr_ms={} no_redraw={}",
            self.renders,
            self.empty,
            self.flips,
            self.missed,
            s(self.cpu),
            s(self.wait),
            s(self.frame),
            s(self.key),
            s(self.pointer),
            self.no_redraw
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    fn vb(time: Duration, sequence: u32) -> Option<VBlank> {
        Some(VBlank { time, monotonic: true, sequence })
    }

    #[test]
    fn nothing_rendered_gives_nothing() {
        assert_eq!(FrameTimer::new().take(), None);
    }

    #[test]
    fn idle_passes_count_as_empty() {
        let mut t = FrameTimer::new();
        let t0 = Instant::now();
        for i in 0..60 {
            t.rendered(t0 + ms(16 * i), t0 + ms(16 * i), false);
        }
        let s = t.take().unwrap();
        assert_eq!((s.renders, s.empty, s.flips), (60, 60, 0));
        assert_eq!((s.cpu, s.wait, s.frame), (None, None, None));
        assert_eq!(t.take(), None, "next interval starts empty");
    }

    /// Continuous rendering at 60 Hz: every frame queued right after the previous flip.
    #[test]
    fn back_to_back_frames_give_frame_time_from_kernel_timestamps() {
        let mut t = FrameTimer::new();
        t.set_refresh_mhz(60_000);
        let t0 = Instant::now();
        let kernel0 = Duration::from_secs(100);
        let period = Duration::from_micros(16_667);
        for i in 0..10u32 {
            let start = t0 + period * i;
            t.rendered(start, start + ms(2), true);
            // Host receives the event 1 ms late; the kernel timestamp is exact.
            t.flipped(t0 + period * (i + 1) + ms(1), vb(kernel0 + period * (i + 1), i + 1));
        }
        let s = t.take().unwrap();
        assert_eq!((s.renders, s.empty, s.flips, s.missed), (10, 0, 10, 0));
        assert_eq!(s.cpu.unwrap().max, ms(2));
        // The first flip has no predecessor; the other 9 are exactly one period apart.
        assert_eq!(s.frame.unwrap(), Spread { p50: period, p95: period, max: period });
    }

    #[test]
    fn skipped_vblanks_are_counted_as_missed() {
        let mut t = FrameTimer::new();
        let t0 = Instant::now();
        t.rendered(t0, t0 + ms(1), true);
        t.flipped(t0 + ms(17), vb(ms(17), 10));
        // Next frame starts right away but its flip lands two vblanks later (30 fps).
        t.rendered(t0 + ms(17), t0 + ms(30), true);
        t.flipped(t0 + ms(50), vb(ms(50), 12));
        let s = t.take().unwrap();
        assert_eq!(s.missed, 1);
        assert_eq!(s.frame.unwrap().max, ms(33));
        assert_eq!(s.wait.unwrap().max, ms(20));
    }

    #[test]
    fn idle_gap_between_frames_is_not_a_frame_time() {
        let mut t = FrameTimer::new();
        let t0 = Instant::now();
        t.rendered(t0, t0 + ms(1), true);
        t.flipped(t0 + ms(16), vb(ms(16), 1));
        // Nothing for 2 s, then a keypress redraws.
        t.rendered(t0 + ms(2016), t0 + ms(2017), true);
        t.flipped(t0 + ms(2032), vb(ms(2032), 127));
        let s = t.take().unwrap();
        assert_eq!(s.flips, 2);
        assert_eq!(s.frame, None);
        assert_eq!(s.missed, 0);
    }

    #[test]
    fn reset_drops_in_flight_frame() {
        let mut t = FrameTimer::new();
        let t0 = Instant::now();
        t.rendered(t0, t0 + ms(1), true);
        t.reset(); // VT switch away; that flip never completes
        t.flipped(t0 + ms(5000), vb(ms(5000), 1));
        let s = t.take().unwrap();
        assert_eq!((s.flips, s.wait), (0, None));
    }

    #[test]
    fn counter_reset_is_not_missed_frames() {
        let mut t = FrameTimer::new();
        let t0 = Instant::now();
        t.rendered(t0, t0 + ms(1), true);
        t.flipped(t0 + ms(16), vb(ms(16), 50_000));
        t.rendered(t0 + ms(16), t0 + ms(17), true);
        t.flipped(t0 + ms(33), vb(ms(33), 3));
        assert_eq!(t.take().unwrap().missed, 0);
    }

    #[test]
    fn missing_kernel_timestamps_fall_back_to_receive_time() {
        let mut t = FrameTimer::new();
        let t0 = Instant::now();
        t.rendered(t0, t0 + ms(1), true);
        t.flipped(t0 + ms(16), None);
        t.rendered(t0 + ms(16), t0 + ms(18), true);
        t.flipped(t0 + ms(33), None);
        assert_eq!(t.take().unwrap().frame.unwrap().max, ms(17));
    }

    #[test]
    fn percentiles_nearest_rank() {
        let mut v: Vec<Duration> = (1..=100).rev().map(ms).collect();
        assert_eq!(spread(&mut v).unwrap(), Spread { p50: ms(50), p95: ms(95), max: ms(100) });
        assert_eq!(spread(&mut [ms(7)]).unwrap(), Spread { p50: ms(7), p95: ms(7), max: ms(7) });
        assert_eq!(spread(&mut []), None);
    }

    #[test]
    fn samples_are_bounded() {
        let mut t = FrameTimer::new();
        let t0 = Instant::now();
        for _ in 0..(MAX_SAMPLES + 100) {
            t.rendered(t0, t0 + ms(1), true);
        }
        assert_eq!(t.cpu.len(), MAX_SAMPLES);
        assert_eq!(t.take().unwrap().renders as usize, MAX_SAMPLES + 100);
        assert!(t.cpu.capacity() >= MAX_SAMPLES, "buffer is reused, not reallocated");
    }

    #[test]
    fn fragment_format() {
        let s = Summary {
            renders: 61,
            empty: 58,
            flips: 3,
            missed: 0,
            no_redraw: 2,
            cpu: Some(Spread { p50: ms(1), p95: ms(2), max: ms(3) }),
            wait: None,
            frame: None,
            key: None,
            pointer: Some(Spread { p50: ms(20), p95: ms(30), max: ms(40) }),
        };
        assert_eq!(
            s.fragment(),
            " renders=61 empty=58 flips=3 missed=0 cpu_ms=1.00/2.00/3.00 wait_ms=- frame_ms=- key_ms=- ptr_ms=20.00/30.00/40.00 no_redraw=2"
        );
    }

    #[test]
    fn input_latency_uses_kernel_timestamps() {
        let mut t = FrameTimer::new();
        let t0 = Instant::now();
        // Key pressed at kernel time 10.000 s, read by kompact 3 ms later.
        t.input(InputKind::Key, t0 + ms(3), Some(10_000_000));
        t.input(InputKind::Key, t0 + ms(5), Some(10_002_000)); // later press, same frame: ignored
        t.rendered(t0 + ms(6), t0 + ms(8), true);
        t.flipped(t0 + ms(18), vb(Duration::from_millis(10_017), 1));
        let s = t.take().unwrap();
        assert_eq!(s.key.unwrap().max, ms(17));
        assert_eq!(s.pointer, None);
    }

    #[test]
    fn input_waits_for_a_queued_frame() {
        let mut t = FrameTimer::new();
        let t0 = Instant::now();
        t.input(InputKind::Pointer, t0, None);
        t.rendered(t0 + ms(1), t0 + ms(2), false); // nothing to draw yet: still pending
        t.rendered(t0 + ms(17), t0 + ms(18), true);
        t.flipped(t0 + ms(33), None);
        // Next frame has no new input.
        t.rendered(t0 + ms(34), t0 + ms(35), true);
        t.flipped(t0 + ms(50), None);
        let s = t.take().unwrap();
        assert_eq!(s.pointer.unwrap(), Spread { p50: ms(33), p95: ms(33), max: ms(33) });
    }

    #[test]
    fn input_latency_falls_back_when_clocks_dont_compare() {
        let mut t = FrameTimer::new();
        let t0 = Instant::now();
        t.input(InputKind::Key, t0, Some(5_000_000));
        t.rendered(t0 + ms(1), t0 + ms(2), true);
        // Realtime kernel stamp: not comparable with the event's monotonic time.
        let realtime = Some(VBlank { time: Duration::from_secs(1_700_000_000), monotonic: false, sequence: 1 });
        t.flipped(t0 + ms(20), realtime);
        assert_eq!(t.take().unwrap().key.unwrap().max, ms(20));
    }

    #[test]
    fn reset_drops_pending_input() {
        let mut t = FrameTimer::new();
        let t0 = Instant::now();
        t.input(InputKind::Key, t0, None);
        t.reset();
        t.rendered(t0 + ms(5000), t0 + ms(5001), true);
        t.flipped(t0 + ms(5017), None);
        assert_eq!(t.take().unwrap().key, None);
    }

    #[test]
    fn modifiers_are_recognised() {
        for evdev in [29, 97, 42, 54, 56, 100, 125, 126, 58] {
            assert!(is_modifier_key(evdev + 8), "{evdev}");
        }
        assert!(!is_modifier_key(30 + 8)); // A
        assert!(!is_modifier_key(28 + 8)); // Enter
        assert!(!is_modifier_key(0));
    }

    #[test]
    fn popup_timing_from_click_to_flip() {
        let mut t = FrameTimer::new();
        let t0 = Instant::now();
        t.button_pressed(t0, None);
        t.popup_created(7, t0 + ms(10));
        assert!(t.popup_waiting(7));
        // A frame before the popup has a buffer doesn't show it.
        t.rendered(t0 + ms(12), t0 + ms(13), true);
        t.flipped(t0 + ms(17), None);
        assert!(t.shown_popups.is_empty());
        t.popup_buffer(7, t0 + ms(40));
        assert!(!t.popup_waiting(7));
        t.popup_buffer(7, t0 + ms(45)); // later buffers don't move the first one
        t.rendered(t0 + ms(41), t0 + ms(43), true);
        t.flipped(t0 + ms(50), None);
        assert_eq!(
            t.shown_popups,
            vec![PopupTiming { from_click: Some(ms(50)), client: ms(30), compositor: ms(10) }]
        );
        assert!(t.popups.is_empty(), "shown popups stop being tracked");
    }

    #[test]
    fn popup_without_recent_click() {
        let mut t = FrameTimer::new();
        let t0 = Instant::now();
        t.button_pressed(t0, None);
        t.popup_created(1, t0 + ms(5000));
        t.popup_buffer(1, t0 + ms(5010));
        t.rendered(t0 + ms(5011), t0 + ms(5012), true);
        t.flipped(t0 + ms(5020), None);
        assert_eq!(t.shown_popups[0].from_click, None);
    }

    #[test]
    fn pending_popups_are_bounded() {
        let mut t = FrameTimer::new();
        let t0 = Instant::now();
        for key in 0..(MAX_PENDING_POPUPS as u64 + 5) {
            t.popup_created(key, t0); // never get a buffer
        }
        assert_eq!(t.popups.len(), MAX_PENDING_POPUPS);
        assert!(!t.popup_waiting(0), "oldest dropped first");
        t.reset();
        assert!(t.popups.is_empty());
    }

    #[test]
    fn input_without_a_redraw_is_not_a_latency() {
        let mut t = FrameTimer::new();
        let t0 = Instant::now();
        t.input(InputKind::Key, t0, None); // e.g. Escape at an empty prompt: nothing redraws
        t.rendered(t0 + ms(400), t0 + ms(400), false);
        t.rendered(t0 + ms(1600), t0 + ms(1601), true); // cursor blink, 1.6 s later
        t.flipped(t0 + ms(1617), None);
        let s = t.take().unwrap();
        assert_eq!((s.key, s.no_redraw), (None, 1));
        // Within the limit it's a sample.
        t.input(InputKind::Key, t0 + ms(2000), None);
        t.rendered(t0 + ms(2400), t0 + ms(2401), true);
        t.flipped(t0 + ms(2417), None);
        let s = t.take().unwrap();
        assert_eq!((s.key.unwrap().max, s.no_redraw), (ms(417), 0));
    }
}
