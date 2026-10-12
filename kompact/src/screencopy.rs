//! 9b: wlr-screencopy-v1, for screenshot tools (`grim`, `grim -g "$(slurp)"`).
//!
//! A client asks for an output or a region of one; kompact offers one wl_shm buffer type
//! (`XRGB8888`, the width and height of the clipped region) and, when the client hands over its
//! buffer with `copy`, queues the copy. After the backend has queued this turn's frames, the
//! copy is made the 9a way (`screenshot::capture`: the output drawn off screen, read back,
//! unbound), cut to the region and written straight into the client's buffer; then `flags` (0:
//! top row first) and `ready`. Scanout buffers are never read.
//!
//! Only wl_shm buffers (no dmabuf). Plain `copy` is refused (`failed`) while the screen is
//! locked. `KOMPACT_SCREENCOPY=off` doesn't offer the protocol at all.
//!
//! 9e-a: `copy_with_damage` (screen sharing: xdg-desktop-portal-wlr asks for it frame after
//! frame) waits for damage, as the protocol says. Each stream (same client, output, region,
//! cursor wish) keeps an `OutputDamageTracker`; every loop turn a waiting copy builds the
//! output's elements and asks the tracker what changed since its last copy (`damage_output`,
//! nothing drawn). Nothing in the region: it waits. Otherwise it is copied and the changed
//! rectangles go out as `damage`. While locked these copies wait too (the stream freezes).
//! Stream copies are logged as a summary every 10 s, not one line each.
//!
//! Like wlroots compositors, any client may capture the screen while this is offered.

use std::collections::HashMap;
use std::hash::Hash;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use smithay::{
    backend::{
        allocator::Fourcc,
        renderer::{damage::OutputDamageTracker, gles::GlesRenderer},
    },
    output::Output,
    reexports::{
        wayland_protocols_wlr::screencopy::v1::server::{
            zwlr_screencopy_frame_v1::{self, Flags, ZwlrScreencopyFrameV1},
            zwlr_screencopy_manager_v1::{self, ZwlrScreencopyManagerV1},
        },
        wayland_server::{
            backend::ClientId,
            protocol::{wl_buffer::WlBuffer, wl_shm},
            Client, DataInit, Dispatch, DisplayHandle, GlobalDispatch, New, Resource,
        },
    },
    utils::{Physical, Rectangle, Transform},
    wayland::shm,
};

use crate::state::Kompact;

pub const VERSION: u32 = 3;

pub fn enabled_from_env(value: Option<&str>) -> bool {
    crate::screenshot::enabled_from_env(value)
}

/// A region (output-local, logical = physical at scale 1) clipped to an output `w` x `h`, or
/// `None` if nothing of it is on the output. Negative sizes count from the other corner.
pub fn clip_region(w: i32, h: i32, x: i32, y: i32, rw: i32, rh: i32) -> Option<Rectangle<i32, Physical>> {
    let (x0, x1) = if rw < 0 { (x.saturating_add(rw), x) } else { (x, x.saturating_add(rw)) };
    let (y0, y1) = if rh < 0 { (y.saturating_add(rh), y) } else { (y, y.saturating_add(rh)) };
    let (x0, y0, x1, y1) = (x0.max(0), y0.max(0), x1.min(w), y1.min(h));
    (x1 > x0 && y1 > y0).then(|| Rectangle::new((x0, y0).into(), (x1 - x0, y1 - y0).into()))
}

/// What the client's buffer must be: XRGB8888 (or ARGB8888), exactly the region's size, rows
/// at least `4 * width` bytes apart, all inside its pool.
pub fn buffer_fits(format_ok: bool, bw: i32, bh: i32, stride: i32, offset: i32, pool_len: usize, w: i32, h: i32) -> bool {
    format_ok
        && bw == w
        && bh == h
        && stride >= 4 * w
        && offset >= 0
        && (offset as i64 + stride as i64 * h as i64) <= pool_len as i64
}

/// Copy `region` out of `rgba` (`src_w` px wide, RGBA, top row first) into `dst` as XRGB8888
/// (little endian: B, G, R, 0xff), `stride` bytes per row.
pub fn copy_region_xrgb(rgba: &[u8], src_w: i32, region: Rectangle<i32, Physical>, dst: &mut [u8], stride: usize) {
    let (rx, ry, rw, rh) = (region.loc.x as usize, region.loc.y as usize, region.size.w as usize, region.size.h as usize);
    let src_stride = src_w as usize * 4;
    for row in 0..rh {
        let src = &rgba[(ry + row) * src_stride + rx * 4..][..rw * 4];
        let out = &mut dst[row * stride..][..rw * 4];
        for (o, p) in out.chunks_exact_mut(4).zip(src.chunks_exact(4)) {
            o.copy_from_slice(&[p[2], p[1], p[0], 0xff]);
        }
    }
}

/// 9e-a2: copy `region` out of `bgra` (`src_w` px wide, B, G, R, A, top row first: the
/// XRGB8888 byte order already) into `dst`, row by row, `stride` bytes per row.
pub fn copy_region_rows(bgra: &[u8], src_w: i32, region: Rectangle<i32, Physical>, dst: &mut [u8], stride: usize) {
    let (rx, ry, rw, rh) = (region.loc.x as usize, region.loc.y as usize, region.size.w as usize, region.size.h as usize);
    let src_stride = src_w as usize * 4;
    for row in 0..rh {
        dst[row * stride..][..rw * 4].copy_from_slice(&bgra[(ry + row) * src_stride + rx * 4..][..rw * 4]);
    }
}

/// Write `region` of an output read back at `src_w` x `src_h` into the client's buffer: plain
/// rows if it was read back as BGRA, converted from RGBA otherwise.
fn write_into(buffer: &WlBuffer, pixels: &[u8], src_w: i32, src_h: i32, region: Rectangle<i32, Physical>, bgra: bool) -> Result<(), String> {
    if region.loc.x + region.size.w > src_w || region.loc.y + region.size.h > src_h {
        return Err("the output changed size meanwhile".into());
    }
    let written = shm::with_buffer_contents_mut(buffer, |ptr, len, b| {
        let (offset, stride) = (b.offset as usize, b.stride as usize);
        let need = stride * region.size.h as usize;
        if offset + need > len {
            return false;
        }
        // SAFETY: inside the pool (checked above); smithay holds the pool mapping meanwhile.
        let dst = unsafe { std::slice::from_raw_parts_mut(ptr.add(offset), need) };
        if bgra {
            copy_region_rows(pixels, src_w, region, dst, stride);
        } else {
            copy_region_xrgb(pixels, src_w, region, dst, stride);
        }
        true
    });
    match written {
        Ok(true) => Ok(()),
        _ => Err("the client's buffer couldn't be written".into()),
    }
}

/// 9e-a: the changed rectangles inside `region` (all output-local), made relative to it.
pub fn region_damage(damage: &[Rectangle<i32, Physical>], region: Rectangle<i32, Physical>) -> Vec<Rectangle<i32, Physical>> {
    damage
        .iter()
        .filter_map(|d| d.intersection(region))
        .filter(|d| d.size.w > 0 && d.size.h > 0)
        .map(|d| Rectangle::new((d.loc.x - region.loc.x, d.loc.y - region.loc.y).into(), d.size))
        .collect()
}

/// A stream with no copy asked for this long has ended (its tracker goes).
pub const STREAM_IDLE: Duration = Duration::from_secs(3);
/// Stream copies are summed up this often.
pub const SUMMARY_EVERY: Duration = Duration::from_secs(10);

/// 9e-a: what makes copies one stream.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct StreamKey {
    client: ClientId,
    output: String,
    /// x, y, w, h (`Rectangle` isn't `Hash`).
    region: (i32, i32, i32, i32),
    cursor: bool,
}

impl StreamKey {
    fn new(client: &Client, output: &Output, r: Rectangle<i32, Physical>, cursor: bool) -> StreamKey {
        StreamKey { client: client.id(), output: output.name(), region: (r.loc.x, r.loc.y, r.size.w, r.size.h), cursor }
    }
}

impl std::fmt::Display for StreamKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let (x, y, w, h) = self.region;
        write!(f, "{} {w}x{h} at {x},{y}{}", self.output, if self.cursor { " with cursor" } else { "" })
    }
}

#[derive(Debug)]
struct Tracked {
    tracker: OutputDamageTracker,
    size: (i32, i32),
    used: Instant,
}

/// 9e-a: the streams' damage trackers (keyed by `StreamKey`; generic for the tests).
#[derive(Debug)]
pub struct Streams<K = StreamKey> {
    trackers: HashMap<K, Tracked>,
    pub summary: Summary,
    /// Whether waiting copies are being held because the screen is locked (logged once).
    paused: bool,
    /// 9e-a2: reading back as BGRA failed once; read RGBA and convert, as in 9b.
    rgba_only: bool,
}

impl<K> Default for Streams<K> {
    fn default() -> Self {
        Streams { trackers: HashMap::new(), summary: Summary::default(), paused: false, rgba_only: false }
    }
}

impl<K: Eq + Hash + Clone> Streams<K> {
    pub fn is_active(&self) -> bool {
        !self.trackers.is_empty()
    }

    pub fn has(&self, key: &K) -> bool {
        self.trackers.contains_key(key)
    }

    /// The stream's tracker for an output of `size` (new, or new again if the size changed),
    /// and whether the stream just started.
    pub fn touch(&mut self, key: K, size: (i32, i32), now: Instant) -> (&mut OutputDamageTracker, bool) {
        let new = !self.trackers.contains_key(&key);
        let t = self.trackers.entry(key).or_insert_with(|| Tracked {
            tracker: OutputDamageTracker::new(size, 1.0, Transform::Normal),
            size,
            used: now,
        });
        if t.size != size {
            *t = Tracked { tracker: OutputDamageTracker::new(size, 1.0, Transform::Normal), size, used: now };
        }
        t.used = now;
        (&mut t.tracker, new)
    }

    /// The stream is still there (a copy waits), without looking at damage.
    pub fn seen(&mut self, key: &K, now: Instant) {
        if let Some(t) = self.trackers.get_mut(key) {
            t.used = now;
        }
    }

    /// Drop the streams idle for `STREAM_IDLE`; returns them.
    pub fn prune(&mut self, now: Instant) -> Vec<K> {
        let gone: Vec<K> = self.trackers.iter().filter(|(_, t)| now.duration_since(t.used) >= STREAM_IDLE).map(|(k, _)| k.clone()).collect();
        for k in &gone {
            self.trackers.remove(k);
        }
        gone
    }
}

/// 9e-c: the monitors being shared, each once, sorted.
pub fn sharing_outputs<'a>(names: impl Iterator<Item = &'a str>) -> Vec<String> {
    let mut list: Vec<String> = names.map(str::to_string).collect();
    list.sort();
    list.dedup();
    list
}

impl Streams<StreamKey> {
    /// 9e-c: the monitors running streams capture (empty: nothing is shared).
    pub fn outputs(&self) -> Vec<String> {
        sharing_outputs(self.trackers.keys().map(|k| k.output.as_str()))
    }
}

/// One stream copy's times.
#[derive(Debug, Clone, Copy)]
pub struct Sample {
    /// From `copy_with_damage` to the copy (waiting for damage).
    pub waited: Duration,
    pub draw: Duration,
    pub read_back: Duration,
    /// 9e-a2: writing into the client's buffer.
    pub write: Duration,
    /// The whole copy: draw, read back, write.
    pub total: Duration,
}

/// 9e-a: stream copies since the last summary line.
#[derive(Debug, Default)]
pub struct Summary {
    samples: Vec<Sample>,
    since: Option<Instant>,
}

impl Summary {
    pub fn add(&mut self, sample: Sample, now: Instant) {
        self.since.get_or_insert(now);
        self.samples.push(sample);
    }

    /// The summary line once `SUMMARY_EVERY` has passed since the first sample (or at once
    /// with `flush`, when the streams have ended); starts over.
    pub fn take_line(&mut self, now: Instant, flush: bool) -> Option<String> {
        let since = self.since?;
        let over = now.duration_since(since);
        if over < SUMMARY_EVERY && !flush {
            return None;
        }
        let line = summary_line(&self.samples, over);
        self.samples.clear();
        self.since = None;
        Some(line)
    }
}

/// Nearest-rank percentile of `values` in ms (sorted here).
fn percentile_ms(values: &mut [Duration], q: f64) -> f64 {
    if values.is_empty() {
        return 0.0;
    }
    values.sort();
    let i = ((q * values.len() as f64).ceil() as usize).clamp(1, values.len()) - 1;
    values[i].as_secs_f64() * 1000.0
}

pub fn summary_line(samples: &[Sample], over: Duration) -> String {
    let pick = |f: fn(&Sample) -> Duration| samples.iter().map(f).collect::<Vec<_>>();
    let (mut waited, mut total) = (pick(|s| s.waited), pick(|s| s.total));
    let (mut draw, mut read, mut write) = (pick(|s| s.draw), pick(|s| s.read_back), pick(|s| s.write));
    let secs = over.as_secs_f64().max(0.001);
    format!(
        "screencopy: {} stream copies in {:.1} s ({:.1}/s); waited for damage p50 {:.1} ms; copy p50 {:.1} / p95 {:.1} ms (draw {:.1} / read back {:.1} / write {:.1} ms p50)",
        samples.len(),
        secs,
        samples.len() as f64 / secs,
        percentile_ms(&mut waited, 0.5),
        percentile_ms(&mut total, 0.5),
        percentile_ms(&mut total, 0.95),
        percentile_ms(&mut draw, 0.5),
        percentile_ms(&mut read, 0.5),
        percentile_ms(&mut write, 0.5),
    )
}

/// One frame object's request: the output, the clipped region, the cursor wish.
#[derive(Debug)]
pub struct FrameData {
    /// `None`: the output is gone or the region is off it; the frame only gets `failed`.
    target: Option<(Output, Rectangle<i32, Physical>)>,
    cursor: bool,
    used: Mutex<bool>,
}

/// A queued copy, made after this turn's frames.
#[derive(Debug)]
pub struct Copy {
    frame: ZwlrScreencopyFrameV1,
    buffer: WlBuffer,
    /// `copy_with_damage`: the stream it belongs to (9e-a).
    stream: Option<StreamKey>,
    queued: Instant,
    /// Logged (debug) once when it starts waiting for damage.
    waiting: bool,
}

pub fn create_global(dh: &DisplayHandle) {
    dh.create_global::<Kompact, ZwlrScreencopyManagerV1, _>(VERSION, ());
}

impl GlobalDispatch<ZwlrScreencopyManagerV1, ()> for Kompact {
    fn bind(_: &mut Self, _: &DisplayHandle, _: &Client, resource: New<ZwlrScreencopyManagerV1>, _: &(), init: &mut DataInit<'_, Self>) {
        init.init(resource, ());
    }
}

impl Dispatch<ZwlrScreencopyManagerV1, ()> for Kompact {
    fn request(
        state: &mut Self,
        client: &Client,
        _: &ZwlrScreencopyManagerV1,
        request: zwlr_screencopy_manager_v1::Request,
        _: &(),
        _: &DisplayHandle,
        init: &mut DataInit<'_, Self>,
    ) {
        let (frame, cursor, output, region) = match request {
            zwlr_screencopy_manager_v1::Request::CaptureOutput { frame, overlay_cursor, output } => (frame, overlay_cursor, output, None),
            zwlr_screencopy_manager_v1::Request::CaptureOutputRegion { frame, overlay_cursor, output, x, y, width, height } => {
                (frame, overlay_cursor, output, Some((x, y, width, height)))
            }
            zwlr_screencopy_manager_v1::Request::Destroy => return,
            _ => return,
        };
        let output = Output::from_resource(&output).filter(|o| state.outputs.contains(o));
        let size = output.as_ref().and_then(|o| o.current_mode()).map(|m| (m.size.w, m.size.h));
        let clipped = size.and_then(|(w, h)| match region {
            None => clip_region(w, h, 0, 0, w, h),
            Some((x, y, rw, rh)) => clip_region(w, h, x, y, rw, rh),
        });
        let (Some(output), Some(region)) = (output, clipped) else {
            tracing::info!("screencopy: capture refused (output gone or region off it: {region:?})");
            let frame = init.init(frame, FrameData { target: None, cursor: false, used: Mutex::new(true) });
            frame.failed();
            return;
        };
        let (w, h) = (region.size.w, region.size.h);
        let key = StreamKey::new(client, &output, region, cursor != 0);
        // 9e-a: a running stream asks for a frame like this many times a second.
        if state.shots.streams.has(&key) {
            tracing::debug!("screencopy: capture of {key} offered");
        } else {
            tracing::info!("screencopy: capture of {key} offered");
        }
        let frame = init.init(frame, FrameData { target: Some((output, region)), cursor: cursor != 0, used: Mutex::new(false) });
        frame.buffer(wl_shm::Format::Xrgb8888, w as u32, h as u32, 4 * w as u32);
        if frame.version() >= 3 {
            frame.buffer_done();
        }
    }
}

impl Dispatch<ZwlrScreencopyFrameV1, FrameData> for Kompact {
    fn request(
        state: &mut Self,
        client: &Client,
        frame: &ZwlrScreencopyFrameV1,
        request: zwlr_screencopy_frame_v1::Request,
        data: &FrameData,
        _: &DisplayHandle,
        _: &mut DataInit<'_, Self>,
    ) {
        let (buffer, damage) = match request {
            zwlr_screencopy_frame_v1::Request::Copy { buffer } => (buffer, false),
            zwlr_screencopy_frame_v1::Request::CopyWithDamage { buffer } => (buffer, true),
            zwlr_screencopy_frame_v1::Request::Destroy => return,
            _ => return,
        };
        {
            let mut used = data.used.lock().unwrap();
            if *used {
                frame.post_error(zwlr_screencopy_frame_v1::Error::AlreadyUsed, "frame already used to copy");
                return;
            }
            *used = true;
        }
        let Some((output, region)) = &data.target else {
            frame.failed();
            return;
        };
        let (w, h) = (region.size.w, region.size.h);
        let fits = shm::with_buffer_contents(&buffer, |_, len, b| {
            let format_ok = matches!(b.format, wl_shm::Format::Xrgb8888 | wl_shm::Format::Argb8888);
            buffer_fits(format_ok, b.width, b.height, b.stride, b.offset, len, w, h)
        });
        match fits {
            Ok(true) => {
                let stream = damage.then(|| StreamKey::new(client, output, *region, data.cursor));
                state.shots.copies.push(Copy { frame: frame.clone(), buffer, stream, queued: Instant::now(), waiting: false });
            }
            Ok(false) => frame.post_error(zwlr_screencopy_frame_v1::Error::InvalidBuffer, "buffer format, size or stride don't match"),
            Err(_) => frame.post_error(zwlr_screencopy_frame_v1::Error::InvalidBuffer, "not a wl_shm buffer"),
        }
    }
}

fn monotonic_now() -> (u64, u32) {
    // SAFETY: clock_gettime only writes the timespec we hand it.
    let mut ts: libc::timespec = unsafe { std::mem::zeroed() };
    unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut ts) };
    (ts.tv_sec as u64, ts.tv_nsec as u32)
}

/// From `screenshot::capture_pending`: make the queued copies; stream copies only once their
/// region has changed (9e-a).
pub fn run_pending(renderer: &mut GlesRenderer, state: &mut Kompact) {
    let now = Instant::now();
    let mut streams = std::mem::take(&mut state.shots.streams);
    let locked = state.lock.is_locked();
    if !locked && streams.paused {
        tracing::info!("screencopy: streams resumed, the screen is unlocked");
        streams.paused = false;
    }
    let mut waiting = Vec::new();
    for mut copy in std::mem::take(&mut state.shots.copies) {
        if !copy.frame.is_alive() || !copy.buffer.is_alive() {
            if let Some(key) = &copy.stream {
                tracing::debug!("screencopy: {key}: frame or buffer destroyed before the copy");
            }
            continue;
        }
        let Some(FrameData { target: Some((output, region)), cursor, .. }) = copy.frame.data::<FrameData>() else { continue };
        let (output, region, cursor) = (output.clone(), *region, *cursor);
        if locked {
            match &copy.stream {
                Some(key) => {
                    if !streams.paused {
                        tracing::info!("screencopy: streams paused while the screen is locked");
                        streams.paused = true;
                    }
                    streams.seen(key, now);
                    waiting.push(copy);
                }
                None => {
                    tracing::info!("screencopy: refused, the screen is locked");
                    copy.frame.failed();
                }
            }
            continue;
        }
        if !state.outputs.contains(&output) {
            tracing::info!("screencopy: {} is gone, failed", output.name());
            copy.frame.failed();
            continue;
        }
        let damage = match &copy.stream {
            None => None,
            Some(key) => {
                let Some(size) = output.current_mode().map(|m| (m.size.w, m.size.h)) else {
                    copy.frame.failed();
                    continue;
                };
                let (tracker, new) = streams.touch(key.clone(), size, now);
                if new {
                    tracing::info!("screencopy: stream of {key} started");
                }
                let elements = crate::render::build_elements_with(renderer, state, &output, cursor);
                let rects = match tracker.damage_output(1, &elements) {
                    Ok((damage, _)) => region_damage(damage.map(Vec::as_slice).unwrap_or(&[]), region),
                    Err(e) => {
                        tracing::warn!("screencopy: {}: no damage ({e:?}), failed", output.name());
                        copy.frame.failed();
                        continue;
                    }
                };
                if rects.is_empty() {
                    if !copy.waiting {
                        tracing::debug!("screencopy: {key}: no damage yet, the copy waits");
                        copy.waiting = true;
                    }
                    waiting.push(copy);
                    continue;
                }
                Some(rects)
            }
        };
        let started = Instant::now();
        // 9e-a2: read back in the client's byte order, so writing is a plain copy per row.
        let capture = |renderer: &mut GlesRenderer, fourcc, bgra| {
            crate::screenshot::capture_with(renderer, state, &output, cursor, fourcc, |w, h, pixels| {
                let t = Instant::now();
                write_into(&copy.buffer, pixels, w, h, region, bgra).map(|()| t.elapsed())
            })
        };
        let mut captured = Err(String::new());
        if !streams.rgba_only {
            captured = capture(renderer, Fourcc::Argb8888, true);
            if let Err(e) = &captured {
                tracing::warn!("screencopy: {e}; reading back as RGBA and converting from now on (as in 9b)");
                streams.rgba_only = true;
            }
        }
        if streams.rgba_only && captured.is_err() {
            captured = capture(renderer, Fourcc::Abgr8888, false);
        }
        let (write, time) = match captured {
            Ok((Ok(write), time)) => (write, time),
            Ok((Err(e), _)) | Err(e) => {
                tracing::warn!("screencopy: {}: {e}, failed", output.name());
                copy.frame.failed();
                continue;
            }
        };
        copy.frame.flags(Flags::empty());
        for r in damage.iter().flatten() {
            copy.frame.damage(r.loc.x as u32, r.loc.y as u32, r.size.w as u32, r.size.h as u32);
        }
        let (sec, nsec) = monotonic_now();
        copy.frame.ready((sec >> 32) as u32, sec as u32, nsec);
        if damage.is_some() {
            let sample = Sample { waited: started - copy.queued, draw: time.draw, read_back: time.read_back, write, total: started.elapsed() };
            streams.summary.add(sample, now);
            if let Some(key) = &copy.stream {
                let n = damage.as_ref().map_or(0, Vec::len);
                tracing::debug!(
                    "screencopy: {key}: copied, {n} damage rects, ready sent (waited {:.0} ms, copy {:.1} ms)",
                    sample.waited.as_secs_f64() * 1000.0,
                    sample.total.as_secs_f64() * 1000.0
                );
            }
        } else {
            tracing::info!(
                "screencopy: copied {} {}x{} at {},{} in {:.1} ms",
                output.name(),
                region.size.w,
                region.size.h,
                region.loc.x,
                region.loc.y,
                started.elapsed().as_secs_f64() * 1000.0,
            );
        }
    }
    state.shots.copies = waiting;
    for key in streams.prune(now) {
        tracing::info!("screencopy: stream of {key} ended");
    }
    if let Some(line) = streams.summary.take_line(now, !streams.is_active()) {
        tracing::info!("{line}");
    }
    state.shots.streams = streams;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn regions_are_clipped() {
        let r = |x, y, w, h| Some(Rectangle::<i32, Physical>::new((x, y).into(), (w, h).into()));
        assert_eq!(clip_region(1440, 900, 0, 0, 1440, 900), r(0, 0, 1440, 900));
        assert_eq!(clip_region(1440, 900, 100, 50, 200, 100), r(100, 50, 200, 100));
        // Hanging off the right/bottom edge: cut there.
        assert_eq!(clip_region(1440, 900, 1400, 880, 200, 100), r(1400, 880, 40, 20));
        // Starting left of / above the output: cut at 0.
        assert_eq!(clip_region(1440, 900, -10, -20, 50, 50), r(0, 0, 40, 30));
        // Negative size: counted from the other corner.
        assert_eq!(clip_region(1440, 900, 300, 200, -100, -50), r(200, 150, 100, 50));
        // Entirely off the output, or empty.
        assert_eq!(clip_region(1440, 900, 1500, 0, 10, 10), None);
        assert_eq!(clip_region(1440, 900, 10, 10, 0, 10), None);
        assert_eq!(clip_region(1440, 900, i32::MAX - 5, 0, i32::MAX, 10), None);
    }

    #[test]
    fn buffers_must_fit() {
        assert!(buffer_fits(true, 200, 100, 800, 0, 80_000, 200, 100));
        assert!(buffer_fits(true, 200, 100, 832, 64, 64 + 83_200, 200, 100)); // padded rows, offset
        assert!(!buffer_fits(false, 200, 100, 800, 0, 80_000, 200, 100)); // format
        assert!(!buffer_fits(true, 201, 100, 804, 0, 80_400, 200, 100)); // size
        assert!(!buffer_fits(true, 200, 100, 799, 0, 80_000, 200, 100)); // stride
        assert!(!buffer_fits(true, 200, 100, 800, 4, 80_000, 200, 100)); // runs past the pool
        assert!(!buffer_fits(true, 200, 100, 800, -4, 80_000, 200, 100));
    }

    #[test]
    fn damage_cut_to_the_region() {
        let rect = |x, y, w, h| Rectangle::<i32, Physical>::new((x, y).into(), (w, h).into());
        let region = rect(100, 50, 200, 100);
        let damage = [rect(0, 0, 120, 60), rect(150, 70, 10, 10), rect(400, 0, 10, 10), rect(290, 140, 50, 50)];
        assert_eq!(region_damage(&damage, region), vec![rect(0, 0, 20, 10), rect(50, 20, 10, 10), rect(190, 90, 10, 10)]);
        assert!(region_damage(&[], region).is_empty());
        // Touching the edge only: nothing.
        assert!(region_damage(&[rect(300, 50, 10, 10)], region).is_empty());
    }

    #[test]
    fn streams_wait_for_damage_and_end_when_idle() {
        use smithay::backend::renderer::element::solid::SolidColorRenderElement;
        let t0 = Instant::now();
        let mut streams: Streams<u32> = Streams::default();
        assert!(!streams.is_active());
        let none: &[SolidColorRenderElement] = &[];
        // A new stream's first frame: everything is damage.
        let (tracker, new) = streams.touch(1, (1440, 900), t0);
        assert!(new);
        let (damage, _) = tracker.damage_output(1, none).unwrap();
        assert_eq!(damage.cloned(), Some(vec![Rectangle::from_size((1440, 900).into())]));
        // Nothing changed since: no damage, the copy would wait.
        let (tracker, new) = streams.touch(1, (1440, 900), t0);
        assert!(!new);
        assert!(tracker.damage_output(1, none).unwrap().0.is_none());
        // A new mode: a fresh tracker, everything again.
        let (tracker, _) = streams.touch(1, (1366, 768), t0);
        assert!(tracker.damage_output(1, none).unwrap().0.is_some());
        assert!(streams.has(&1) && streams.is_active());
        // Idle streams end; one still waiting (seen) stays.
        streams.touch(2, (1366, 768), t0);
        streams.seen(&2, t0 + Duration::from_secs(2));
        assert!(streams.prune(t0 + Duration::from_secs(1)).is_empty());
        assert_eq!(streams.prune(t0 + STREAM_IDLE), vec![1]);
        assert_eq!(streams.prune(t0 + Duration::from_secs(2) + STREAM_IDLE), vec![2]);
        assert!(!streams.is_active());
    }

    #[test]
    fn shared_outputs_once_each() {
        assert!(sharing_outputs(std::iter::empty()).is_empty());
        assert_eq!(sharing_outputs(["VGA-1", "HDMIA-1", "VGA-1"].into_iter()), ["HDMIA-1", "VGA-1"]);
    }

    #[test]
    fn summaries() {
        let ms = Duration::from_millis;
        let t0 = Instant::now();
        let mut summary = Summary::default();
        assert_eq!(summary.take_line(t0, true), None);
        for i in 1..=10u64 {
            summary.add(Sample { waited: ms(i), draw: ms(1), read_back: ms(20 + i), write: ms(2), total: ms(25 + i) }, t0);
        }
        assert_eq!(summary.take_line(t0 + ms(5000), false), None);
        assert_eq!(
            summary.take_line(t0 + SUMMARY_EVERY, false).as_deref(),
            Some("screencopy: 10 stream copies in 10.0 s (1.0/s); waited for damage p50 5.0 ms; copy p50 30.0 / p95 35.0 ms (draw 1.0 / read back 25.0 / write 2.0 ms p50)")
        );
        assert_eq!(summary.take_line(t0 + SUMMARY_EVERY * 2, true), None);
        // When the streams end, the rest goes out at once.
        summary.add(Sample { waited: ms(0), draw: ms(1), read_back: ms(2), write: ms(1), total: ms(4) }, t0);
        assert!(summary.take_line(t0 + ms(2000), true).unwrap().starts_with("screencopy: 1 stream copies in 2.0 s"));
    }

    #[test]
    fn region_copied_as_xrgb() {
        // A 3x2 RGBA source; pixel (x, y) = [10x+y, 1, 2, 128].
        let mut src = Vec::new();
        for y in 0..2u8 {
            for x in 0..3u8 {
                src.extend([10 * x + y, 1, 2, 128]);
            }
        }
        let region = Rectangle::<i32, Physical>::new((1, 0).into(), (2, 2).into());
        let stride = 12; // 2 px + 4 bytes of padding
        let mut dst = vec![0u8; stride * 2];
        copy_region_xrgb(&src, 3, region, &mut dst, stride);
        assert_eq!(&dst[0..8], &[2, 1, 10, 0xff, 2, 1, 20, 0xff]);
        assert_eq!(&dst[8..12], &[0, 0, 0, 0]); // padding untouched
        assert_eq!(&dst[12..20], &[2, 1, 11, 0xff, 2, 1, 21, 0xff]);
    }

    #[test]
    fn bgra_rows_copied_as_they_are() {
        // A 3x2 BGRA source; pixel (x, y) = [10x+y, 1, 2, 255].
        let mut src = Vec::new();
        for y in 0..2u8 {
            for x in 0..3u8 {
                src.extend([10 * x + y, 1, 2, 255]);
            }
        }
        let region = Rectangle::<i32, Physical>::new((1, 1).into(), (2, 1).into());
        let stride = 12;
        let mut dst = vec![7u8; stride];
        copy_region_rows(&src, 3, region, &mut dst, stride);
        assert_eq!(&dst[0..8], &[11, 1, 2, 255, 21, 1, 2, 255]);
        assert_eq!(&dst[8..12], &[7, 7, 7, 7]); // padding untouched
        // The same picture read back as RGBA and converted ends up the same.
        let rgba: Vec<u8> = src.chunks(4).flat_map(|p| [p[2], p[1], p[0], p[3]]).collect();
        let mut converted = vec![7u8; stride];
        copy_region_xrgb(&rgba, 3, region, &mut converted, stride);
        assert_eq!(dst, converted);
    }
}
