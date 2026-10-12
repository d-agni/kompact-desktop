use std::time::Duration;

use smithay::{
    backend::{
        renderer::{damage::OutputDamageTracker, gles::GlesRenderer},
        winit::{self, WinitEvent},
    },
    output::{Mode, Output, PhysicalProperties, Subpixel},
    reexports::{
        calloop::{timer::Timer, EventLoop},
        wayland_server::Display,
        winit::platform::pump_events::PumpStatus,
    },
    utils::Transform,
};

use crate::state::{ClientState, Kompact};

const FRAME_INTERVAL: Duration = Duration::from_millis(1000 / 60);

pub fn run(spawn_cmd: String) -> anyhow::Result<()> {
    let mut event_loop: EventLoop<Kompact> = EventLoop::try_new()?;
    let mut display: Display<Kompact> = Display::new()?;

    let (mut backend, mut winit_events) = winit::init::<GlesRenderer>()
        .map_err(|e| anyhow::anyhow!("failed to init winit backend: {e}"))?;

    let mode = Mode {
        size: backend.window_size(),
        refresh: 60_000,
    };
    let output = Output::new(
        "WINIT-0".into(),
        PhysicalProperties {
            size: (0, 0).into(),
            subpixel: Subpixel::Unknown,
            make: "kompact".into(),
            model: "winit".into(),
        },
    );
    output.create_global::<Kompact>(&display.handle());
    output.change_current_state(Some(mode), Some(Transform::Flipped180), None, Some((0, 0).into()));
    output.set_preferred(mode);

    let mut state = Kompact::new(&event_loop, &mut display, spawn_cmd);
    state.space.map_output(&output, (0, 0));
    state.outputs.push(output.clone());

    let socket = smithay::wayland::socket::ListeningSocketSource::new_auto()?;
    let socket_name = socket.socket_name().to_os_string();
    event_loop
        .handle()
        .insert_source(socket, move |client_stream, _, state: &mut Kompact| {
            if let Err(e) = state
                .display_handle
                .insert_client(client_stream, std::sync::Arc::new(ClientState::default()))
            {
                tracing::warn!("failed to insert new wayland client: {e}");
            }
        })?;
    std::env::set_var("WAYLAND_DISPLAY", &socket_name);
    tracing::info!("listening on WAYLAND_DISPLAY={:?}", socket_name);

    let mut damage_tracker = OutputDamageTracker::from_output(&output);

    // Kick off the startup app (e.g. a terminal) once the loop is running. Timer's event source
    // isn't Send/Sync (it holds an Rc internally), so `?` can't auto-convert its InsertError into
    // an (implicitly Send+Sync) anyhow::Error — format it into a plain string instead.
    let startup_cmd = state.spawn_cmd.clone();
    event_loop
        .handle()
        .insert_source(Timer::immediate(), move |_, _, state| {
            state.spawn(&startup_cmd);
            smithay::reexports::calloop::timer::TimeoutAction::Drop
        })
        .map_err(|e| anyhow::anyhow!("failed to schedule startup command: {e}"))?;

    // event_loop.run() is calloop's own drive loop: it calls dispatch() on our timeout, then the
    // closure, repeatedly, until something calls `state.loop_signal.stop()`. That replaces the
    // hand-rolled "loop { ... if not running break }" this had before (EventLoop has no
    // is_running() to check in this calloop version).
    event_loop.run(Some(FRAME_INTERVAL), &mut state, move |state| {
        let status = winit_events.dispatch_new_events(|event| match event {
            WinitEvent::Resized { size, .. } => {
                output.change_current_state(
                    Some(Mode { size, refresh: 60_000 }),
                    None,
                    None,
                    None,
                );
            }
            WinitEvent::Input(event) => state.process_input_event(event),
            WinitEvent::CloseRequested => {
                state.loop_signal.stop();
            }
            _ => {}
        });

        if let PumpStatus::Exit(_) = status {
            state.loop_signal.stop();
            return;
        }

        if let Err(e) = render_frame(state, &mut backend, &mut damage_tracker, &output) {
            tracing::warn!("render failed: {e}");
        }

        // 6m-d3: a requested window thumbnail, off screen.
        if !state.thumbs.is_idle() || state.shots.is_pending() {
            crate::screenshot::capture_pending(backend.renderer(), state); // 9a
            crate::thumbnail::capture_next(backend.renderer(), state);
        }

        state.send_frames(&output);
        state.space.refresh();
        state.popups.cleanup();

        if let Err(e) = display.flush_clients() {
            tracing::warn!("flush_clients failed: {e}");
        }
        if let Err(e) = display.dispatch_clients(state) {
            tracing::warn!("dispatch_clients failed: {e}");
        }
    })?;

    Ok(())
}

fn render_frame(
    state: &mut Kompact,
    backend: &mut smithay::backend::winit::WinitGraphicsBackend<GlesRenderer>,
    damage_tracker: &mut OutputDamageTracker,
    output: &Output,
) -> anyhow::Result<()> {
    backend.bind()?;

    let elements = crate::render::build_elements(backend.renderer(), state, output);

    damage_tracker.render_output(backend.renderer(), 0, &elements, [0.1, 0.1, 0.15, 1.0])?;
    backend.submit(None)?;
    Ok(())
}
