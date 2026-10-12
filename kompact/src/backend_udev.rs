//! Direct DRM/KMS backend for physical Linux hardware.
//!
//! This backend intentionally starts in a conservative mode:
//! - obtains the GPU through libseat instead of assuming /dev/dri/card0
//! - requires an active libseat session
//! - selects a connected connector and a compatible CRTC
//! - uses Smithay's DrmOutputManager for device-wide DRM synchronization
//! - disables overlay/cursor planes initially (software cursor is composited)
//! - disables direct scan-out initially; this is much safer while bringing up
//!   older/quirky DRM drivers such as nouveau on a GT 710
//! - treats DRM/page-flip errors as errors instead of silently retrying forever
//!
//! Once the compositor is stable on real hardware, individual optimizations can
//! be enabled deliberately.
//!
//! Phase 5 (multi-monitor, hotplug) is chosen with `KOMPACT_OUTPUTS` (see `outputs.rs`); since
//! 5f the default is `all`. `first` takes the startup and render path of before Phase 5: one
//! connector (the primary, 5e), no udev watching, any DRM error fatal. `observe` adds a udev watch that only logs
//! connectors; `reconnect` and `all` set outputs up and shut them down on hotplug events (never
//! in a loop: a connector whose output failed isn't retried until the next hotplug event).

use std::{
    cell::RefCell,
    path::PathBuf,
    rc::Rc,
    time::{Duration, Instant},
};

use smithay::{
    backend::{
        allocator::{
            gbm::{GbmAllocator, GbmBufferFlags, GbmDevice},
            Fourcc,
        },
        drm::{
            compositor::{FrameError, FrameFlags, PrimaryPlaneElement},
            output::{DrmOutput, DrmOutputManager, DrmOutputRenderElements},
            DrmDevice, DrmDeviceFd, DrmError, DrmEvent, DrmEventTime,
        },
        egl::{EGLContext, EGLDisplay},
        libinput::{LibinputInputBackend, LibinputSessionInterface},
        renderer::gles::GlesRenderer,
        session::{libseat::LibSeatSession, Event as SessionEvent, Session},
        udev::{primary_gpu, UdevBackend, UdevEvent},
    },
    desktop::layer_map_for_output,
    output::{Mode as OutputMode, Output, PhysicalProperties, Subpixel},
    reexports::{
        calloop::{
            timer::{TimeoutAction, Timer},
            EventLoop,
        },
        drm::Device as DrmControlDevice,
        drm::control::{connector, crtc, Device as ControlDevice, Mode, ModeTypeFlags},
        input::Libinput,
        rustix::fs::OFlags,
        wayland_server::{backend::GlobalId, Display},
    },
    utils::{DeviceFd, Logical, Rectangle, Size, Transform},
};

use crate::{
    outputs::{self, Mode as OutputsMode},
    render::KompactElement,
    state::{ClientState, Kompact},
    Ctx,
};

type GbmAllocatorType = GbmAllocator<DrmDeviceFd>;
type GbmExporterType = GbmDevice<DrmDeviceFd>;
type GbmOutputManager = DrmOutputManager<GbmAllocatorType, GbmExporterType, (), DrmDeviceFd>;
type GbmOutput = DrmOutput<GbmAllocatorType, GbmExporterType, (), DrmDeviceFd>;
type DrmRenderElements = DrmOutputRenderElements<GlesRenderer, KompactElement>;

const CLEAR_COLOR: [f32; 4] = [0.1, 0.1, 0.15, 1.0];

/// One output kompact drives: a connector on a CRTC (Phase 5: there can be several).
struct Head {
    connector: connector::Handle,
    crtc: crtc::Handle,
    mode: Mode,
    drm: GbmOutput,
    output: Output,
    global: GlobalId,
    flip_pending: bool,
    /// 12i-3: turned off (idle): DPMS off through Smithay's `DrmCompositor::clear()`, not
    /// rendered until it's turned on; then `reset_buffer_ages()` makes the next frame a full
    /// one, and its page flip turns DPMS back on (Smithay's legacy surface does that).
    screen_off: bool,
}

struct DrmBackend {
    renderer: GlesRenderer,
    manager: GbmOutputManager,
    /// In layout order (left to right).
    heads: Vec<Head>,
    /// The main monitor (`outputs::pick_primary` at startup): the one perf.rs times. Another
    /// head takes over if it's shut down.
    primary: connector::Handle,
    active: bool,
    outputs_mode: OutputsMode,
    /// Connectors whose output failed since the last hotplug event; not retried until the next.
    failed: Vec<u32>,
    /// 12i-3: idle screens may be turned off (`KOMPACT_SCREEN_OFF=off`: never; also after a
    /// failed try).
    screen_off_allowed: bool,
    /// No drawing between our own `change_vt` and the seat's answer (`vt.rs`).
    vt_gate: crate::vt::SwitchGate,
}

/// Render one head. Unchanged from the single-output backend apart from `head`/`timed`.
/// `Ok(true)`: the page flip was refused because another session has the screen (`vt.rs`).
fn render_head(renderer: &mut GlesRenderer, head: &mut Head, state: &Kompact, timed: bool) -> anyhow::Result<bool> {
    // Phase 4a: timing only, see perf.rs. Only the first output is timed (perf.rs follows one
    // swapchain).
    let started = Instant::now();
    let elements = crate::render::build_elements(renderer, state, &head.output);

    // Conservative first implementation for real hardware. In particular,
    // don't ask nouveau to direct-scanout client buffers while bringing up
    // the physical DRM path.
    let frame = head
        .drm
        .render_frame(renderer, &elements, CLEAR_COLOR, FrameFlags::empty())
        .map_err(|e| anyhow::anyhow!("DRM render_frame failed: {e:?}"))?;

    if frame.is_empty {
        if timed {
            state.frame_timer.borrow_mut().rendered(started, Instant::now(), false);
        }
        return Ok(false);
    }

    if frame.needs_sync() {
        if let PrimaryPlaneElement::Swapchain(primary) = frame.primary_element {
            primary
                .sync
                .wait()
                .map_err(|e| anyhow::anyhow!("primary-plane sync failed: {e:?}"))?;
        }
    }

    if let Err(e) = head.drm.queue_frame(()) {
        let failure = match &e {
            FrameError::DrmError(DrmError::DeviceInactive) => crate::vt::flip_failure(true, None),
            FrameError::DrmError(DrmError::Access(a)) => crate::vt::flip_failure(false, Some(&a.source)),
            _ => crate::vt::FlipFailure::Fatal,
        };
        if failure == crate::vt::FlipFailure::SeatTaken {
            tracing::warn!("vt: page flip refused, another session has the screen; waiting for the seat ({e:?})");
            return Ok(true);
        }
        return Err(anyhow::anyhow!("DRM page-flip queue failed: {e:?}"));
    }

    head.flip_pending = true;
    if timed {
        state.frame_timer.borrow_mut().rendered(started, Instant::now(), true);
    }
    tracing::debug!("DRM page flip queued");
    Ok(false)
}

impl DrmBackend {
    /// Render every output that isn't waiting for a flip. A DRM error is fatal (`Err`) unless
    /// the outputs mode survives output errors; then the failed outputs' connectors are returned
    /// for the caller to shut down.
    fn render(&mut self, state: &Kompact) -> anyhow::Result<Vec<connector::Handle>> {
        let mut failed = Vec::new();
        if !self.active || !self.vt_gate.may_render(Instant::now()) {
            return Ok(failed);
        }
        let DrmBackend { renderer, heads, outputs_mode, active, primary, .. } = self;
        for head in heads.iter_mut() {
            // 12i-3: nothing is drawn on a screen that's off.
            if head.flip_pending || head.screen_off {
                continue;
            }
            let timed = head.connector == *primary;
            match render_head(renderer, head, state, timed) {
                Ok(false) => {}
                Ok(true) => {
                    // vt.rs: no more frames until the seat's ActivateSession sets this again.
                    *active = false;
                    break;
                }
                Err(e) => {
                    if !outputs_mode.survives_output_errors() {
                        return Err(e);
                    }
                    tracing::error!("output {} failed: {e:#}; shutting it down until the next hotplug event", head.output.name());
                    failed.push(head.connector);
                }
            }
        }
        Ok(failed)
    }

    /// 12i-3: turn the screens off or on as `state.screens_off` says (idle). Off only once a
    /// head's page flip is done (until then, the next turn); not while the session is inactive
    /// (VT switched away). `KOMPACT_SCREEN_OFF=off` leaves them on.
    fn apply_screen_power(&mut self, off: bool) {
        if !self.active || !self.screen_off_allowed {
            return;
        }
        for head in &mut self.heads {
            if off && !head.screen_off && !head.flip_pending {
                match head.drm.with_compositor(|c| c.clear()) {
                    Ok(()) => {
                        head.screen_off = true;
                        tracing::info!("screen {} off (DPMS)", head.output.name());
                    }
                    Err(e) => {
                        // Stays on and keeps being drawn; no more tries this session.
                        tracing::warn!("screen {}: can't turn it off: {e:?}; screens stay on this session", head.output.name());
                        self.screen_off_allowed = false;
                        return;
                    }
                }
            } else if !off && head.screen_off {
                // `clear()` doesn't touch the damage history: with nothing changed since, the
                // next frame would be empty, never flipped, and the screen would stay dark.
                // Forgetting the buffer ages makes it a full frame, whose flip turns DPMS on.
                head.drm.with_compositor(|c| c.reset_buffer_ages());
                head.screen_off = false;
                tracing::info!("screen {} on (full frame next; its page flip turns DPMS on)", head.output.name());
            }
        }
    }

    /// 6m-d3: one window thumbnail, off screen (`thumbnail.rs`). Not while the session is
    /// inactive (VT switched away).
    fn capture_thumbnail(&mut self, state: &mut Kompact) {
        if self.active && self.vt_gate.may_render(Instant::now()) {
            crate::screenshot::capture_pending(&mut self.renderer, state); // 9a, same way, full size
            crate::thumbnail::capture_next(&mut self.renderer, state);
        }
    }

    /// A page flip completed on `crtc`. Returns whether it was the timed (primary) output.
    fn flip_done(&mut self, crtc: crtc::Handle) -> Option<bool> {
        let i = self.heads.iter().position(|h| h.crtc == crtc)?;
        let head = &mut self.heads[i];
        if let Err(e) = head.drm.frame_submitted() {
            tracing::error!("DRM frame_submitted failed: {e:?}");
            head.flip_pending = false;
            return None;
        }
        head.flip_pending = false;
        Some(head.connector == self.primary)
    }

    fn driven(&self) -> Vec<outputs::Driven> {
        self.heads
            .iter()
            .map(|h| outputs::Driven { connector: h.connector.into(), crtc: h.crtc.into(), mode: video_mode(&h.mode) })
            .collect()
    }

    fn geometries(&self, state: &Kompact) -> Vec<(Output, Rectangle<i32, Logical>)> {
        self.heads
            .iter()
            .filter_map(|h| Some((h.output.clone(), state.space.output_geometry(&h.output)?)))
            .collect()
    }

    /// Phase 5: look at every connector after a hotplug event (or at startup / VT return) and,
    /// if the outputs mode drives hotplug, set outputs up or shut them down to match.
    fn rescan(&mut self, state: &mut Kompact, why: &str) {
        // While on another VT the device can't be touched; returning always rescans.
        if !self.active {
            return;
        }
        // The switch's own change event comes before the seat pauses us (`vt.rs`); setting
        // outputs up or down then would race the seat. The scan on return covers it.
        if self.vt_gate.switching(Instant::now()) {
            tracing::info!("connector scan ({why}) skipped: a VT switch is under way");
            return;
        }
        let scan = match scan_connectors(self.manager.device()) {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!("connector scan ({why}) failed: {e:#}");
                return;
            }
        };
        for c in &scan {
            let mode = c.info.mode.map_or("no modes".to_string(), |m| {
                format!("{}x{}@{}Hz", m.w, m.h, m.refresh_mhz / 1000)
            });
            tracing::info!(
                "connector {} ({why}): {}, {mode}, crtcs {:?}",
                c.info.name,
                if c.info.connected { "connected" } else { "disconnected" },
                c.info.crtcs
            );
        }
        if !self.outputs_mode.drives_hotplug() {
            return;
        }
        let infos: Vec<outputs::Connector> = scan.iter().map(|c| c.info.clone()).collect();
        let plan = outputs::plan(&self.driven(), &infos, self.outputs_mode.max_outputs(), &self.failed);
        if plan.is_empty() {
            tracing::info!("outputs unchanged ({why})");
            return;
        }
        tracing::info!("outputs ({why}): shutting down {:?}, setting up {:?}", plan.remove, plan.add);
        let before = self.geometries(state);
        for id in &plan.remove {
            self.remove_head(*id, state);
        }
        for (id, crtc) in &plan.add {
            let Some(entry) = scan.iter().find(|c| c.info.id == *id) else { continue };
            let Some(crtc) = entry.crtcs.iter().copied().find(|c| u32::from(*c) == *crtc) else { continue };
            if let Err(e) = self.add_head(entry, crtc, state) {
                tracing::error!("could not set up output on {}: {e:#}; not retried until the next hotplug event", entry.info.name);
                self.failed.push(*id);
            }
        }
        self.relayout(state, before);
    }

    /// Shut down the outputs on these connectors (they failed while rendering).
    fn drop_failed(&mut self, failed: &[connector::Handle], state: &mut Kompact) {
        let before = self.geometries(state);
        for c in failed {
            self.failed.push((*c).into());
            self.remove_head((*c).into(), state);
        }
        self.relayout(state, before);
    }

    fn add_head(&mut self, entry: &Scanned, crtc: crtc::Handle, state: &mut Kompact) -> anyhow::Result<()> {
        let mode = entry.mode.ok_or_else(|| anyhow::anyhow!("connector has no usable mode"))?;
        let output = new_output(&entry.conn, &mode);
        let mut planes = self.manager.device().planes(&crtc).ctx()?;
        planes.cursor.clear();
        planes.overlay.clear();
        if planes.primary.is_empty() {
            anyhow::bail!("CRTC {crtc:?} has no primary DRM plane");
        }
        // initialize_output may re-commit the other outputs to make room (bandwidth fallbacks),
        // so it gets their current contents; the new output starts with just the clear colour.
        let mut elements = DrmRenderElements::new();
        for h in &self.heads {
            let els = crate::render::build_elements(&mut self.renderer, state, &h.output);
            elements.add_output(&h.crtc, CLEAR_COLOR.into(), els);
        }
        elements.add_output(&crtc, CLEAR_COLOR.into(), std::iter::empty::<KompactElement>());
        let global = output.create_global::<Kompact>(&state.display_handle);
        let drm = match self.manager.initialize_output(
            crtc,
            mode,
            &[entry.handle],
            &output,
            Some(planes),
            &mut self.renderer,
            &elements,
        ) {
            Ok(drm) => drm,
            Err(e) => {
                state.display_handle.remove_global::<Kompact>(global);
                anyhow::bail!("failed to initialize DRM output: {e:?}");
            }
        };
        tracing::info!(
            "output {} set up on {crtc:?}: {}x{}@{}Hz",
            output.name(),
            mode.size().0,
            mode.size().1,
            mode.vrefresh()
        );
        self.heads.push(Head {
            connector: entry.handle,
            crtc,
            mode,
            drm,
            output,
            global,
            flip_pending: false,
            screen_off: false,
        });
        Ok(())
    }

    fn remove_head(&mut self, connector_id: u32, state: &mut Kompact) {
        let Some(i) = self.heads.iter().position(|h| u32::from(h.connector) == connector_id) else { return };
        let head = self.heads.remove(i);
        let name = head.output.name();
        // Panels etc. on it are asked to close; they can come back on another output.
        {
            let mut map = layer_map_for_output(&head.output);
            let layers: Vec<_> = map.layers().cloned().collect();
            for layer in layers {
                layer.layer_surface().send_close();
                map.unmap_layer(&layer);
            }
        }
        state.space.unmap_output(&head.output);
        state.display_handle.remove_global::<Kompact>(head.global);
        // Dropping the DrmOutput turns its CRTC off (Smithay's surface disables the connectors
        // and does a null commit).
        drop(head.drm);
        tracing::info!("output {name} shut down");
    }

    /// Put the outputs side by side in connector order, tell the shell where things went.
    fn relayout(&mut self, state: &mut Kompact, before: Vec<(Output, Rectangle<i32, Logical>)>) {
        self.heads.sort_by_key(|h| u32::from(h.connector));
        let sizes: Vec<Size<i32, Logical>> = self
            .heads
            .iter()
            .map(|h| Size::from((h.mode.size().0 as i32, h.mode.size().1 as i32)))
            .collect();
        for (h, pos) in self.heads.iter().zip(outputs::layout(&sizes)) {
            h.output.change_current_state(None, None, None, Some(pos));
            state.space.map_output(&h.output, pos);
        }
        state.outputs = self.heads.iter().map(|h| h.output.clone()).collect();
        let moves: Vec<_> = before
            .iter()
            .map(|(o, old)| (*old, state.outputs.contains(o).then(|| state.space.output_geometry(o)).flatten()))
            .collect();
        state.outputs_changed(&moves);
        // perf.rs times the primary output; if it was shut down, the leftmost one takes over.
        if !self.heads.iter().any(|h| h.connector == self.primary) {
            if let Some(h) = self.heads.first() {
                tracing::info!("primary monitor is now {}", h.output.name());
                self.primary = h.connector;
            }
        }
        let timer = state.frame_timer.get_mut();
        timer.reset();
        if let Some(h) = self.heads.iter().find(|h| h.connector == self.primary) {
            timer.set_refresh_mhz((h.mode.vrefresh() * 1000) as i32);
        }
        let layout: Vec<String> = self
            .heads
            .iter()
            .filter_map(|h| Some(format!("{} at {:?}", h.output.name(), state.space.output_geometry(&h.output)?)))
            .collect();
        if layout.is_empty() {
            tracing::warn!("no outputs left; windows keep their places until a monitor is connected");
        } else {
            tracing::info!("output layout: {}", layout.join(", "));
        }
    }
}

/// A connector as found by a scan, with the DRM handles behind `outputs::Connector`.
struct Scanned {
    info: outputs::Connector,
    handle: connector::Handle,
    conn: connector::Info,
    mode: Option<Mode>,
    crtcs: Vec<crtc::Handle>,
}

fn video_mode(mode: &Mode) -> outputs::VideoMode {
    let (w, h) = mode.size();
    outputs::VideoMode { w: w as i32, h: h as i32, refresh_mhz: mode.vrefresh() * 1000 }
}

fn output_name(conn: &connector::Info) -> String {
    format!("{:?}-{}", conn.interface(), conn.interface_id())
}

/// The mode kompact uses for a connector: its preferred one, else the first.
fn preferred_mode(conn: &connector::Info) -> Option<Mode> {
    conn.modes()
        .iter()
        .find(|mode| mode.mode_type().contains(ModeTypeFlags::PREFERRED))
        .or_else(|| conn.modes().first())
        .cloned()
}

/// 12i-3: `KOMPACT_SCREEN_OFF=off` keeps the screens on whatever Settings says.
fn screen_off_allowed() -> bool {
    let allowed = !std::env::var("KOMPACT_SCREEN_OFF").is_ok_and(|v| v.trim().eq_ignore_ascii_case("off"));
    if !allowed {
        tracing::info!("screens off when idle: disabled (KOMPACT_SCREEN_OFF=off)");
    }
    allowed
}

/// A wl_output for a connector, at 0,0 until `relayout` places it (same setup as at startup).
fn new_output(conn: &connector::Info, mode: &Mode) -> Output {
    let (w, h) = mode.size();
    let output_mode = OutputMode { size: (w as i32, h as i32).into(), refresh: (mode.vrefresh() * 1000) as i32 };
    let (pw, ph) = conn.size().unwrap_or((0, 0));
    let output = Output::new(
        output_name(conn),
        PhysicalProperties {
            size: (pw as i32, ph as i32).into(),
            subpixel: Subpixel::Unknown,
            make: "kompact".into(),
            model: "drm".into(),
        },
    );
    output.change_current_state(Some(output_mode), Some(Transform::Normal), None, Some((0, 0).into()));
    output.set_preferred(output_mode);
    output
}

/// Every connector's state, as the kernel last probed it (no forced re-probe: the hotplug
/// event already means the kernel has looked).
fn scan_connectors(device: &DrmDevice) -> anyhow::Result<Vec<Scanned>> {
    let resources = device.resource_handles().ctx()?;
    let mut out = Vec::new();
    for &handle in resources.connectors() {
        let Ok(conn) = device.get_connector(handle, false) else { continue };
        // Same CRTC preference as `choose_connector_and_crtc`: the one it's on now if that's
        // compatible, then the encoders' possible ones in order.
        let compatible: Vec<crtc::Handle> = conn
            .encoders()
            .iter()
            .filter_map(|&e| device.get_encoder(e).ok())
            .flat_map(|e| resources.filter_crtcs(e.possible_crtcs()))
            .collect();
        let current = conn
            .current_encoder()
            .and_then(|e| device.get_encoder(e).ok())
            .and_then(|e| e.crtc())
            .filter(|c| compatible.contains(c));
        let mut crtcs: Vec<crtc::Handle> = Vec::new();
        for c in current.into_iter().chain(compatible) {
            if !crtcs.contains(&c) {
                crtcs.push(c);
            }
        }
        let mode = preferred_mode(&conn);
        let connected = conn.state() == connector::State::Connected;
        out.push(Scanned {
            info: outputs::Connector {
                id: handle.into(),
                name: output_name(&conn),
                connected,
                mode: if connected { mode.as_ref().map(video_mode) } else { None },
                crtcs: crtcs.iter().map(|c| u32::from(*c)).collect(),
            },
            handle,
            conn,
            mode,
            crtcs,
        });
    }
    Ok(out)
}

fn drm_device_from_environment_or_primary(session: &LibSeatSession) -> anyhow::Result<PathBuf> {
    if let Some(path) = std::env::var_os("KOMPACT_DRM_DEVICE") {
        let path = PathBuf::from(path);
        if !path.exists() {
            anyhow::bail!("KOMPACT_DRM_DEVICE does not exist: {path:?}");
        }
        tracing::info!(?path, "using DRM device selected by KOMPACT_DRM_DEVICE");
        return Ok(path);
    }

    let path = primary_gpu(session.seat())
        .map_err(|e| anyhow::anyhow!("failed to query primary GPU for seat {}: {e:?}", session.seat()))?
        .ok_or_else(|| {
            anyhow::anyhow!(
                "libseat did not report a primary GPU for seat {}; refusing to guess /dev/dri/card0",
                session.seat()
            )
        })?;

    tracing::info!(?path, "using libseat primary GPU");
    Ok(path)
}

fn choose_connector_and_crtc(
    drm_device: &DrmDevice,
) -> anyhow::Result<(connector::Info, crtc::Handle, Mode)> {
    let resources = drm_device.resource_handles().ctx()?;

    // Every connected monitor with a mode, in connector order; the main one is picked from
    // these (Phase 5: `KOMPACT_PRIMARY`, else the largest; with one monitor, that one).
    let monitors: Vec<(connector::Info, Mode)> = resources
        .connectors()
        .iter()
        .filter_map(|&handle| drm_device.get_connector(handle, false).ok())
        .filter(|connector| connector.state() == connector::State::Connected)
        .filter_map(|connector| preferred_mode(&connector).map(|mode| (connector, mode)))
        .collect();
    let names: Vec<String> = monitors.iter().map(|(c, _)| output_name(c)).collect();
    let candidates: Vec<(&str, outputs::VideoMode)> =
        names.iter().zip(&monitors).map(|(n, (_, m))| (n.as_str(), video_mode(m))).collect();
    let wanted = std::env::var("KOMPACT_PRIMARY").ok();
    let (index, why) = outputs::pick_primary(&candidates, wanted.as_deref())
        .ok_or_else(|| anyhow::anyhow!("no connected DRM connector with modes was found"))?;
    let (connector_info, mode) = monitors.into_iter().nth(index).expect("index from pick_primary");
    let how = match why {
        outputs::PrimaryChoice::Named => "named by KOMPACT_PRIMARY".to_string(),
        outputs::PrimaryChoice::Largest => {
            format!("detected: largest of {} (KOMPACT_PRIMARY=<name> to choose)", names.join(", "))
        }
        outputs::PrimaryChoice::NamedMissing => format!(
            "KOMPACT_PRIMARY={:?} isn't connected; largest of {}",
            wanted.unwrap_or_default(),
            names.join(", ")
        ),
    };
    tracing::info!("primary monitor: {} {}x{} ({how})", names[index], mode.size().0, mode.size().1);

    // Prefer the currently associated CRTC if it is compatible, otherwise find
    // a CRTC through the connector's encoders. This avoids blindly assuming
    // CRTC 0/CRTC 1 and works across physical GPUs with different layouts.
    let compatible: Vec<_> = connector_info
        .encoders()
        .iter()
        .filter_map(|&encoder_handle| drm_device.get_encoder(encoder_handle).ok())
        .flat_map(|encoder| resources.filter_crtcs(encoder.possible_crtcs()))
        .collect();

    let current = connector_info
        .current_encoder()
        .and_then(|encoder| drm_device.get_encoder(encoder).ok())
        .and_then(|encoder| encoder.crtc())
        .filter(|crtc| compatible.contains(crtc));

    let crtc = current
        .or_else(|| compatible.first().copied())
        .ok_or_else(|| anyhow::anyhow!("no CRTC compatible with the connected connector"))?;

    tracing::info!(
        connector = ?connector_info.handle(),
        interface = ?connector_info.interface(),
        interface_id = connector_info.interface_id(),
        ?crtc,
        mode = ?mode,
        "selected DRM output"
    );

    Ok((connector_info, crtc, mode))
}

pub fn run(spawn_cmd: String) -> anyhow::Result<()> {
    tracing::info!("starting kompact physical DRM/KMS backend");

    // Phase 5: which outputs to drive and whether to follow hotplug (outputs.rs).
    let outputs_mode = OutputsMode::from_env(std::env::var("KOMPACT_OUTPUTS").ok().as_deref())
        .map_err(|v| anyhow::anyhow!("KOMPACT_OUTPUTS={v:?}: expected all (default), first, observe or reconnect"))?;
    tracing::info!("outputs mode: {outputs_mode:?} (KOMPACT_OUTPUTS; default all since 5f, first for one screen)");

    let mut event_loop: EventLoop<Kompact> = EventLoop::try_new()?;
    let mut display: Display<Kompact> = Display::new()?;

    // libseat is the authority for VT/seat ownership. Do not bypass it and do
    // not silently fall back to root/card0 access.
    let (mut session, session_notifier) = LibSeatSession::new().map_err(|e| {
        anyhow::anyhow!(
            "failed to open libseat session: {e:?}; run Kompact from a real TTY with logind/seatd available"
        )
    })?;

    if !session.is_active() {
        anyhow::bail!(
            "libseat session is not active; another graphical session currently owns seat {}",
            session.seat()
        );
    }

    let gpu_path = drm_device_from_environment_or_primary(&session)?;

    let fd = session
        .open(&gpu_path, OFlags::RDWR | OFlags::CLOEXEC)
        .map_err(|e| anyhow::anyhow!("failed to open DRM device {gpu_path:?} through libseat: {e:?}"))?;
    let drm_fd = DrmDeviceFd::new(DeviceFd::from(fd));

    let (drm_device, drm_notifier) = DrmDevice::new(drm_fd.clone(), true).ctx()?;

    if let Ok(driver) = DrmControlDevice::get_driver(&drm_device) {
        tracing::info!(
            name = ?driver.name(),
            description = ?driver.description(),
            "DRM driver detected"
        );
    }

    let gbm = GbmDevice::new(drm_fd.clone()).ctx()?;
    tracing::info!("GBM device initialized");

    let egl_display = unsafe { EGLDisplay::new(gbm.clone()) }.ctx()?;
    let egl_context = EGLContext::new(&egl_display).ctx()?;
    let mut renderer = unsafe { GlesRenderer::new(egl_context) }.ctx()?;
    tracing::info!("EGL/GLES renderer initialized");

    let (connector_info, crtc, mode) = choose_connector_and_crtc(&drm_device)?;

    let (w, h) = mode.size();
    let output_mode = OutputMode {
        size: (w as i32, h as i32).into(),
        refresh: (mode.vrefresh() * 1000) as i32,
    };

    let (pw, ph) = connector_info.size().unwrap_or((0, 0));
    let output = Output::new(
        format!("{:?}-{}", connector_info.interface(), connector_info.interface_id()),
        PhysicalProperties {
            size: (pw as i32, ph as i32).into(),
            subpixel: Subpixel::Unknown,
            make: "kompact".into(),
            model: "drm".into(),
        },
    );
    let global = output.create_global::<Kompact>(&display.handle());
    output.change_current_state(
        Some(output_mode),
        Some(Transform::Normal),
        None,
        Some((0, 0).into()),
    );
    output.set_preferred(output_mode);

    // Build a conservative plane set. We already draw the software cursor as
    // a normal render element, so a hardware cursor plane is unnecessary.
    // Disabling overlay/cursor planes also avoids a class of atomic-test and
    // nouveau-specific plane problems while bringing up the compositor.
    let mut planes = drm_device.planes(&crtc).ctx()?;
    tracing::info!(
        primary_planes = planes.primary.len(),
        cursor_planes = planes.cursor.len(),
        overlay_planes = planes.overlay.len(),
        "DRM planes discovered"
    );
    planes.cursor.clear();
    planes.overlay.clear();

    if planes.primary.is_empty() {
        anyhow::bail!("selected CRTC has no primary DRM plane");
    }

    let allocator = GbmAllocator::new(
        gbm.clone(),
        GbmBufferFlags::RENDERING | GbmBufferFlags::SCANOUT,
    );

    let render_formats: Vec<_> = renderer
        .egl_context()
        .dmabuf_render_formats()
        .iter()
        .copied()
        .collect();

    if render_formats.is_empty() {
        anyhow::bail!("EGL/GLES reported no DMA-BUF render formats");
    }

    let mut manager = DrmOutputManager::new(
        drm_device,
        allocator,
        gbm.clone(),
        Some(gbm.clone()),
        [Fourcc::Xrgb8888, Fourcc::Argb8888],
        render_formats,
    );

    // initialize_output expects a render-element set because it may need to
    // perform fallback commits while establishing the CRTC.
    let init_elements = DrmRenderElements::new();
    let mut init_elements = init_elements;
    init_elements.add_output(
        &crtc,
        [0.1, 0.1, 0.15, 1.0].into(),
        std::iter::empty::<KompactElement>(),
    );

    let drm_output = manager
        .initialize_output(
            crtc,
            mode,
            &[connector_info.handle()],
            &output,
            Some(planes),
            &mut renderer,
            &init_elements,
        )
        .map_err(|e| anyhow::anyhow!("failed to initialize DRM output: {e:?}"))?;

    tracing::info!("DRM/KMS output initialized successfully");

    let backend = Rc::new(RefCell::new(DrmBackend {
        renderer,
        manager,
        heads: vec![Head {
            connector: connector_info.handle(),
            crtc,
            mode,
            drm: drm_output,
            output: output.clone(),
            global,
            flip_pending: false,
            screen_off: false,
        }],
        primary: connector_info.handle(),
        active: true,
        outputs_mode,
        failed: Vec::new(),
        screen_off_allowed: screen_off_allowed(),
        vt_gate: Default::default(),
    }));

    let mut state = Kompact::new(&event_loop, &mut display, spawn_cmd);
    state.space.map_output(&output, (0, 0));
    state.outputs.push(output.clone());
    state.frame_timer.get_mut().set_refresh_mhz(output_mode.refresh);

    // 6p-f (on unless KOMPACT_DMABUF=off): linux-dmabuf, so apps can hand over GPU buffers. Setup only: the buffers
    // become GL textures, never KMS planes (frames still use FrameFlags::empty()).
    if crate::dmabuf::enabled() {
        use smithay::backend::renderer::ImportDma;
        match drm_fd.dev_id() {
            Ok(dev) => {
                let formats = backend.borrow().renderer.dmabuf_formats();
                let b = backend.clone();
                let import = Box::new(move |d: &smithay::backend::allocator::dmabuf::Dmabuf| match b.try_borrow_mut() {
                    Ok(mut b) => b.renderer.import_dmabuf(d, None).is_ok(),
                    // Not expected (clients are dispatched outside any borrow); it's checked again when drawn.
                    Err(_) => true,
                });
                state.offer_dmabuf(&display.handle(), dev, formats, import);
            }
            Err(e) => tracing::warn!("dmabuf: not offered, no device id: {e}"),
        }
    } else {
        tracing::info!("dmabuf: not offered, apps use shared memory (KOMPACT_DMABUF=off)");
    }

    // Phase 5: log every connector, and in `all` mode set up the other connected ones.
    if outputs_mode.watches_hotplug() {
        backend.borrow_mut().rescan(&mut state, "startup");
        // Several monitors: start the pointer on the primary one, so the shell's panel and dock
        // (put on the output under the pointer) and the first windows open there.
        let b = backend.borrow();
        if b.heads.len() > 1 {
            let primary = b.heads.iter().find(|h| h.connector == b.primary).map(|h| h.output.clone());
            if let Some(geo) = primary.as_ref().and_then(|o| state.space.output_geometry(o)) {
                let centre = geo.loc.to_f64() + geo.size.to_f64().downscale(2.0).to_point();
                if let Some(pointer) = state.seat.get_pointer() {
                    pointer.set_location(centre);
                }
                tracing::info!("pointer starts on the primary monitor {} at {centre:?}", primary.unwrap().name());
            }
        }
    }

    // libinput is attached to the same libseat session as the DRM device.
    let mut libinput = Libinput::new_with_udev(LibinputSessionInterface::from(session.clone()));
    libinput
        .udev_assign_seat(&session.seat())
        .map_err(|_| anyhow::anyhow!("failed to assign libinput to seat {}", session.seat()))?;

    event_loop
        .handle()
        .insert_source(
            LibinputInputBackend::new(libinput.clone()),
            |event, _, state: &mut Kompact| state.process_input_event(event),
        )
        .ctx()?;

    // A DRM page flip is completed by VBlank. Only then may the swapchain
    // buffer be released and another frame queued.
    {
        let backend = backend.clone();
        event_loop
            .handle()
            .insert_source(drm_notifier, move |event, metadata, state: &mut Kompact| {
                match event {
                    DrmEvent::VBlank(crtc) => {
                        let received = Instant::now();
                        let mut b = backend.borrow_mut();
                        match b.flip_done(crtc) {
                            // Phase 4a timing follows the first output only.
                            Some(true) => {
                                // Phase 4a: the kernel's flip timestamp and vblank sequence.
                                let vblank = metadata.as_ref().map(|m| {
                                    let (time, monotonic) = match m.time {
                                        DrmEventTime::Monotonic(d) => (d, true),
                                        DrmEventTime::Realtime(t) => (
                                            t.duration_since(std::time::UNIX_EPOCH)
                                                .unwrap_or_default(),
                                            false,
                                        ),
                                    };
                                    crate::perf::VBlank { time, monotonic, sequence: m.sequence }
                                });
                                state.frame_timer.get_mut().flipped(received, vblank);
                                tracing::debug!("DRM VBlank received; frame submitted");
                            }
                            Some(false) => tracing::debug!("DRM VBlank received on {crtc:?}"),
                            // An output shut down while its flip was in flight, or a failed
                            // frame_submitted (logged in flip_done).
                            None => {}
                        }
                    }
                    DrmEvent::Error(e) => {
                        tracing::error!("DRM event error: {e:?}");
                        // Do not leave the compositor permanently stuck behind
                        // a stale flip_pending flag after a failed DRM event.
                        for h in &mut backend.borrow_mut().heads {
                            h.flip_pending = false;
                        }
                    }
                }
            })
            .ctx()?;
    }

    // VT/session ownership changes are expected on real hardware. The old
    // implementation paused the raw DrmDevice separately from the compositor;
    // the manager keeps the device and its output surfaces synchronized.
    {
        let backend = backend.clone();
        let mut libinput = libinput.clone();

        event_loop
            .handle()
            .insert_source(session_notifier, move |event, _, state: &mut Kompact| {
                match event {
                    SessionEvent::PauseSession => {
                        tracing::info!("seat paused; releasing DRM and input");
                        libinput.suspend();

                        let mut b = backend.borrow_mut();
                        b.vt_gate.answered();
                        b.manager.pause();
                        for h in &mut b.heads {
                            h.flip_pending = false;
                        }
                        b.active = false;
                        state.frame_timer.get_mut().reset();
                    }
                    SessionEvent::ActivateSession => {
                        tracing::info!("seat activated; reacquiring DRM and input");

                        if let Err(e) = libinput.resume() {
                            tracing::warn!("failed to resume libinput: {e:?}");
                        }

                        let mut b = backend.borrow_mut();
                        b.vt_gate.answered();
                        if let Err(e) = b.manager.activate(true) {
                            tracing::error!("failed to reactivate DRM device: {e:?}");
                            b.active = false;
                        } else {
                            for h in &mut b.heads {
                                h.flip_pending = false;
                            }
                            b.active = true;
                            tracing::info!("DRM device reactivated");
                            // Phase 5: monitors may have changed while on another VT.
                            if b.outputs_mode.watches_hotplug() {
                                b.rescan(state, "VT return");
                            }
                        }
                    }
                }
            })
            .ctx()?;
    }

    // Phase 5: hotplug. udev reports a "change" on the DRM device when a monitor is plugged in
    // or out; the connectors are scanned then. Not registered at all in `first` mode.
    if outputs_mode.watches_hotplug() {
        let udev = UdevBackend::new(session.seat())
            .map_err(|e| anyhow::anyhow!("failed to watch udev for hotplug: {e}"))?;
        let our_device = backend.borrow().manager.device().device_id();
        let backend = backend.clone();
        event_loop
            .handle()
            .insert_source(udev, move |event, _, state: &mut Kompact| match event {
                UdevEvent::Changed { device_id } if device_id == our_device => {
                    tracing::info!("udev: display hotplug event");
                    let mut b = backend.borrow_mut();
                    b.failed.clear();
                    b.rescan(state, "hotplug");
                }
                UdevEvent::Changed { .. } => {}
                UdevEvent::Added { path, .. } => tracing::info!("udev: GPU {path:?} added (not used)"),
                UdevEvent::Removed { device_id } if device_id == our_device => {
                    tracing::warn!("udev: the GPU kompact runs on was removed")
                }
                UdevEvent::Removed { .. } => tracing::info!("udev: a GPU kompact doesn't use was removed"),
            })
            .ctx()?;
        tracing::info!("watching udev for display hotplug");
    }

    let socket = smithay::wayland::socket::ListeningSocketSource::new_auto()?;
    let socket_name = socket.socket_name().to_os_string();
    event_loop
        .handle()
        .insert_source(socket, |client_stream, _, state: &mut Kompact| {
            if let Err(e) = state
                .display_handle
                .insert_client(client_stream, std::sync::Arc::new(ClientState::default()))
            {
                tracing::warn!("failed to insert Wayland client: {e}");
            }
        })
        .ctx()?;

    std::env::set_var("WAYLAND_DISPLAY", &socket_name);
    tracing::info!("listening on WAYLAND_DISPLAY={socket_name:?}");

    let startup_cmd = state.spawn_cmd.clone();
    event_loop
        .handle()
        .insert_source(Timer::immediate(), move |_, _, state: &mut Kompact| {
            state.spawn(&startup_cmd);
            TimeoutAction::Drop
        })
        .ctx()?;

    event_loop
        .run(Some(Duration::from_millis(16)), &mut state, move |state| {
            if let Some(vt) = state.pending_vt.take() {
                tracing::info!("switching to VT {vt}");
                match session.change_vt(vt) {
                    // vt.rs: seatd takes the screen now, but tells us a loop turn or more later.
                    Ok(()) => backend.borrow_mut().vt_gate.requested(Instant::now()),
                    Err(e) => tracing::warn!("VT switch failed: {e:?}"),
                }
            }

            if let Err(e) = display.dispatch_clients(state) {
                tracing::warn!("dispatch_clients failed: {e}");
            }

            // 12i-3: idle screens off, on again on input.
            backend.borrow_mut().apply_screen_power(state.screens_off);
            let rendered = backend.borrow_mut().render(state);
            match rendered {
                Ok(failed) if failed.is_empty() => {}
                // Phase 5 (`reconnect`/`all`): shut just those outputs down.
                Ok(failed) => {
                    let mut b = backend.borrow_mut();
                    b.drop_failed(&failed, state);
                    // 5f: if that was every output (say the only monitor), quit as before Phase 5,
                    // back to the console, instead of leaving a black screen.
                    if b.heads.is_empty() {
                        tracing::error!("fatal DRM rendering error: every output failed");
                        state.loop_signal.stop();
                    }
                }
                Err(e) => {
                    // A real DRM failure should terminate instead of continuing to
                    // submit bad KMS state every 16 ms and making the machine appear
                    // frozen. The error will be visible on the TTY/log.
                    tracing::error!("fatal DRM rendering error: {e:#}");
                    state.loop_signal.stop();
                }
            }

            // 6m-d3 (render path, off screen only): a requested window thumbnail, after this
            // turn's frames are queued. Nothing happens unless the shell asked and
            // KOMPACT_THUMBNAILS=on.
            if !state.thumbs.is_idle() || state.shots.is_pending() {
                backend.borrow_mut().capture_thumbnail(state);
            }

            for output in state.outputs.clone() {
                state.send_frames(&output);
            }
            state.space.refresh();
            state.popups.cleanup();

            if let Err(e) = display.flush_clients() {
                tracing::warn!("flush_clients failed: {e}");
            }
        })
        .ctx()?;

    Ok(())
}
