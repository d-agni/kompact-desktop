//! A minimal wlr-layer-shell client, purpose-built to exercise kompact's layer-shell support —
//! not a real status bar, just enough of one to be a meaningful test:
//!
//! - Anchored to the top edge, full width (TOP | LEFT | RIGHT, size (0, BAR_HEIGHT) — 0 means
//!   "stretch to fill" when anchored to both opposite edges, per the layer-shell protocol).
//! - Sets an exclusive zone equal to its height, so you can check whether kompact's layer-shell
//!   handling actually reserves that space (vs. just rendering the surface on top of everything).
//! - keyboard_interactivity: None, like a real status bar — good for checking kompact doesn't
//!   hand it keyboard focus. `LAYER_TEST_KEYBOARD=exclusive|on-demand` (Phase 6c) asks for the
//!   keyboard instead, like a launcher: key presses are logged and Esc closes the bar.
//! - Redraws continuously (a sweeping gradient) via the frame-callback loop, so this also
//!   exercises repeated commits/damage, not just a single static first frame.
//! - Logs every configure event and pointer enter/leave to stderr, so what the client *asked
//!   for* vs what the compositor *granted* is visible side by side with kompact's own log.
//!
//! Run with kompact already running (point WAYLAND_DISPLAY at its socket, or just run this from
//! a terminal kompact itself spawned, which already has it set):
//!   cargo run --release

use std::convert::TryInto;

use smithay_client_toolkit::{
    compositor::{CompositorHandler, CompositorState},
    delegate_compositor, delegate_keyboard, delegate_layer, delegate_output, delegate_pointer,
    delegate_registry, delegate_seat, delegate_shm,
    output::{OutputHandler, OutputState},
    registry::{ProvidesRegistryState, RegistryState},
    registry_handlers,
    seat::{
        keyboard::{KeyEvent, KeyboardHandler, Keysym, Modifiers},
        pointer::{PointerEvent, PointerEventKind, PointerHandler},
        Capability, SeatHandler, SeatState,
    },
    shell::{
        wlr_layer::{
            Anchor, KeyboardInteractivity, Layer, LayerShell, LayerShellHandler, LayerSurface,
            LayerSurfaceConfigure,
        },
        WaylandSurface,
    },
    shm::{slot::SlotPool, Shm, ShmHandler},
};
use wayland_client::{
    globals::registry_queue_init,
    protocol::{wl_keyboard, wl_output, wl_pointer, wl_seat, wl_shm, wl_surface},
    Connection, QueueHandle,
};

const BAR_HEIGHT: u32 = 32;

/// `LAYER_TEST_KEYBOARD`: none (default), exclusive or on-demand.
fn keyboard_mode() -> (KeyboardInteractivity, &'static str) {
    match std::env::var("LAYER_TEST_KEYBOARD").unwrap_or_default().trim().to_ascii_lowercase().as_str() {
        "exclusive" => (KeyboardInteractivity::Exclusive, "Exclusive"),
        "on-demand" | "ondemand" => (KeyboardInteractivity::OnDemand, "OnDemand"),
        _ => (KeyboardInteractivity::None, "None"),
    }
}

fn main() {
    let (keys, keys_name) = keyboard_mode();
    let conn = Connection::connect_to_env()
        .expect("failed to connect to a Wayland compositor — is WAYLAND_DISPLAY set?");
    let (globals, mut event_queue) = registry_queue_init(&conn).unwrap();
    let qh = event_queue.handle();

    let compositor = CompositorState::bind(&globals, &qh).expect("wl_compositor not available");
    let layer_shell = LayerShell::bind(&globals, &qh)
        .expect("wlr-layer-shell not available — is kompact's layer-shell support actually active?");
    let shm = Shm::bind(&globals, &qh).expect("wl_shm not available");

    let surface = compositor.create_surface(&qh);
    let layer = layer_shell.create_layer_surface(
        &qh,
        surface,
        Layer::Top,
        Some("layer-test"),
        None, // let the compositor pick the output
    );
    layer.set_anchor(Anchor::TOP | Anchor::LEFT | Anchor::RIGHT);
    layer.set_size(0, BAR_HEIGHT); // 0 width = stretch, since we're anchored both sides
    layer.set_exclusive_zone(BAR_HEIGHT as i32);
    layer.set_keyboard_interactivity(keys);
    layer.commit(); // initial commit with no buffer — compositor replies with the first configure

    eprintln!(
        "[layer-test] requested: anchor=TOP|LEFT|RIGHT size=(0,{BAR_HEIGHT}) exclusive_zone={BAR_HEIGHT} \
         keyboard_interactivity={keys_name}"
    );
    eprintln!("[layer-test] waiting for the compositor's first configure...");

    let pool = SlotPool::new(256 * BAR_HEIGHT as usize * 4, &shm).expect("failed to create SHM pool");

    let mut app = LayerTest {
        registry_state: RegistryState::new(&globals),
        seat_state: SeatState::new(&globals, &qh),
        output_state: OutputState::new(&globals, &qh),
        shm,
        pool,
        layer,
        width: 256,
        height: BAR_HEIGHT,
        shift: 0,
        first_configure: true,
        pointer: None,
        keyboard: None,
        exit: false,
    };

    while !app.exit {
        event_queue.blocking_dispatch(&mut app).unwrap();
    }
    eprintln!("[layer-test] exiting");
}

struct LayerTest {
    registry_state: RegistryState,
    seat_state: SeatState,
    output_state: OutputState,
    shm: Shm,
    pool: SlotPool,
    layer: LayerSurface,
    width: u32,
    height: u32,
    shift: u32,
    first_configure: bool,
    pointer: Option<wl_pointer::WlPointer>,
    keyboard: Option<wl_keyboard::WlKeyboard>,
    exit: bool,
}

impl LayerShellHandler for LayerTest {
    fn closed(&mut self, _conn: &Connection, _qh: &QueueHandle<Self>, _layer: &LayerSurface) {
        eprintln!("[layer-test] compositor closed our layer surface");
        self.exit = true;
    }

    fn configure(
        &mut self,
        _conn: &Connection,
        qh: &QueueHandle<Self>,
        _layer: &LayerSurface,
        configure: LayerSurfaceConfigure,
        _serial: u32,
    ) {
        let (w, h) = configure.new_size;
        eprintln!("[layer-test] configure: granted size=({w},{h})");
        self.width = if w == 0 { 256 } else { w };
        self.height = if h == 0 { BAR_HEIGHT } else { h };

        if self.first_configure {
            self.first_configure = false;
            self.draw(qh);
        }
    }
}

impl LayerTest {
    /// A horizontally sweeping gradient, redrawn every frame — proves the frame-callback /
    /// repeated-commit path works, not just a single static buffer.
    fn draw(&mut self, qh: &QueueHandle<Self>) {
        let (width, height) = (self.width, self.height);
        let stride = width as i32 * 4;

        let (buffer, canvas) = self
            .pool
            .create_buffer(width as i32, height as i32, stride, wl_shm::Format::Argb8888)
            .expect("create buffer");

        let shift = self.shift;
        canvas.chunks_exact_mut(4).enumerate().for_each(|(index, chunk)| {
            let x = ((index as u32 + shift) % width) as u32;
            let hue = (x * 360 / width.max(1)) as f32;
            let (r, g, b) = hsv_to_rgb(hue, 0.6, 0.85);
            let array: &mut [u8; 4] = chunk.try_into().unwrap();
            *array = (((0xFFu32) << 24) | ((r as u32) << 16) | ((g as u32) << 8) | b as u32)
                .to_le_bytes();
        });
        self.shift = (self.shift + 2) % width.max(1);

        self.layer.wl_surface().damage_buffer(0, 0, width as i32, height as i32);
        self.layer.wl_surface().frame(qh, self.layer.wl_surface().clone());
        buffer.attach_to(self.layer.wl_surface()).expect("buffer attach");
        self.layer.commit();
    }
}

fn hsv_to_rgb(h: f32, s: f32, v: f32) -> (u8, u8, u8) {
    let c = v * s;
    let x = c * (1.0 - ((h / 60.0) % 2.0 - 1.0).abs());
    let m = v - c;
    let (r, g, b) = match h as u32 {
        0..=59 => (c, x, 0.0),
        60..=119 => (x, c, 0.0),
        120..=179 => (0.0, c, x),
        180..=239 => (0.0, x, c),
        240..=299 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    (
        ((r + m) * 255.0) as u8,
        ((g + m) * 255.0) as u8,
        ((b + m) * 255.0) as u8,
    )
}

impl CompositorHandler for LayerTest {
    fn scale_factor_changed(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface,
        _: i32,
    ) {
    }
    fn transform_changed(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface,
        _: wl_output::Transform,
    ) {
    }
    fn frame(&mut self, _: &Connection, qh: &QueueHandle<Self>, _: &wl_surface::WlSurface, _: u32) {
        self.draw(qh);
    }
    fn surface_enter(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface,
        _: &wl_output::WlOutput,
    ) {
    }
    fn surface_leave(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface,
        _: &wl_output::WlOutput,
    ) {
    }
}

impl OutputHandler for LayerTest {
    fn output_state(&mut self) -> &mut OutputState {
        &mut self.output_state
    }
    fn new_output(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}
    fn update_output(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}
    fn output_destroyed(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}
}

impl SeatHandler for LayerTest {
    fn seat_state(&mut self) -> &mut SeatState {
        &mut self.seat_state
    }
    fn new_seat(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_seat::WlSeat) {}
    fn new_capability(
        &mut self,
        _: &Connection,
        qh: &QueueHandle<Self>,
        seat: wl_seat::WlSeat,
        capability: Capability,
    ) {
        if capability == Capability::Pointer && self.pointer.is_none() {
            eprintln!("[layer-test] pointer capability available");
            self.pointer = Some(self.seat_state.get_pointer(qh, &seat).expect("get pointer"));
        }
        if capability == Capability::Keyboard && self.keyboard.is_none() {
            eprintln!("[layer-test] keyboard capability available");
            self.keyboard = Some(self.seat_state.get_keyboard(qh, &seat, None).expect("get keyboard"));
        }
    }
    fn remove_capability(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: wl_seat::WlSeat,
        capability: Capability,
    ) {
        if capability == Capability::Pointer {
            if let Some(p) = self.pointer.take() {
                p.release();
            }
        }
        if capability == Capability::Keyboard {
            if let Some(k) = self.keyboard.take() {
                k.release();
            }
        }
    }
    fn remove_seat(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_seat::WlSeat) {}
}

impl PointerHandler for LayerTest {
    fn pointer_frame(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_pointer::WlPointer,
        events: &[PointerEvent],
    ) {
        for event in events {
            if &event.surface != self.layer.wl_surface() {
                continue;
            }
            match event.kind {
                PointerEventKind::Enter { .. } => {
                    eprintln!("[layer-test] pointer entered the bar @ {:?}", event.position)
                }
                PointerEventKind::Leave { .. } => eprintln!("[layer-test] pointer left the bar"),
                PointerEventKind::Press { button, .. } => {
                    eprintln!("[layer-test] button {button:#x} pressed @ {:?}", event.position)
                }
                _ => {}
            }
        }
    }
}

impl KeyboardHandler for LayerTest {
    fn enter(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_keyboard::WlKeyboard,
        surface: &wl_surface::WlSurface,
        _: u32,
        _: &[u32],
        _: &[Keysym],
    ) {
        if surface == self.layer.wl_surface() {
            eprintln!("[layer-test] keyboard focus: got it");
        }
    }
    fn leave(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_keyboard::WlKeyboard, surface: &wl_surface::WlSurface, _: u32) {
        if surface == self.layer.wl_surface() {
            eprintln!("[layer-test] keyboard focus: lost it");
        }
    }
    fn press_key(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_keyboard::WlKeyboard, _: u32, event: KeyEvent) {
        eprintln!("[layer-test] key pressed: {:?} {:?}", event.keysym, event.utf8.unwrap_or_default());
        if event.keysym == Keysym::Escape {
            eprintln!("[layer-test] Esc: closing");
            self.exit = true;
        }
    }
    fn release_key(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_keyboard::WlKeyboard, _: u32, _: KeyEvent) {}
    fn update_modifiers(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_keyboard::WlKeyboard, _: u32, _: Modifiers, _: u32) {}
}

impl ShmHandler for LayerTest {
    fn shm_state(&mut self) -> &mut Shm {
        &mut self.shm
    }
}

delegate_compositor!(LayerTest);
delegate_output!(LayerTest);
delegate_shm!(LayerTest);
delegate_seat!(LayerTest);
delegate_pointer!(LayerTest);
delegate_keyboard!(LayerTest);
delegate_layer!(LayerTest);
delegate_registry!(LayerTest);

impl ProvidesRegistryState for LayerTest {
    fn registry(&mut self) -> &mut RegistryState {
        &mut self.registry_state
    }
    registry_handlers![OutputState, SeatState];
}
