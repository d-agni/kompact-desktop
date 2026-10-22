use std::time::Instant;

use smithay::utils::IsAlive;

use smithay::reexports::wayland_protocols::xdg::decoration::zv1::server::zxdg_toplevel_decoration_v1::Mode as DecorationMode;

use smithay::{
    backend::allocator::dmabuf::Dmabuf,
    delegate_compositor, delegate_cursor_shape, delegate_data_control, delegate_data_device, delegate_dmabuf, delegate_layer_shell,
    delegate_output, delegate_primary_selection, delegate_seat, delegate_session_lock, delegate_shm, delegate_xdg_activation, delegate_xdg_decoration, delegate_xdg_shell,
    desktop::{layer_map_for_output, LayerSurface as DesktopLayerSurface, PopupManager, Space, Window},
    input::{pointer::CursorImageStatus, Seat, SeatHandler, SeatState},
    output::Output,
    reexports::{
        calloop::{generic::Generic, EventLoop, Interest, LoopSignal, Mode, PostAction},
        wayland_server::{
            backend::{ClientData, ClientId, DisconnectReason},
            protocol::{
                wl_buffer::WlBuffer, wl_output::WlOutput, wl_seat::WlSeat, wl_surface::WlSurface,
            },
            Display, DisplayHandle,
        },
    },
    wayland::{
        buffer::BufferHandler,
        compositor::{CompositorClientState, CompositorHandler, CompositorState},
        cursor_shape::CursorShapeManagerState,
        xdg_activation::{XdgActivationHandler, XdgActivationState, XdgActivationToken, XdgActivationTokenData},
        session_lock::{LockSurface, SessionLockHandler, SessionLockManagerState, SessionLocker},
        dmabuf::{DmabufFeedbackBuilder, DmabufGlobal, DmabufHandler, DmabufState, ImportNotifier},
        output::{OutputHandler, OutputManagerState},
        selection::{
            data_device::{
                ClientDndGrabHandler, DataDeviceHandler, DataDeviceState, ServerDndGrabHandler,
            },
            primary_selection::{PrimarySelectionHandler, PrimarySelectionState},
            wlr_data_control::{DataControlHandler, DataControlState},
            SelectionHandler,
        },
        shell::{
            wlr_layer::{
                Layer as WlrLayer, LayerSurface, WlrLayerShellHandler, WlrLayerShellState,
            },
            xdg::{
                decoration::{XdgDecorationHandler, XdgDecorationState},
                PopupSurface, PositionerState, ToplevelSurface, XdgShellHandler, XdgShellState,
            },
        },
        shm::{ShmHandler, ShmState},
        tablet_manager::TabletSeatHandler,
    },
};

/// What `render.rs` should draw as the cursor (see `Kompact::cursor_draw`).
pub enum CursorDraw {
    Hidden,
    Builtin(crate::cursor::Shape),
    Surface(WlSurface),
}

/// Everything the compositor needs to track. One instance lives for the whole process and is
/// threaded through every protocol callback via calloop.
pub struct Kompact {
    pub start_time: Instant,
    pub display_handle: DisplayHandle,
    pub loop_signal: LoopSignal,

    pub space: Space<Window>,
    pub popups: PopupManager,
    pub outputs: Vec<Output>,
    /// 5i: the outputs (name, geometry) as of the last output change, to tell which went away.
    pub last_layout: Vec<(String, smithay::utils::Rectangle<i32, smithay::utils::Logical>)>,
    /// 12h-1: where the monitors go (Settings → Displays).
    pub arrangement: crate::outputs::Arrangement,
    /// 12h-2: where the next dock surface goes (the last `dock_to`); `None`: the main display.
    pub dock_target: Option<String>,
    /// 5i: windows moved off an unplugged monitor, to put back when it returns.
    pub displaced: Vec<(Window, crate::outputs::Displaced)>,
    /// 5j-3: the dock follows the pointer to another monitor (`KOMPACT_DOCK_FOLLOW=off`: no).
    pub dock_follow: bool,
    /// 10d-1: `KOMPACT_DOCK_FOLLOW` was set, so it wins over the Dock settings.
    pub dock_follow_from_env: bool,
    /// 11a: minimize/restore animations (`anim.rs`; `KOMPACT_ANIMATIONS=off`: none).
    pub animations: bool,
    /// 11c: the settings file (`prefs.rs`); `animations` and `focus_follows_mouse` apply it.
    pub prefs: crate::prefs::Prefs,
    /// 11c: `KOMPACT_ANIMATIONS` was set, so it wins over the setting this session.
    pub animations_from_env: bool,
    /// 10d-3: when Auto appearance was last checked against the clock.
    pub appearance_checked: std::time::Instant,
    pub anims: crate::anim::Animations<smithay::desktop::Window>,
    /// 11a: dock icon size and each window's icon centre in the dock surface (`dock-icons`).
    pub dock_icons: (i32, std::collections::HashMap<u64, (i32, i32)>),
    pub dock_push: crate::dock_edge::EdgePush,
    /// 16d: the dock's bar at rest, in the dock surface's coordinates (`dock-bar`).
    pub dock_bar: Option<smithay::utils::Rectangle<i32, smithay::utils::Logical>>,
    /// 10d-1c: which edge the dock is on (the shell says).
    pub dock_side: crate::dock_edge::Side,
    /// 12f: the mice, touchpads and keyboards libinput has (for the pointer settings), and a
    /// layout switch waiting for the key that asked for it to be processed.
    pub input_devices: Vec<smithay::reexports::input::Device>,
    pub layout_cycle_pending: bool,

    // Wayland protocol state objects
    pub compositor_state: CompositorState,
    pub xdg_shell_state: XdgShellState,
    pub layer_shell_state: WlrLayerShellState,
    pub shm_state: ShmState,
    #[allow(dead_code)] // held only to keep the wl_output/xdg-output globals alive
    pub output_manager_state: OutputManagerState,
    pub seat_state: SeatState<Self>,
    pub data_device_state: DataDeviceState,
    /// 7b: middle-click paste (zwp_primary_selection_v1).
    pub primary_selection_state: PrimarySelectionState,
    /// 7c: wlr-data-control, for clipboard tools and managers (`wl-copy`, `wl-paste`, history).
    pub data_control_state: DataControlState,
    /// 12i-1: ext-idle-notify (swayidle and the like) and zwp_idle_inhibit (keep awake).
    pub idle_notifier_state: smithay::wayland::idle_notify::IdleNotifierState<Self>,
    /// Kept for the global's id; never read.
    pub _idle_inhibit_state: smithay::wayland::idle_inhibit::IdleInhibitManagerState,
    /// 12i-1: surfaces that asked the screen to stay awake.
    pub idle_inhibitors: Vec<WlSurface>,
    /// 12i-1b: apps keeping the screen awake through the shell's org.freedesktop.ScreenSaver
    /// (`keep-awake`); cleared when no shell is connected.
    pub bus_awake: Vec<String>,
    /// 12i-1: when there was input last (`idle.rs`), and the apps keeping the screen awake as
    /// last told to the shell.
    pub idle: crate::idle::Idle,
    pub awake_apps: Vec<String>,
    /// 12i-3: the screens should be off (idle); the DRM backend turns them off and on.
    pub screens_off: bool,

    pub seat: Seat<Self>,
    /// What runs at startup (empty: nothing, `--no-startup-app`).
    pub spawn_cmd: String,
    /// 14d: Open a terminal's command when Settings names none: the startup command, or the
    /// terminal found on `$PATH` when there's no startup app.
    pub terminal_cmd: String,

    /// Software cursor images, drawn at the pointer position (see `render.rs`).
    pub cursors: crate::cursor::Cursors,
    /// What the client under the pointer last asked the cursor to look like.
    pub cursor_status: CursorImageStatus,
    #[allow(dead_code)] // held only to keep the cursor-shape-v1 global alive
    pub cursor_shape_state: CursorShapeManagerState,
    /// 8a: apps bring each other forward (`activation.rs`).
    pub activation_state: XdgActivationState,
    pub activation_enabled: bool,
    /// 8d: the lock screen (`lock.rs`).
    pub session_lock_state: SessionLockManagerState,
    pub lock: crate::lock::Lock,
    #[allow(dead_code)] // held only to keep the xdg-decoration global alive (6m-e1)
    pub decoration_state: XdgDecorationState,
    pub decoration_askers: crate::decoration::Askers,
    /// 6p-f: linux-dmabuf. The global exists unless `KOMPACT_DMABUF=off` (DRM backend, see
    /// `offer_dmabuf`); `dmabuf_import` tries a new client buffer in the renderer.
    pub dmabuf_state: DmabufState,
    #[allow(dead_code)] // held only to keep the global alive
    pub dmabuf_global: Option<DmabufGlobal>,
    pub dmabuf_import: Option<Box<dyn FnMut(&Dmabuf) -> bool>>,
    pub dmabuf_refused: u64,
    /// Active Super+drag (or client-requested) move/resize, if any.
    pub drag: Option<crate::shell::Drag>,
    /// 6m-e3: pointer state for kompact's title bars.
    pub bar_input: crate::shell::BarInput,
    /// VT the user asked to switch to (Ctrl+Alt+Fn); the DRM backend acts on it.
    pub pending_vt: Option<i32>,

    /// Keys and mouse buttons currently held down. Some VMs/USB setups expose more than one
    /// virtual input device for what is physically one keyboard/mouse; libinput can then report
    /// the same physical key or button from more than one of them. These sets turn a duplicate
    /// press into a no-op instead of a repeated keystroke or a phantom second click, and stop a
    /// duplicate release from ending a drag/keypress that a *different* device still reports as
    /// held.
    pub pressed_keys: std::collections::HashSet<smithay::input::keyboard::Keycode>,
    pub pressed_buttons: std::collections::HashSet<u32>,

    /// Remembered floating geometry per app-id (Phase 2c), and the file it's persisted to.
    pub geometry_memory: crate::geometry_memory::GeometryMemory,
    pub geometry_memory_path: Option<std::path::PathBuf>,

    /// Focus-follows-mouse (Phase 2d). Off by default; `--focus-follows-mouse` or Super+M.
    pub focus_follows_mouse: bool,

    /// Workspaces (Phase 2e): windows parked on the inactive ones.
    pub workspaces: crate::workspace::Workspaces<Window>,

    /// Debug overlay (Phase 3c), `Super+D`.
    pub overlay: crate::overlay::Overlay,

    /// Page-flip timing (Phase 4a), fed by the DRM backend; summarized in each `stats:` line.
    pub frame_timer: std::cell::RefCell<crate::perf::FrameTimer>,
    /// 6p-d: damaged pixels per client since the last `stats:` line.
    pub uploads: std::cell::RefCell<crate::uploads::Meter>,

    /// 6m-c: the channel to kompact-shell (`ipc.rs`).
    pub ipc: crate::ipc::Ipc,
    /// 6d: starts kompact-shell and restarts it after a crash (`KOMPACT_SHELL`).
    pub shell: crate::shell_supervisor::Supervisor,
    /// 6m-d3: windows the shell wants thumbnails of (captured by the backend loop).
    pub thumbs: crate::thumbnail::Thumbs,
    /// 9a: queued screenshots (`screenshot.rs`), taken off screen after the frames.
    pub shots: crate::screenshot::Shots,
    /// 10a: the keyboard shortcuts (`keymap.rs`): a preset plus the user's changes.
    pub keymap: crate::keymap::Keymap,
    /// 10a: a lone tap of Super (bound in the Windows preset).
    pub key_tap: crate::keymap::Tap,
    /// 10b: the settings app is recording a combination.
    pub key_recorder: crate::keymap::Recorder,
    /// 16a: the window switcher, while Alt+Tab (or its dock button) has it open.
    pub switcher: Option<crate::switcher::Session<smithay::desktop::Window>>,
    /// 10a, Show desktop: the windows it minimized, and on which workspace, to bring back.
    pub desktop_shown: Option<(usize, Vec<smithay::desktop::Window>)>,
}

#[derive(Default)]
pub struct ClientState {
    pub compositor_state: CompositorClientState,
}
impl ClientData for ClientState {
    fn disconnected(&self, _client_id: ClientId, _reason: DisconnectReason) {}
}

impl Kompact {
    pub fn new(
        event_loop: &EventLoop<'static, Self>,
        display: &mut Display<Self>,
        spawn_cmd: String,
    ) -> Self {
        let dh = display.handle();
        let loop_handle = event_loop.handle();

        // Pump the Wayland display's client connections through calloop.
        loop_handle
            .insert_source(
                Generic::new(
                    display.backend().poll_fd().try_clone_to_owned().unwrap(),
                    Interest::READ,
                    Mode::Level,
                ),
                |_, _, state: &mut Self| {
                    state.dispatch_clients();
                    Ok(PostAction::Continue)
                },
            )
            .expect("failed to init the wayland event source");

        let geometry_memory_path = crate::geometry_memory::default_path();
        let geometry_memory = geometry_memory_path
            .as_deref()
            .map(crate::geometry_memory::load)
            .unwrap_or_default();
        tracing::info!(
            "loaded {} remembered window geometries from {:?}",
            geometry_memory.len(),
            geometry_memory_path
        );

        // Phase 3b: periodic resource stats in the log (KOMPACT_STATS_SECS, default 60, 0 = off).
        match crate::stats::interval_from_env(std::env::var("KOMPACT_STATS_SECS").ok().as_deref()) {
            Some(every) => {
                use smithay::reexports::calloop::timer::{TimeoutAction, Timer};
                let res = loop_handle.insert_source(Timer::from_duration(every), move |_, _, state: &mut Self| {
                    state.log_stats();
                    TimeoutAction::ToDuration(every)
                });
                if let Err(e) = res {
                    tracing::warn!("could not start the stats timer: {e}");
                } else {
                    tracing::info!("stats every {}s (KOMPACT_STATS_SECS)", every.as_secs());
                }
            }
            None => tracing::info!("stats disabled (KOMPACT_STATS_SECS=0)"),
        }

        // 6m-c: the shell channel. Polled every 50 ms (the loop wakes every 16 ms anyway), so it
        // needs no event source of its own and a stuck shell can't hold the compositor up.
        {
            use smithay::reexports::calloop::timer::{TimeoutAction, Timer};
            let every = std::time::Duration::from_millis(50);
            let res = loop_handle.insert_source(Timer::from_duration(every), move |_, _, state: &mut Self| {
                state.ipc_tick();
                TimeoutAction::ToDuration(every)
            });
            if let Err(e) = res {
                tracing::warn!("could not start the shell channel timer: {e}");
            }
        }

        // 6d: the shell supervisor, twice a second (a `try_wait` when the shell runs, else nothing).
        {
            use smithay::reexports::calloop::timer::{TimeoutAction, Timer};
            let every = std::time::Duration::from_millis(500);
            let res = loop_handle.insert_source(Timer::from_duration(std::time::Duration::from_millis(200)), move |_, _, state: &mut Self| {
                state.shell.tick();
                TimeoutAction::ToDuration(every)
            });
            if let Err(e) = res {
                tracing::warn!("could not start the shell supervisor timer: {e}");
            }
        }

        // Phase 3c: refresh the debug overlay once a second while it's on.
        {
            use smithay::reexports::calloop::timer::{TimeoutAction, Timer};
            let every = std::time::Duration::from_secs(1);
            let res = loop_handle.insert_source(Timer::from_duration(every), move |_, _, state: &mut Self| {
                if state.overlay.enabled {
                    state.update_overlay();
                }
                TimeoutAction::ToDuration(every)
            });
            if let Err(e) = res {
                tracing::warn!("could not start the overlay timer: {e}");
            }
        }

        let compositor_state = CompositorState::new::<Self>(&dh);
        let xdg_shell_state = XdgShellState::new::<Self>(&dh);
        let layer_shell_state = WlrLayerShellState::new::<Self>(&dh);
        let shm_state = ShmState::new::<Self>(&dh, vec![]);
        let output_manager_state = OutputManagerState::new_with_xdg_output::<Self>(&dh);
        // cursor-shape-v1: lets clients (foot, GTK4, ...) ask for named cursors, e.g. the
        // resize arrows over their borders, which we draw ourselves.
        let cursor_shape_state = CursorShapeManagerState::new::<Self>(&dh);
        let activation_state = XdgActivationState::new::<Self>(&dh);
        let lock = crate::lock::Lock::from_env();
        let lock_on = lock.enabled;
        let session_lock_state = SessionLockManagerState::new::<Self, _>(&dh, move |_| lock_on);
        tracing::info!(
            "lock screen: {} (8d: Super+L runs {:?}; KOMPACT_LOCK=off to hide, KOMPACT_LOCK_CMD=<command>)",
            if lock.enabled { "on" } else { "off" },
            lock.cmd
        );
        let activation_enabled = crate::dmabuf::enabled_from_env(std::env::var("KOMPACT_ACTIVATION").ok().as_deref());
        tracing::info!(
            "activation: {} (8a: apps bring each other forward, only with a token from the app that had the keyboard; KOMPACT_ACTIVATION=off to refuse)",
            if activation_enabled { "on" } else { "off" }
        );
        let keymap = crate::keymap::load();
        {
            use crate::keymap::{format_combos, Action};
            let on = crate::screenshot::enabled_from_env(std::env::var("KOMPACT_SCREENSHOTS").ok().as_deref());
            tracing::info!(
                "screenshots: {} (every monitor {}, the one under the pointer {}, drag out a region {}; keys from the keymap, 10a; KOMPACT_SCREENSHOTS=off, KOMPACT_SCREENSHOT_DIR=<dir>)",
                if on { "on" } else { "off" },
                format_combos(keymap.combos(Action::ShotAll)),
                format_combos(keymap.combos(Action::ShotPointerOutput)),
                format_combos(keymap.combos(Action::ShotRegion)),
            );
        }
        // 9b: wlr-screencopy for screenshot tools, unless KOMPACT_SCREENCOPY=off.
        if crate::screencopy::enabled_from_env(std::env::var("KOMPACT_SCREENCOPY").ok().as_deref()) {
            crate::screencopy::create_global(&dh);
            tracing::info!("screencopy: wlr-screencopy offered, grim and other screenshot tools work (9b; KOMPACT_SCREENCOPY=off to hide it)");
        } else {
            tracing::info!("screencopy: not offered (KOMPACT_SCREENCOPY=off)");
        }
        // 6m-e1: xdg-decoration; every app is told to draw its own title bar (`decoration.rs`).
        let decoration_state = XdgDecorationState::new::<Self>(&dh);
        let mut seat_state = SeatState::new();
        let data_device_state = DataDeviceState::new::<Self>(&dh);
        let primary_selection_state = PrimarySelectionState::new::<Self>(&dh);
        // 7c: offered unless KOMPACT_DATA_CONTROL=off. Any client may then read and set both
        // selections without having the keyboard (as on sway/Hyprland); the filter hides the
        // global entirely when switched off.
        let data_control = crate::dmabuf::enabled_from_env(std::env::var("KOMPACT_DATA_CONTROL").ok().as_deref());
        let prefs = crate::prefs::load();
        let focus_follows_mouse = crate::focus::focus_follows_mouse_at_start() || prefs.focus_follows_mouse;
        // 10d-3: title bars light or dark, buttons left or right, from the start.
        let light = prefs.appearance.is_light(crate::prefs::local_hour());
        crate::titlebar::set_style(crate::titlebar::Style { light, right: prefs.buttons_right });
        // 10d-3b: and apps the same, from the start.
        crate::theme::sync_apps(!light, prefs.accent, prefs.buttons_right);
        let animations_env = std::env::var("KOMPACT_ANIMATIONS").ok();
        let animations_from_env = animations_env.is_some();
        let animations = if animations_from_env {
            crate::anim::enabled_from_env(animations_env.as_deref())
        } else {
            prefs.animations
        };
        tracing::info!(
            "animations: {} (11a: minimize/restore into the dock; KOMPACT_ANIMATIONS=off for none)",
            if animations { "on" } else { "off" }
        );
        let dock_follow_env = std::env::var("KOMPACT_DOCK_FOLLOW").ok();
        let dock_follow_from_env = dock_follow_env.is_some();
        let dock_follow = crate::dmabuf::enabled_from_env(dock_follow_env.as_deref());
        tracing::info!(
            "dock follows the pointer: {} (5j-3: hold it at another monitor's bottom edge; KOMPACT_DOCK_FOLLOW=off to stop)",
            if dock_follow { "on" } else { "off" }
        );
        if data_control {
            tracing::info!("clipboard: wlr-data-control offered, clipboard tools and managers work (7c; KOMPACT_DATA_CONTROL=off to hide it)");
        } else {
            tracing::info!("clipboard: wlr-data-control not offered (KOMPACT_DATA_CONTROL=off)");
        }
        let data_control_state = DataControlState::new::<Self, _>(&dh, Some(&primary_selection_state), move |_| data_control);
        let idle_notifier_state = smithay::wayland::idle_notify::IdleNotifierState::new(&dh, loop_handle.clone());
        let idle_inhibit_state = smithay::wayland::idle_inhibit::IdleInhibitManagerState::new::<Self>(&dh);

        let mut seat: Seat<Self> = seat_state.new_wl_seat(&dh, "seat0");
        // 12f: the layouts, Caps Lock and key repeat from Settings → Mouse & Keyboard (a layout
        // xkb doesn't know falls back to the default keymap).
        let (layout, variant, options) = prefs.xkb_parts();
        let xkb = smithay::input::keyboard::XkbConfig { layout: &layout, variant: &variant, options, ..Default::default() };
        let (delay, rate) = (prefs.repeat_delay as i32, prefs.repeat_rate as i32);
        if seat.add_keyboard(xkb, delay, rate).is_err() {
            tracing::warn!("keyboard: layouts {:?} didn't compile; the default keymap", prefs.layouts);
            seat.add_keyboard(Default::default(), delay, rate).expect("failed to add keyboard to seat");
        }
        tracing::info!("keyboard: layouts {}, Caps Lock {}, repeat after {delay} ms at {rate}/s", prefs.layouts, prefs.caps.name());
        seat.add_pointer();

        Self {
            start_time: Instant::now(),
            display_handle: dh,
            loop_signal: event_loop.get_signal(),
            space: Space::default(),
            popups: PopupManager::default(),
            outputs: Vec::new(),
            last_layout: Vec::new(),
            arrangement: crate::outputs::load_arrangement(),
            dock_target: None,
            displaced: Vec::new(),
            dock_follow,
            dock_follow_from_env,
            animations,
            prefs,
            animations_from_env,
            appearance_checked: std::time::Instant::now(),
            anims: Default::default(),
            dock_icons: (50, Default::default()),
            dock_push: Default::default(),
            dock_bar: None,
            dock_side: Default::default(),
            input_devices: Vec::new(),
            layout_cycle_pending: false,
            compositor_state,
            xdg_shell_state,
            layer_shell_state,
            shm_state,
            output_manager_state,
            seat_state,
            data_device_state,
            primary_selection_state,
            data_control_state,
            idle_notifier_state,
            _idle_inhibit_state: idle_inhibit_state,
            idle_inhibitors: Vec::new(),
            bus_awake: Vec::new(),
            idle: crate::idle::Idle::new(std::time::Instant::now()),
            awake_apps: Vec::new(),
            screens_off: false,
            seat,
            terminal_cmd: if spawn_cmd.trim().is_empty() { crate::default_spawn_command() } else { spawn_cmd.clone() },
            spawn_cmd,
            cursors: crate::cursor::Cursors::new(),
            cursor_status: CursorImageStatus::default_named(),
            cursor_shape_state,
            activation_state,
            activation_enabled,
            session_lock_state,
            lock,
            decoration_state,
            decoration_askers: Default::default(),
            dmabuf_state: DmabufState::new(),
            dmabuf_global: None,
            dmabuf_import: None,
            dmabuf_refused: 0,
            drag: None,
            bar_input: Default::default(),
            pending_vt: None,
            pressed_keys: std::collections::HashSet::new(),
            pressed_buttons: std::collections::HashSet::new(),
            geometry_memory,
            geometry_memory_path,
            focus_follows_mouse,
            workspaces: Default::default(),
            overlay: crate::overlay::Overlay::new(),
            frame_timer: Default::default(),
            uploads: Default::default(),
            ipc: Default::default(),
            shell: Default::default(),
            thumbs: Default::default(),
            shots: Default::default(),
            keymap,
            key_tap: Default::default(),
            key_recorder: Default::default(),
            switcher: None,
            desktop_shown: None,
        }
    }

    pub fn toggle_overlay(&mut self) {
        self.overlay.enabled = !self.overlay.enabled;
        if self.overlay.enabled {
            self.update_overlay();
        }
        tracing::info!("debug overlay: {}", if self.overlay.enabled { "on" } else { "off" });
    }

    /// Gather the overlay's numbers and redraw it (Phase 3c).
    pub fn update_overlay(&mut self) {
        let (rss_kib, fds) = crate::stats::sample_self();
        let timing = self.overlay.frames.borrow_mut().timing(Instant::now());
        let snapshot = crate::overlay::Snapshot {
            rss_kib,
            fds,
            timing,
            visible: self.space.elements().count(),
            parked: self.workspaces.parked().count(),
            workspace: self.workspaces.active(),
            surfaces_with_buffer: self.surfaces_with_buffer(),
            uptime: self.start_time.elapsed(),
        };
        self.overlay.show(crate::overlay::format_lines(&snapshot));
    }

    /// Client surfaces (window and layer-shell trees, popups included via their own trees)
    /// currently holding a buffer, i.e. roughly how many client textures the renderer keeps.
    fn surfaces_with_buffer(&self) -> usize {
        use smithay::backend::renderer::utils::RendererSurfaceStateUserData;
        use smithay::wayland::compositor::{with_surface_tree_downward, TraversalAction};
        let mut roots: Vec<WlSurface> = self
            .space
            .elements()
            .chain(self.workspaces.parked())
            .filter_map(|w| w.toplevel().map(|t| t.wl_surface().clone()))
            .collect();
        let popups: Vec<WlSurface> = roots
            .iter()
            .flat_map(|s| PopupManager::popups_for_surface(s).map(|(p, _)| p.wl_surface().clone()))
            .collect();
        roots.extend(popups);
        for o in &self.outputs {
            roots.extend(layer_map_for_output(o).layers().map(|l| l.wl_surface().clone()));
        }
        let mut count = 0;
        for root in &roots {
            with_surface_tree_downward(
                root,
                (),
                |_, _, _| TraversalAction::DoChildren(()),
                |_, states, _| {
                    let has = states
                        .data_map
                        .get::<RendererSurfaceStateUserData>()
                        .is_some_and(|d| d.lock().unwrap().buffer().is_some());
                    count += has as usize;
                },
                |_, _, _| true,
            );
        }
        count
    }

    /// One `stats:` log line (Phase 3b), see `stats.rs`.
    pub fn log_stats(&self) {
        let popups = self
            .space
            .elements()
            .filter_map(|w| w.toplevel())
            .map(|t| PopupManager::popups_for_surface(t.wl_surface()).count())
            .sum();
        let layers = self.outputs.iter().map(|o| layer_map_for_output(o).layers().count()).sum();
        let counts = crate::stats::Counts {
            visible: self.space.elements().count(),
            parked: self.workspaces.parked().count(),
            layers,
            popups,
            workspace: self.workspaces.active(),
        };
        let (rss, fds) = crate::stats::sample_self();
        let timing = self
            .overlay
            .enabled
            .then(|| self.overlay.frames.borrow_mut().timing(Instant::now()));
        let frames = self.frame_timer.borrow_mut().take();
        tracing::info!(
            "{}",
            crate::stats::format_line(counts, rss, fds, timing.as_ref(), frames.as_ref())
        );
        if let Some(line) = self.uploads.borrow_mut().take_line() {
            tracing::info!("{line}");
        }
    }

    pub fn dispatch_clients(&mut self) {
        // The actual `display.dispatch_clients` call happens in the backend's main loop, which
        // owns the `Display<Self>`. This hook exists so the calloop source above has something
        // to call; backends call `display.dispatch_clients(state)` right before this returns.
    }

    /// Spawn a process detached from the compositor (so it isn't reaped as our child on exit).
    pub fn spawn(&self, cmd: &str) {
        // 14c-2: the login screen, watched so kompact quits after it.
        if crate::greeter::is_greeter_cmd(cmd) {
            crate::greeter::spawn(cmd);
            return;
        }
        // First spawn happens after the backend created the socket and set WAYLAND_DISPLAY.
        crate::session_env::export_once();
        let cmd = cmd.to_string();
        std::thread::spawn(move || {
            let mut parts = cmd.split_whitespace();
            let Some(program) = parts.next() else { return };
            match std::process::Command::new(program).args(parts).spawn() {
                Ok(mut child) => {
                    tracing::info!("spawned {cmd}");
                    let _ = child.wait(); // reap so it doesn't linger as a zombie
                }
                Err(e) => tracing::warn!("failed to spawn {cmd}: {e}"),
            }
        });
    }

    /// 10a: run a program with these arguments (no splitting, so paths with spaces work).
    pub fn spawn_argv(&self, argv: Vec<String>) {
        crate::session_env::export_once();
        std::thread::spawn(move || {
            let Some((program, args)) = argv.split_first() else { return };
            match std::process::Command::new(program).args(args).spawn() {
                Ok(mut child) => {
                    tracing::info!("spawned {}", argv.join(" "));
                    let _ = child.wait();
                }
                Err(e) => tracing::warn!("failed to spawn {}: {e}", argv.join(" ")),
            }
        });
    }

    /// What to draw as the cursor now. A compositor-driven resize shows the matching resize
    /// arrow; otherwise the client under the pointer decides: a named shape (cursor-shape-v1,
    /// e.g. foot), its own cursor surface (wl_pointer.set_cursor, e.g. Firefox's link hand), or
    /// nothing.
    pub fn cursor_draw(&self) -> CursorDraw {
        use crate::cursor::{shape_for_edges, shape_for_icon, Shape};
        if let Some(crate::shell::Drag { kind: crate::shell::DragKind::Resize(edges), .. }) = &self.drag {
            return CursorDraw::Builtin(shape_for_edges(*edges));
        }
        if let Some(edges) = self.bar_input.hover {
            return CursorDraw::Builtin(shape_for_edges(edges));
        }
        match &self.cursor_status {
            CursorImageStatus::Hidden => CursorDraw::Hidden,
            CursorImageStatus::Named(icon) => CursorDraw::Builtin(shape_for_icon(*icon)),
            CursorImageStatus::Surface(s) if s.alive() => CursorDraw::Surface(s.clone()),
            CursorImageStatus::Surface(_) => CursorDraw::Builtin(Shape::Arrow),
        }
    }

    pub fn surface_under(
        &self,
        pos: smithay::utils::Point<f64, smithay::utils::Logical>,
    ) -> Option<(WlSurface, smithay::utils::Point<f64, smithay::utils::Logical>)> {
        // 6c: layer surfaces (panel, launcher, wallpaper) get pointer input too, in drawing order.
        self.hit(pos).map(|(_, surface, loc)| (surface, loc))
    }
}

// --- Protocol handler impls -------------------------------------------------------------

impl CompositorHandler for Kompact {
    fn compositor_state(&mut self) -> &mut CompositorState {
        &mut self.compositor_state
    }

    fn client_compositor_state<'a>(
        &self,
        client: &'a smithay::reexports::wayland_server::Client,
    ) -> &'a CompositorClientState {
        &client.get_data::<ClientState>().unwrap().compositor_state
    }

    fn commit(&mut self, surface: &WlSurface) {
        smithay::backend::renderer::utils::on_commit_buffer_handler::<Self>(surface);
        self.meter_upload(surface);
        self.popups.commit(surface);
        // A popup's first commit gets its initial configure (position from `new_popup`).
        if let Some(smithay::desktop::PopupKind::Xdg(popup)) = self.popups.find_popup(surface) {
            // Phase 4c: the popup's first buffer (perf.rs times it to the screen).
            let key = popup_key(surface);
            if self.frame_timer.get_mut().popup_waiting(key)
                && smithay::backend::renderer::utils::with_renderer_surface_state(surface, |s| {
                    s.buffer().is_some()
                })
                .unwrap_or(false)
            {
                self.frame_timer.get_mut().popup_buffer(key, Instant::now());
            }
            if !popup.is_initial_configure_sent() {
                match popup.send_configure() {
                    Ok(_) => tracing::info!(
                        "popup configured at {:?} (relative to its parent)",
                        popup.with_pending_state(|s| s.geometry)
                    ),
                    Err(e) => tracing::warn!("popup initial configure failed: {e}"),
                }
            }
        }

        // Resolved in its own statement so the space's iterator borrow ends before
        // place_if_pending takes `&mut self` (2021-edition if-let temporaries outlive the body).
        let window = self.window_for_surface(surface);
        if let Some(window) = window {
            window.on_commit();
            crate::shell::place_if_pending(self, &window);
            crate::shell::follow_resize_anchor(self, &window);
            crate::shell::animate_on_commit(self, &window);
        }

        // Layer-shell's initial configure must be sent in response to the surface's first
        // commit, not at role-assignment time (new_layer_surface) — at that point the client
        // hasn't sent its set_anchor/set_size/set_exclusive_zone requests yet, so arrange()
        // would compute geometry against empty/default state instead of what the client
        // actually asked for. This is also why map_layer() alone (in new_layer_surface) doesn't
        // auto-send a configure: it's deliberately left to us to do here, once, after the first
        // real commit.
        for output in self.outputs.clone() {
            let mut map = layer_map_for_output(&output);
            let layer = map.layers().find(|l| l.wl_surface() == surface).cloned();
            let Some(layer) = layer else { continue };

            let already_configured = smithay::wayland::compositor::with_states(surface, |states| {
                states
                    .data_map
                    .get::<smithay::wayland::shell::wlr_layer::LayerSurfaceData>()
                    .map(|d| d.lock().unwrap().initial_configure_sent)
                    .unwrap_or(true)
            });
            // Re-arrange on every layer commit, not just the first: anchors, size and exclusive
            // zone are double-buffered, and clients may set them later (gtk4-layer-shell's auto
            // exclusive zone arrives after the first configure). arrange() itself only sends a
            // configure when a surface's size actually changes. Layer surfaces commit rarely
            // (the panel about once a minute), so this isn't the per-frame walk Phase 1 removed.
            let zone_before = map.non_exclusive_zone();
            map.arrange();
            let zone_after = map.non_exclusive_zone();
            if !already_configured {
                layer.layer_surface().send_configure();
            }
            // The window code below takes the layer map lock itself.
            drop(map);
            if zone_after != zone_before {
                self.on_layer_zone_changed(&output, zone_before, zone_after);
            }
            // 6c: keyboard interactivity is double-buffered too (a launcher asks for exclusive).
            self.refresh_layer_focus(None);
            break;
        }
    }
}

impl Kompact {
    /// A panel changed how much of `output` it reserves (zones relative to the output, as
    /// `non_exclusive_zone()` gives them): move and refit the windows there (Phase 6a).
    fn on_layer_zone_changed(
        &mut self,
        output: &smithay::output::Output,
        before: smithay::utils::Rectangle<i32, smithay::utils::Logical>,
        after: smithay::utils::Rectangle<i32, smithay::utils::Logical>,
    ) {
        let (old, new) = (self.area_from_zone(output, before), self.area_from_zone(output, after));
        tracing::info!("layer exclusive zone changed on {}: usable area now {:?}", output.name(), new);
        self.usable_area_changed(output, old, new);
    }

    /// Frame callbacks after a render of `output`: windows, and layer-shell surfaces (6t-a fix:
    /// layers got none, so GTK panels drew their first frame and then waited forever).
    /// 6p-e: at most one per `frames::PACE` per surface (this runs on every loop turn, not once a
    /// refresh); with `KOMPACT_FRAME_PACING=off`, every surface counts as shown and gets one each turn.
    pub fn send_frames(&self, output: &Output) {
        let time = self.start_time.elapsed();
        let paced = crate::frames::paced();
        let throttle = Some(if paced { crate::frames::PACE } else { std::time::Duration::ZERO });
        let shown_on = |_: &_, _: &_| (!paced).then(|| output.clone());
        self.space.elements().for_each(|window| {
            window.send_frame(output, time, throttle, shown_on);
        });
        for layer in layer_map_for_output(output).layers() {
            layer.send_frame(output, time, throttle, shown_on);
        }
        // 8d: the lock client draws on frame callbacks like anyone else.
        if let Some(lock) = self.lock.surface_on(output) {
            smithay::desktop::utils::send_frames_surface_tree(lock.wl_surface(), output, time, throttle, |_, _| Some(output.clone()));
        }
    }
}
delegate_compositor!(Kompact);

impl XdgShellHandler for Kompact {
    fn xdg_shell_state(&mut self) -> &mut XdgShellState {
        &mut self.xdg_shell_state
    }

    fn new_toplevel(&mut self, surface: ToplevelSurface) {
        let window = Window::new_wayland_window(surface);
        crate::shell::map_new_window(self, window);
    }

    fn app_id_changed(&mut self, surface: ToplevelSurface) {
        if let Some(w) = self.window_for_surface(surface.wl_surface()) {
            crate::shell::request_remembered_size(self, &w);
        }
    }

    fn new_popup(&mut self, surface: PopupSurface, positioner: PositionerState) {
        self.frame_timer.get_mut().popup_created(popup_key(surface.wl_surface()), Instant::now());
        // Work out where it goes now; the configure itself is sent on the popup's first commit
        // (see `commit`), which is when xdg-shell expects it. Without that configure the client
        // may never attach a buffer, so the menu never appears.
        self.position_popup(&surface, positioner);
        if let Err(e) = self.popups.track_popup(smithay::desktop::PopupKind::Xdg(surface)) {
            tracing::warn!("failed to track popup: {e}");
        }
    }

    /// Implements the real xdg_popup grab, per the protocol: while a popup has an active grab,
    /// pointer/keyboard input routes to it exclusively, and an outside click or keyboard press
    /// dismisses the whole popup chain. Previously a no-op — some clients (seen with Firefox)
    /// appear to wait on the grab actually being established before finishing a popup's open
    /// sequence, which plausibly explains dialogs feeling slow to open rather than slow to
    /// render: they weren't waiting on the GPU, they were waiting on a grab we never granted.
    fn grab(&mut self, surface: PopupSurface, _seat: WlSeat, serial: smithay::utils::Serial) {
        // 8d: nothing grabs the keyboard from the lock screen.
        if self.lock.is_locked() {
            return;
        }
        let popup = smithay::desktop::PopupKind::Xdg(surface);
        let Ok(root) = smithay::desktop::find_popup_root_surface(&popup) else {
            return;
        };

        // Single-seat compositor: no need to resolve the wl_seat resource back to our Seat<Self>,
        // we already have the one and only seat directly.
        let seat = self.seat.clone();

        match self.popups.grab_popup(root, popup, &seat, serial) {
            Ok(grab) => {
                // set_grab() overwrites whatever grab (if any) was already active, so no manual
                // unset is needed first.
                if let Some(keyboard) = seat.get_keyboard() {
                    keyboard.set_focus(self, grab.current_grab(), serial);
                    keyboard.set_grab(self, smithay::desktop::PopupKeyboardGrab::new(&grab), serial);
                }
                if let Some(pointer) = seat.get_pointer() {
                    pointer.set_grab(
                        self,
                        smithay::desktop::PopupPointerGrab::new(&grab),
                        serial,
                        smithay::input::pointer::Focus::Keep,
                    );
                }
            }
            Err(err) => {
                tracing::debug!("popup grab denied: {err}");
            }
        }
    }

    fn toplevel_destroyed(&mut self, surface: ToplevelSurface) {
        crate::shell::unmap_window(self, surface.wl_surface());
    }

    fn move_request(&mut self, surface: ToplevelSurface, _seat: WlSeat, _serial: smithay::utils::Serial) {
        // Only honour it while a button is actually held, otherwise the drag would never end.
        let held = self.seat.get_pointer().map(|p| p.is_grabbed()).unwrap_or(false);
        if held {
            if let Some(w) = self.window_for_surface(surface.wl_surface()) {
                self.begin_drag(w, crate::shell::DragKind::Move, true);
            }
        }
    }

    fn resize_request(
        &mut self,
        surface: ToplevelSurface,
        _seat: WlSeat,
        _serial: smithay::utils::Serial,
        edges: smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel::ResizeEdge,
    ) {
        // Same rule as move_request: only while a button is actually held.
        let held = self.seat.get_pointer().map(|p| p.is_grabbed()).unwrap_or(false);
        let Some(edges) = crate::resize::Edges::from_xdg(edges) else { return };
        if held {
            if let Some(w) = self.window_for_surface(surface.wl_surface()) {
                self.begin_drag(w, crate::shell::DragKind::Resize(edges), true);
            }
        }
    }

    fn maximize_request(&mut self, surface: ToplevelSurface) {
        if let Some(w) = self.window_for_surface(surface.wl_surface()) {
            self.animated_fill(&w, true);
        }
    }

    fn minimize_request(&mut self, surface: ToplevelSurface) {
        if let Some(w) = self.window_for_surface(surface.wl_surface()) {
            self.minimize_window(&w);
        }
    }

    fn unmaximize_request(&mut self, surface: ToplevelSurface) {
        if let Some(w) = self.window_for_surface(surface.wl_surface()) {
            self.animated_fill(&w, false);
        }
    }

    fn fullscreen_request(&mut self, surface: ToplevelSurface, output: Option<WlOutput>) {
        if let Some(w) = self.window_for_surface(surface.wl_surface()) {
            // Phase 5: on the output the client asked for, else the one the window is on.
            let requested = output
                .as_ref()
                .and_then(Output::from_resource)
                .filter(|o| self.outputs.contains(o));
            match requested {
                Some(o) => self.set_fill_on(&w, true, true, Some(&o)),
                None => self.set_fill(&w, true, true),
            }
        }
    }

    fn unfullscreen_request(&mut self, surface: ToplevelSurface) {
        if let Some(w) = self.window_for_surface(surface.wl_surface()) {
            self.set_fill(&w, false, true);
        }
    }

    fn reposition_request(&mut self, surface: PopupSurface, positioner: PositionerState, token: u32) {
        self.position_popup(&surface, positioner);
        surface.send_repositioned(token);
        tracing::debug!("popup repositioned to {:?}", surface.with_pending_state(|s| s.geometry));
    }
}
delegate_xdg_shell!(Kompact);

/// wlr-layer-shell support: panels, backgrounds, menus — surfaces anchored to screen edges or
/// filling the output, outside the normal window stack. Placement/sizing per anchor+exclusive-
/// zone is handled entirely by `LayerMap::map_layer`/`arrange()` internally; we just need to map
/// new surfaces into the right output's map and send the resulting configure.
impl WlrLayerShellHandler for Kompact {
    fn shell_state(&mut self) -> &mut WlrLayerShellState {
        &mut self.layer_shell_state
    }

    /// A popup was just given a layer surface as its parent (`zwlr_layer_surface_v1.get_popup`).
    /// GTK creates layer-shell popups without a parent and attaches them right after, so when
    /// `new_popup` positioned it the parent was unknown and nothing kept it on screen: a dock's
    /// right-click menu opened below the bottom edge. Place it again now that the parent is
    /// known; its first configure still goes out on its first commit.
    fn new_popup(&mut self, _parent: LayerSurface, popup: PopupSurface) {
        let positioner = popup.with_pending_state(|s| s.positioner);
        self.position_popup(&popup, positioner);
        let geometry = popup.with_pending_state(|s| s.geometry);
        tracing::info!("layer popup placed at {:?} (relative to its layer surface)", geometry);
    }

    fn new_layer_surface(
        &mut self,
        surface: LayerSurface,
        wl_output: Option<WlOutput>,
        _layer: WlrLayer,
        namespace: String,
    ) {
        // 12h-2: one that asks for no output: menu bars and the dock by `output_for_layer`.
        let Some(output) = wl_output.as_ref().and_then(Output::from_resource).or_else(|| self.output_for_layer(&namespace))
        else {
            tracing::warn!("new layer surface ({namespace}) but no output to put it on");
            return;
        };

        // 5j-3: where the dock (re)appears is what moving it is about.
        if namespace == "kompact-dock" || namespace == "kompact-panel" {
            tracing::info!("layer surface {namespace} on {}{}", output.name(), if wl_output.is_some() { " (asked for)" } else { "" });
        }
        let layer = DesktopLayerSurface::new(surface, namespace);
        // The MutexGuard from layer_map_for_output() must be dropped before `output` itself
        // goes out of scope at the end of this function; resolving it to an owned `Result` in
        // its own block first (rather than chaining straight into the match scrutinee) forces
        // that drop to happen immediately, not deferred to the end of the match/function.
        let map_result = {
            let mut map = layer_map_for_output(&output);
            map.map_layer(&layer)
        };
        // Deliberately not sending a configure here: at role-assignment time the client hasn't
        // sent its set_anchor/set_size/set_exclusive_zone requests yet (those, plus the
        // triggering commit, arrive as separate protocol messages after this one), so arrange()
        // would size this against empty/default state. The real initial configure is sent from
        // CompositorHandler::commit(), once the surface's first real commit has actually landed.
        if let Err(e) = map_result {
            tracing::warn!("failed to map new layer surface: {e}");
        }
    }

    fn layer_destroyed(&mut self, surface: LayerSurface) {
        for output in self.outputs.clone() {
            let mut map = layer_map_for_output(&output);
            // Resolved and dropped in its own statement, before `map` is borrowed mutably below
            // — doing this inline in an `if let` keeps the iterator's borrow alive for the whole
            // `if let` body under 2021-edition temporary scoping rules, conflicting with
            // `map.unmap_layer(&layer)`'s `&mut self`.
            let found = map.layers().find(|l| l.layer_surface() == &surface).cloned();
            if let Some(layer) = found {
                // unmap_layer re-arranges, which gives the panel's space back.
                let before = map.non_exclusive_zone();
                map.unmap_layer(&layer);
                let after = map.non_exclusive_zone();
                drop(map);
                if after != before {
                    self.on_layer_zone_changed(&output, before, after);
                }
                self.refresh_layer_focus(Some(layer.wl_surface()));
                break;
            }
        }
    }
}
delegate_layer_shell!(Kompact);

impl BufferHandler for Kompact {
    fn buffer_destroyed(&mut self, _buffer: &WlBuffer) {}
}

delegate_shm!(Kompact);

impl Kompact {
    /// 6p-f: create the linux-dmabuf global (v4, default feedback: `main_device` and the formats
    /// the renderer can sample). `import` tries each new client buffer in the renderer.
    pub fn offer_dmabuf(
        &mut self,
        dh: &DisplayHandle,
        main_device: libc::dev_t,
        formats: smithay::backend::allocator::format::FormatSet,
        import: Box<dyn FnMut(&Dmabuf) -> bool>,
    ) {
        let count = formats.iter().count();
        match DmabufFeedbackBuilder::new(main_device, formats).build() {
            Ok(feedback) => {
                let global = self.dmabuf_state.create_global_with_default_feedback::<Self>(dh, &feedback);
                self.dmabuf_global = Some(global);
                self.dmabuf_import = Some(import);
                tracing::info!("dmabuf: linux-dmabuf v4 offered, {count} formats, main device {main_device} (6p-f; KOMPACT_DMABUF=off for shared memory only)");
            }
            Err(e) => tracing::warn!("dmabuf: not offered, feedback failed: {e}"),
        }
    }
}

impl DmabufHandler for Kompact {
    fn dmabuf_state(&mut self) -> &mut DmabufState {
        &mut self.dmabuf_state
    }

    fn dmabuf_imported(&mut self, _global: &DmabufGlobal, dmabuf: Dmabuf, notifier: ImportNotifier) {
        use smithay::backend::allocator::Buffer;
        let ok = self.dmabuf_import.as_mut().map_or(true, |import| import(&dmabuf));
        if ok {
            let _ = notifier.successful::<Self>();
        } else {
            self.dmabuf_refused += 1;
            if crate::dmabuf::log_failure(self.dmabuf_refused) {
                tracing::warn!(
                    "dmabuf: refused a {:?} {:?} buffer the renderer can't import ({} so far)",
                    dmabuf.format().code,
                    dmabuf.size(),
                    self.dmabuf_refused
                );
            }
            notifier.failed();
        }
    }
}
delegate_dmabuf!(Kompact);
impl ShmHandler for Kompact {
    fn shm_state(&self) -> &ShmState {
        &self.shm_state
    }
}

impl OutputHandler for Kompact {}

delegate_output!(Kompact);

impl SeatHandler for Kompact {
    type KeyboardFocus = WlSurface;
    type PointerFocus = WlSurface;
    type TouchFocus = WlSurface;

    fn seat_state(&mut self) -> &mut SeatState<Self> {
        &mut self.seat_state
    }

    fn cursor_image(&mut self, _seat: &Seat<Self>, image: CursorImageStatus) {
        tracing::debug!(
            "client cursor: {}",
            match &image {
                CursorImageStatus::Hidden => "hidden".to_string(),
                CursorImageStatus::Named(icon) => format!("named {icon:?}"),
                CursorImageStatus::Surface(_) => "surface".to_string(),
            }
        );
        self.cursor_status = image;
    }

    /// 7a: the clipboard follows keyboard focus. Smithay offers the current selection only to
    /// the client set here (0.4.4 `selection/seat_data.rs`), and nothing else sets it, so until
    /// now no app was ever offered another app's copy. 7b: the primary selection likewise.
    fn focus_changed(&mut self, seat: &Seat<Self>, focused: Option<&WlSurface>) {
        let client = focused.and_then(|s| smithay::reexports::wayland_server::Resource::client(s));
        smithay::wayland::selection::data_device::set_data_device_focus(&self.display_handle, seat, client.clone());
        smithay::wayland::selection::primary_selection::set_primary_focus(&self.display_handle, seat, client);
    }
}
delegate_seat!(Kompact);

impl SessionLockHandler for Kompact {
    fn lock_state(&mut self) -> &mut SessionLockManagerState {
        &mut self.session_lock_state
    }

    fn lock(&mut self, confirmation: SessionLocker) {
        let was = self.lock.phase;
        if !self.lock.enabled || !crate::lock::accept_new_lock(was, self.lock.holder_alive()) {
            // Dropping the locker tells the client it didn't get the lock.
            tracing::info!("lock screen: lock request refused ({})", if self.lock.enabled { "already locked by a running lock client" } else { "KOMPACT_LOCK=off" });
            return;
        }
        self.lock.holder = Some(confirmation.ext_session_lock().clone());
        self.lock.surfaces.retain(|(s, _)| s.alive());
        if was == crate::lock::Phase::Unlocked {
            self.lock.phase = crate::lock::Phase::Locking { since: Instant::now() };
            self.lock.pending = Some(confirmation);
            self.lock_input();
            tracing::info!("lock screen: locking (screen cleared, confirmed in {} ms)", crate::lock::CONFIRM_AFTER.as_millis());
        } else {
            // Already black: a new lock client takes over from one that went away.
            confirmation.lock();
            tracing::info!("lock screen: a new lock client took over the lock");
        }
    }

    fn unlock(&mut self) {
        self.lock.phase = crate::lock::Phase::Unlocked;
        self.lock.pending = None;
        self.lock.holder = None;
        self.lock.surfaces.clear();
        tracing::info!("lock screen: unlocked");
        self.ipc.broadcast(&crate::ipc::encode_lock(false));
        let top = self.space.elements().last().cloned();
        if let Some(keyboard) = self.seat.get_keyboard() {
            keyboard.set_focus(self, None, smithay::utils::SERIAL_COUNTER.next_serial());
        }
        self.set_focus(top.as_ref());
        self.refresh_layer_focus(None);
        self.refresh_pointer_focus();
    }

    fn new_surface(&mut self, surface: LockSurface, wl_output: WlOutput) {
        let Some(output) = Output::from_resource(&wl_output) else {
            tracing::warn!("lock screen: lock surface for an unknown output");
            return;
        };
        let size = self.output_geometry_of(Some(&output)).size;
        // Smithay sends the first configure right after this returns.
        surface.with_pending_state(|s| s.size = Some((size.w as u32, size.h as u32).into()));
        tracing::info!("lock screen: lock surface on {} ({}x{})", output.name(), size.w, size.h);
        self.lock.surfaces.retain(|(s, o)| s.alive() && o != &output);
        self.lock.surfaces.push((surface, output));
        self.focus_lock_surface();
    }
}
delegate_session_lock!(Kompact);

impl XdgActivationHandler for Kompact {
    fn activation_state(&mut self) -> &mut XdgActivationState {
        &mut self.activation_state
    }

    fn token_created(&mut self, _token: XdgActivationToken, data: XdgActivationTokenData) -> bool {
        // Old tokens nobody used are dropped here, as good a time as any.
        self.activation_state.retain_tokens(|_, d| d.timestamp.elapsed() < crate::activation::MAX_AGE);
        let focused_client = self
            .seat
            .get_keyboard()
            .and_then(|k| k.current_focus())
            .and_then(|s| smithay::reexports::wayland_server::Resource::client(&s))
            .map(|c| c.id());
        let has_keyboard = data.client_id.is_some() && data.client_id == focused_client;
        let ok = crate::activation::accept_token(self.activation_enabled, has_keyboard);
        tracing::info!(
            "activation: token for {:?} {}",
            data.app_id.as_deref().unwrap_or("?"),
            if ok { "given" } else if self.activation_enabled { "refused (the app asking hasn't the keyboard)" } else { "refused (KOMPACT_ACTIVATION=off)" }
        );
        ok
    }

    fn request_activation(&mut self, token: XdgActivationToken, data: XdgActivationTokenData, surface: WlSurface) {
        // A token is good for one activation.
        self.activation_state.remove_token(&token);
        let Some(window) = self.window_for_surface(&surface) else {
            tracing::info!("activation: request for a surface that isn't a window, ignored");
            return;
        };
        if self.lock.is_locked() {
            tracing::info!("activation: {:?} not brought forward (screen locked)", crate::shell::app_id(&window));
            return;
        }
        let age = data.timestamp.elapsed();
        match crate::activation::decide(self.activation_enabled, age) {
            crate::activation::Verdict::Activate => {
                tracing::info!("activation: bringing {:?} forward (token {} ms old)", crate::shell::app_id(&window), age.as_millis());
                self.activate_window(&window);
            }
            verdict => tracing::info!("activation: {:?} not brought forward ({verdict:?}, token {} ms old)", crate::shell::app_id(&window), age.as_millis()),
        }
    }
}
delegate_xdg_activation!(Kompact);

// Required by cursor-shape-v1 (it can also set tablet tool cursors); no tablets here.
impl TabletSeatHandler for Kompact {}
delegate_cursor_shape!(Kompact);

impl Kompact {
    /// 6m-e1: answer an app's decoration request (`None`: it left it to us). The mode goes out
    /// with the initial configure, or now if that was already sent (sending before it would
    /// break the initial commit/configure order).
    fn answer_decoration(&mut self, toplevel: ToplevelSurface, requested: Option<DecorationMode>) {
        let app = smithay::wayland::compositor::with_states(toplevel.wl_surface(), |states| {
            states
                .data_map
                .get::<smithay::wayland::shell::xdg::XdgToplevelSurfaceData>()
                .and_then(|d| d.lock().ok()?.app_id.clone())
        })
        .unwrap_or_default();
        let mode = crate::decoration::answer(requested, crate::titlebar::enabled());
        if self.decoration_askers.first_ask(&app, requested) {
            tracing::info!("decoration: {app:?} asked for a server-side title bar; told {mode:?}");
        }
        toplevel.with_pending_state(|s| s.decoration_mode = Some(mode));
        if toplevel.is_initial_configure_sent() {
            toplevel.send_pending_configure();
        }
    }
}

impl XdgDecorationHandler for Kompact {
    fn new_decoration(&mut self, toplevel: ToplevelSurface) {
        self.answer_decoration(toplevel, None);
    }

    fn request_mode(&mut self, toplevel: ToplevelSurface, mode: DecorationMode) {
        self.answer_decoration(toplevel, Some(mode));
    }

    fn unset_mode(&mut self, toplevel: ToplevelSurface) {
        self.answer_decoration(toplevel, None);
    }
}
delegate_xdg_decoration!(Kompact);

impl SelectionHandler for Kompact {
    type SelectionUserData = ();

    /// 7a: one line per copy, naming the app with the keyboard (never the content).
    fn new_selection(
        &mut self,
        ty: smithay::wayland::selection::SelectionTarget,
        source: Option<smithay::wayland::selection::SelectionSource>,
        _seat: Seat<Self>,
    ) {
        let app = self.focused_window().and_then(|w| crate::shell::app_id(&w));
        match source {
            Some(source) => tracing::info!("clipboard: {ty:?} set while {app:?} has the keyboard, {} formats", source.mime_types().len()),
            None => tracing::info!("clipboard: {ty:?} cleared"),
        }
    }
}

impl DataDeviceHandler for Kompact {
    fn data_device_state(&self) -> &DataDeviceState {
        &self.data_device_state
    }
}
impl ClientDndGrabHandler for Kompact {}
impl ServerDndGrabHandler for Kompact {}
delegate_data_device!(Kompact);

impl PrimarySelectionHandler for Kompact {
    fn primary_selection_state(&self) -> &PrimarySelectionState {
        &self.primary_selection_state
    }
}
delegate_primary_selection!(Kompact);

impl DataControlHandler for Kompact {
    fn data_control_state(&self) -> &DataControlState {
        &self.data_control_state
    }
}
delegate_data_control!(Kompact);

/// Identifies a popup's surface for perf.rs's popup timing (Phase 4c).
fn popup_key(surface: &WlSurface) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    smithay::reexports::wayland_server::Resource::id(surface).hash(&mut h);
    h.finish()
}

/// 12i-1: ext-idle-notify; activity comes from `input.rs` (`note_activity`).
impl smithay::wayland::idle_notify::IdleNotifierHandler for Kompact {
    fn idle_notifier_state(&mut self) -> &mut smithay::wayland::idle_notify::IdleNotifierState<Self> {
        &mut self.idle_notifier_state
    }
}
smithay::delegate_idle_notify!(Kompact);

/// 12i-1: an app asks the screen to stay awake while `surface` shows (`shell.rs` `idle_tick`
/// counts only those whose window is on screen).
impl smithay::wayland::idle_inhibit::IdleInhibitHandler for Kompact {
    fn inhibit(&mut self, surface: WlSurface) {
        tracing::info!("idle: a surface asks to keep the screen awake");
        if !self.idle_inhibitors.contains(&surface) {
            self.idle_inhibitors.push(surface);
        }
    }

    fn uninhibit(&mut self, surface: WlSurface) {
        tracing::info!("idle: a surface no longer keeps the screen awake");
        self.idle_inhibitors.retain(|s| s != &surface);
    }
}
smithay::delegate_idle_inhibit!(Kompact);
