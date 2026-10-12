use std::sync::{
    atomic::{AtomicBool, Ordering},
    Mutex,
};

use smithay::{
    desktop::{
        find_popup_root_surface, get_popup_toplevel_coords, layer_map_for_output, LayerSurface, PopupKind,
        Window, WindowSurfaceType,
    },
    output::Output,
    wayland::shell::wlr_layer::{KeyboardInteractivity, Layer},
    reexports::{
        wayland_protocols::xdg::shell::server::xdg_toplevel,
        wayland_server::protocol::wl_surface::WlSurface,
    },
    utils::{IsAlive, Logical, Point, Rectangle, Size},
    wayland::{
        compositor::with_states,
        shell::xdg::{PopupSurface, PositionerState, SurfaceCachedState, XdgToplevelSurfaceData},
    },
};

use crate::focus::{ClickFocus, LayerKeys};
use crate::geometry_memory::fit_inside;

use crate::resize::{self, Edges};
use crate::snap::{self, Action, Direction, DropTarget, Layout, Region};
use crate::state::Kompact;
use crate::titlebar;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum DragKind {
    Move,
    /// Resize by dragging these edges (the others stay put).
    Resize(Edges),
}

/// An in-progress pointer drag. Handled by the compositor directly (no PointerGrab): while it is
/// active, pointer motion is not forwarded to clients.
#[derive(Clone)]
pub struct Drag {
    pub window: Window,
    pub kind: DragKind,
    pub start_ptr: Point<f64, Logical>,
    pub start_loc: Point<i32, Logical>,
    pub start_size: Size<i32, Logical>,
    /// True when the client asked for the move (its button press was already delivered to it).
    pub client_initiated: bool,
}

/// Floating geometry to restore when a window leaves maximized/fullscreen or a snap region.
/// Present = window is either filled or snapped (`SnapRegion` says which).
struct SavedGeometry(Mutex<Option<Rectangle<i32, Logical>>>);

/// The region a window is snapped to. `None` (or absent) = not snapped.
struct SnapRegion(Mutex<Option<Region>>);

/// Set when a window leaves maximized/fullscreen: what it filled, what it went back to, and
/// when. Clients like Ptyxis un-maximize themselves and then ask to be moved in the same
/// breath; `begin_drag` uses this to put the window under the pointer instead of leaving it at
/// its old spot (6m-d2 retest).
struct JustRestored(Mutex<Option<(Rectangle<i32, Logical>, Rectangle<i32, Logical>, std::time::Instant)>>);

/// How long after a restore a move request still counts as dragging the window out.
const RESTORE_GRAB_MS: u128 = 1000;

/// Set while a window is being resized from its left and/or top edge. The window has to move as
/// it changes size so the opposite edges stay put, and that has to follow the size the client
/// actually commits (not the one we asked for), so it's applied on each commit. Kept after the
/// button is released until the client has acked the final, non-resizing configure.
struct ResizeAnchor(Mutex<Option<Anchor>>);

#[derive(Clone, Copy)]
struct Anchor {
    start: Rectangle<i32, Logical>,
    edges: Edges,
    finishing: bool,
}

const TILED: [xdg_toplevel::State; 4] = [
    xdg_toplevel::State::TiledLeft,
    xdg_toplevel::State::TiledRight,
    xdg_toplevel::State::TiledTop,
    xdg_toplevel::State::TiledBottom,
];

/// Set on a new toplevel until it has been placed. Placement needs the window's real size, which
/// is only known once the client has committed its first buffer, so mapping and placing are split.
struct PlacementPending(AtomicBool);

impl Kompact {
    /// The output containing `pos`, if any.
    pub fn output_at(&self, pos: Point<f64, Logical>) -> Option<&Output> {
        self.outputs.iter().find(|o| {
            self.space.output_geometry(o).is_some_and(|g| g.to_f64().contains(pos))
        })
    }

    /// The output things happen on when nothing more specific applies (new windows, keyboard
    /// actions without a window): the one under the pointer, else the first (Phase 5).
    pub fn active_output(&self) -> Option<&Output> {
        let pos = self.seat.get_pointer().map(|p| p.current_location());
        pos.and_then(|p| self.output_at(p)).or_else(|| self.outputs.first())
    }

    /// 12h-2: the main display (Settings → Displays; else `KOMPACT_PRIMARY`, else the largest):
    /// where the menu bar and the dock go, and windows off an unplugged monitor.
    pub fn main_output(&self) -> Option<&Output> {
        let list: Vec<(String, smithay::utils::Size<i32, Logical>)> =
            self.outputs.iter().map(|o| (o.name(), self.output_geometry_of(Some(o)).size)).collect();
        let named: Vec<(&str, smithay::utils::Size<i32, Logical>)> = list.iter().map(|(n, s)| (n.as_str(), *s)).collect();
        let env = std::env::var("KOMPACT_PRIMARY").ok();
        crate::outputs::pick_main(&named, self.arrangement.primary.as_deref(), env.as_deref()).and_then(|i| self.outputs.get(i))
    }

    /// 12h-2: where a new layer surface that asks for no output goes: menu bars on the main
    /// display, then on each display without one; the dock where it was last sent (`dock_to`),
    /// else the main display; anything else under the pointer.
    pub fn output_for_layer(&self, namespace: &str) -> Option<Output> {
        let has = |o: &Output, ns: &str| layer_map_for_output(o).layers().any(|l| l.namespace() == ns);
        match namespace {
            "kompact-panel" => {
                let names: Vec<String> = self.outputs.iter().map(|o| o.name()).collect();
                let refs: Vec<&str> = names.iter().map(String::as_str).collect();
                let taken: Vec<&str> =
                    self.outputs.iter().zip(&refs).filter(|(o, _)| has(o, namespace)).map(|(_, n)| *n).collect();
                let main = self.main_output().and_then(|m| self.outputs.iter().position(|o| o == m));
                crate::outputs::next_menu_bar(&refs, main, &taken, self.arrangement.every_menu_bar).and_then(|i| self.outputs.get(i)).cloned()
            }
            "kompact-dock" => self
                .dock_target
                .as_deref()
                .and_then(|t| self.outputs.iter().find(|o| o.name() == t))
                .or_else(|| self.main_output())
                .cloned(),
            _ => self.active_output().cloned(),
        }
    }

    /// The output a window is on: the one containing its centre, else the active one.
    pub fn window_output(&self, window: &Window) -> Option<&Output> {
        let r = self.current_rect(window);
        let center = r.loc + Point::from((r.size.w / 2, r.size.h / 2));
        self.output_at(center.to_f64()).or_else(|| self.active_output())
    }

    /// The output containing `pos`, else the active one.
    fn output_at_or_active(&self, pos: Point<i32, Logical>) -> Option<&Output> {
        self.output_at(pos.to_f64()).or_else(|| self.active_output())
    }

    /// Geometry of `output`; a sane default before any output is known (or after the only one
    /// was unplugged).
    pub fn output_geometry_of(&self, output: Option<&Output>) -> Rectangle<i32, Logical> {
        output
            .and_then(|o| self.space.output_geometry(o))
            .unwrap_or_else(|| Rectangle::new((0, 0).into(), (1280, 720).into()))
    }

    /// `output` minus layer-shell exclusive zones (panels). `non_exclusive_zone()` is
    /// output-local, so it's shifted by the output's position.
    pub fn usable_area_of(&self, output: Option<&Output>) -> Rectangle<i32, Logical> {
        let Some(output) = output else { return self.output_geometry_of(None) };
        let zone = layer_map_for_output(output).non_exclusive_zone();
        self.area_from_zone(output, zone)
    }

    /// A layer map's `non_exclusive_zone()` (relative to its output) as a global usable area;
    /// the whole output if the zone is empty (no layer map arranged yet).
    pub fn area_from_zone(&self, output: &Output, mut zone: Rectangle<i32, Logical>) -> Rectangle<i32, Logical> {
        let geo = self.output_geometry_of(Some(output));
        if zone.size.w <= 0 || zone.size.h <= 0 {
            return geo;
        }
        zone.loc += geo.loc;
        zone
    }

    /// Geometry of the active output (pointer clamping, drop targets).
    pub fn output_geometry(&self) -> Rectangle<i32, Logical> {
        self.output_geometry_of(self.active_output())
    }

    /// Area new windows are placed in: the active output minus panels.
    pub fn usable_area(&self) -> Rectangle<i32, Logical> {
        self.usable_area_of(self.active_output())
    }

    /// Usable area of the output containing `pos` (else the active one): where a remembered or
    /// restored rectangle is kept on screen.
    fn usable_area_at(&self, pos: Point<i32, Logical>) -> Rectangle<i32, Logical> {
        self.usable_area_of(self.output_at_or_active(pos))
    }

    pub fn focused_window(&self) -> Option<Window> {
        let focus = self.seat.get_keyboard()?.current_focus()?;
        self.window_for_surface(&focus)
    }

    pub fn set_focus(&mut self, window: Option<&Window>) {
        // 8d: the lock surface keeps the keyboard.
        if self.lock.is_locked() {
            return;
        }
        let Some(keyboard) = self.seat.get_keyboard() else { return };
        // 6c: while a layer surface holds the keyboard exclusively (launcher), windows can be
        // raised but not focused.
        if let Some(layer) = self.exclusive_layer_surface() {
            if let Some(window) = window {
                self.space.raise_element(window, false);
            }
            tracing::debug!("focus change kept on exclusive layer surface {:?}", layer.namespace());
            return;
        }
        let serial = smithay::utils::SERIAL_COUNTER.next_serial();
        if let Some(window) = window {
            self.space.raise_element(window, true);
            keyboard.set_focus(self, window.toplevel().map(|t| t.wl_surface().clone()), serial);
            if let Some(toplevel) = window.toplevel() {
                toplevel.send_configure();
            }
        } else {
            keyboard.set_focus(self, None, serial);
            self.clipboard_unfocus();
        }
    }

    /// Cycle keyboard focus to the next window in stacking order; 10c: then through this
    /// workspace's minimized windows, bringing back the one picked (`workspace::next_in_cycle`).
    pub fn focus_next_window(&mut self) {
        let visible: Vec<Window> = self.space.elements().cloned().collect();
        let minimized = self.workspaces.minimized_here();
        let current = self.focused_window();
        let Some(next) = crate::workspace::next_in_cycle(&visible, &minimized, current.as_ref()) else { return };
        if minimized.contains(&next) {
            tracing::info!("next window: bringing back minimized {:?}", app_id(&next));
            self.activate_window(&next);
        } else {
            self.set_focus(Some(&next));
        }
    }

    pub fn close_focused_window(&mut self) {
        if let Some(window) = self.focused_window() {
            if let Some(toplevel) = window.toplevel() {
                toplevel.send_close();
            }
        }
    }

    /// Give `window` keyboard focus and the activated look without changing the stacking order
    /// (for focus-follows-mouse; clicking still goes through `set_focus`, which raises).
    fn focus_without_raise(&mut self, window: &Window) {
        if self.lock.is_locked() {
            return;
        }
        let Some(keyboard) = self.seat.get_keyboard() else { return };
        let Some(surface) = window.toplevel().map(|t| t.wl_surface().clone()) else { return };
        let serial = smithay::utils::SERIAL_COUNTER.next_serial();
        keyboard.set_focus(self, Some(surface), serial);
        for w in self.space.elements() {
            if w.set_activated(w == window) {
                if let Some(t) = w.toplevel() {
                    t.send_pending_configure();
                }
            }
        }
    }

    /// Pointer over a layer-shell surface on the layers drawn above windows (panels, overlays).
    fn pointer_over_layer(&self, pos: Point<f64, Logical>) -> bool {
        // Through the input region, not the surface's box: the dock (6m-d2) is a tall,
        // mostly see-through surface that only takes input where its icons are.
        self.layer_hit(pos, &[Layer::Overlay, Layer::Top]).is_some()
    }

    /// Called on every pointer motion: with focus-follows-mouse on, focus the window under `pos`.
    pub fn focus_follows_pointer(&mut self, pos: Point<f64, Logical>) {
        if !self.focus_follows_mouse {
            return;
        }
        let grabbed = self.seat.get_pointer().is_some_and(|p| p.is_grabbed());
        let focus = self.seat.get_keyboard().and_then(|k| k.current_focus());
        let focus_on_non_window = focus
            .as_ref()
            .is_some_and(|s| s.alive() && self.window_for_surface(s).is_none());
        let target = crate::focus::hover_focus_target(crate::focus::Hover {
            enabled: true,
            dragging: self.drag.is_some(),
            grabbed,
            over_layer: self.pointer_over_layer(pos),
            focus_on_non_window,
            // kompact's title bar and resize margin are the window's too (the bar sits outside
            // the app's surface, so `window_under` alone finds the window behind it).
            under: self.decoration_hit(pos).map(|(w, _)| w).or_else(|| self.window_under(pos)),
            focused: self.focused_window(),
        });
        if let Some(w) = target {
            self.focus_without_raise(&w);
        }
    }

    pub fn toggle_focus_follows_mouse(&mut self) {
        let value = if self.focus_follows_mouse { "off" } else { "on" };
        // 11c: the key changes the setting too, so the Settings window and the file agree.
        self.change_setting("focus-follows-mouse", value);
    }

    /// 12f: a new libinput device: the pointer settings on it, and remembered for later changes.
    pub fn add_input_device(&mut self, mut d: smithay::reexports::input::Device) {
        let what = configure_device(&self.prefs, &mut d);
        tracing::info!("input: {} ({what})", d.name());
        self.input_devices.push(d);
    }

    /// 12f: the pointer settings on every device again.
    fn configure_devices(&mut self) {
        let prefs = self.prefs.clone();
        for d in self.input_devices.iter_mut() {
            let what = configure_device(&prefs, d);
            tracing::info!("input: {} ({what})", d.name());
        }
    }

    /// 12f: the next keyboard layout (the switch key, or the menu bar's choice).
    pub fn cycle_layout(&mut self) {
        let Some(keyboard) = self.seat.get_keyboard() else { return };
        keyboard.with_xkb_state(self, |mut ctx| ctx.cycle_next_layout());
        self.send_keyboard();
    }

    /// 12f: layout `n` (0-based) of the configured ones.
    pub fn choose_layout(&mut self, n: u32) {
        let Some(keyboard) = self.seat.get_keyboard() else { return };
        let count = keyboard.with_xkb_state(self, |ctx| ctx.xkb().lock().unwrap().layouts().count() as u32);
        if n < count {
            keyboard.with_xkb_state(self, |mut ctx| ctx.set_layout(smithay::input::keyboard::Layout(n)));
            self.send_keyboard();
        }
    }

    /// 12f: the configured layouts, with xkb's names, and the active one (`keyboard` message).
    pub fn send_keyboard(&mut self) {
        let Some(keyboard) = self.seat.get_keyboard() else { return };
        let (names, active) = keyboard.with_xkb_state(self, |ctx| {
            let xkb = ctx.xkb().lock().unwrap();
            let names: Vec<String> = xkb.layouts().map(|l| xkb.layout_name(l).to_string()).collect();
            (names, xkb.active_layout().0)
        });
        let codes: Vec<&str> = self.prefs.layouts.split(',').filter(|c| !c.is_empty()).collect();
        tracing::info!("keyboard: layout {} of {} ({})", active + 1, names.len(), names.get(active as usize).map(String::as_str).unwrap_or("?"));
        self.ipc.broadcast(&crate::ipc::encode_keyboard(&codes, &names, active));
    }

    /// 12f: the keyboard as the settings say (layouts, Caps Lock, repeat). Err: xkb refused it.
    fn apply_keyboard(&mut self, prefs: &crate::prefs::Prefs) -> Result<(), String> {
        let Some(keyboard) = self.seat.get_keyboard() else { return Ok(()) };
        let (layout, variant, options) = prefs.xkb_parts();
        let xkb = smithay::input::keyboard::XkbConfig { layout: &layout, variant: &variant, options, ..Default::default() };
        keyboard.set_xkb_config(self, xkb).map_err(|_| format!("xkb doesn't know the layouts {:?}", prefs.layouts))?;
        keyboard.change_repeat_info(prefs.repeat_rate as i32, prefs.repeat_delay as i32);
        Ok(())
    }

    /// 11c: the settings for the shell (`settings` message).
    fn send_settings(&mut self, message: Option<&str>) {
        let mut shown = self.prefs.clone();
        shown.animations = self.animations;
        shown.focus_follows_mouse = self.focus_follows_mouse;
        let mut locked: Vec<&str> = Vec::new();
        if self.animations_from_env {
            locked.push("animations");
        }
        if std::env::var("KOMPACT_SCREENSHOT_DIR").is_ok_and(|d| !d.trim().is_empty()) {
            locked.push("shots-dir");
        }
        let shots = crate::screenshot::current_dir(shown.shots_dir.as_deref()).map(|d| d.display().to_string()).unwrap_or_default();
        self.ipc.broadcast(&shown.to_json(&locked, message, !titlebar::style().light, &shots, &self.terminal_cmd));
    }

    /// 10d-3b: apps (GTK, libadwaita, Firefox) the same look, through the session's dconf
    /// database (`theme::sync_apps`; nothing if the locked theme profile is installed).
    pub fn sync_app_theme(&self) {
        let style = titlebar::style();
        crate::theme::sync_apps(!style.light, self.prefs.accent, style.right);
    }

    /// 10d-3: title bars as Settings → Appearance says (light or dark now, buttons' side).
    /// Returns whether that changed; the bars are redrawn on the next frame (`BarKey`).
    pub fn apply_appearance(&mut self) -> bool {
        let style = titlebar::Style {
            light: self.prefs.appearance.is_light(crate::prefs::local_hour()),
            right: self.prefs.buttons_right,
        };
        if style == titlebar::style() {
            return false;
        }
        titlebar::set_style(style);
        tracing::info!(
            "appearance: {} ({}), title-bar buttons on the {}",
            if style.light { "light" } else { "dark" },
            self.prefs.appearance.name(),
            if style.right { "right" } else { "left" }
        );
        true
    }

    /// 11c: change one setting (System Settings, or the focus-follows-mouse key), apply it,
    /// save it, and tell the shell.
    pub fn change_setting(&mut self, key: &str, value: &str) {
        self.change_setting_with(key, value, false);
    }

    /// `path`: a folder setting (`set-path`).
    pub fn change_setting_with(&mut self, key: &str, value: &str, path: bool) {
        if key == "animations" && self.animations_from_env {
            tracing::info!("settings: animations {value} not applied, KOMPACT_ANIMATIONS wins this session");
            self.send_settings(Some("KOMPACT_ANIMATIONS is set for this session"));
            return;
        }
        let mut next = self.prefs.clone();
        next.focus_follows_mouse = self.focus_follows_mouse;
        let changed = if path { next.set_path(key, value) } else { next.set(key, value) };
        if let Err(e) = changed {
            tracing::info!("settings: {e}");
            self.send_settings(Some(&e));
            return;
        }
        // 12f: the keyboard first: a layout xkb refuses isn't kept.
        if matches!(key, "layouts" | "caps" | "repeat-delay" | "repeat-rate") {
            if let Err(e) = self.apply_keyboard(&next) {
                tracing::info!("settings: {e}");
                self.send_settings(Some(&e));
                return;
            }
        }
        self.prefs = next.clone();
        if !self.animations_from_env {
            self.animations = next.animations;
            if !self.animations {
                self.anims = Default::default();
            }
        }
        self.focus_follows_mouse = next.focus_follows_mouse;
        let saved = match crate::prefs::save(&next) {
            Ok(p) => format!("saved to {}", p.display()),
            Err(e) => format!("not saved: {e}"),
        };
        tracing::info!("settings: {key} = {value} ({saved})");
        if key == "focus-follows-mouse" {
            tracing::info!("focus-follows-mouse: {value}");
        }
        self.apply_appearance();
        self.sync_app_theme();
        if matches!(key, "pointer-speed" | "pointer-accel" | "natural-scroll" | "mouse-natural-scroll" | "left-handed" | "tap-to-click") {
            self.configure_devices();
        }
        if matches!(key, "layouts" | "caps") {
            self.send_keyboard();
        }
        self.send_settings(None);
    }

    pub fn window_under(&self, pos: Point<f64, Logical>) -> Option<Window> {
        self.space.element_under(pos).map(|(w, _)| w.clone())
    }

    /// The top-most thing under `pos` that takes input, in drawing order (6c): overlay and top
    /// layer surfaces, windows, then bottom and background layer surfaces. Returns it with the
    /// exact `wl_surface` hit (a subsurface or popup counts) and that surface's global origin.
    pub fn hit(&self, pos: Point<f64, Logical>) -> Option<(Hit, WlSurface, Point<f64, Logical>)> {
        if let Some((layer, surface, loc)) = self.layer_hit(pos, &[Layer::Overlay, Layer::Top]) {
            return Some((Hit::Layer(layer), surface, loc));
        }
        // 6m-e3: kompact's title bar and resize margin belong to no client (the window behind
        // mustn't get the pointer there).
        if self.decoration_hit(pos).is_some() {
            return None;
        }
        if let Some((window, loc)) = self.space.element_under(pos) {
            if let Some((surface, p)) = window.surface_under(pos - loc.to_f64(), WindowSurfaceType::ALL) {
                return Some((Hit::Window(window.clone()), surface, (p + loc).to_f64()));
            }
        }
        self.layer_hit(pos, &[Layer::Bottom, Layer::Background])
            .map(|(layer, surface, loc)| (Hit::Layer(layer), surface, loc))
    }

    /// 6m-e3: the decorated window whose title bar or resize margin is on top at `pos`, and
    /// which part of it. Panels and menus above windows, and windows in front, cover it.
    pub fn decoration_hit(&self, pos: Point<f64, Logical>) -> Option<(Window, titlebar::Part)> {
        if !titlebar::enabled() || self.layer_hit(pos, &[Layer::Overlay, Layer::Top]).is_some() {
            return None;
        }
        for w in self.space.elements().rev() {
            if titlebar::decorated(w) {
                if let Some(part) = titlebar::part_at(self.current_rect(w), titlebar::HEIGHT, pos) {
                    return Some((w.clone(), part));
                }
            }
            let Some(loc) = self.space.element_location(w) else { continue };
            let render_loc = loc - w.geometry().loc;
            if w.surface_under(pos - render_loc.to_f64(), WindowSurfaceType::ALL).is_some() {
                return None;
            }
        }
        None
    }

    /// 6m-e3: a left press on a decoration. A resize margin starts a resize at once; the bar
    /// waits for the pointer to move (`bar_moved`), so a click (or double-click) on a maximized
    /// window's bar doesn't restore it; a button acts on release over the same button.
    pub fn press_decoration(&mut self, window: Window, part: titlebar::Part, pos: Point<f64, Logical>) {
        self.set_focus(Some(&window));
        match part {
            titlebar::Part::Edge(edges) => self.begin_drag(window, DragKind::Resize(edges), false),
            _ => self.bar_input.pressed = Some((window, part, pos)),
        }
    }

    /// 6m-e3: the pointer moved to `pos` with the bar pressed: past `BAR_DRAG_PX` it's a move.
    pub fn bar_moved(&mut self, pos: Point<f64, Logical>) {
        let Some((window, titlebar::Part::Bar, start)) = &self.bar_input.pressed else { return };
        let d = pos - *start;
        if d.x * d.x + d.y * d.y >= BAR_DRAG_PX * BAR_DRAG_PX {
            let window = window.clone();
            self.bar_input.pressed = None;
            self.bar_input.last_bar_click = None;
            self.begin_drag(window, DragKind::Move, false);
        }
    }

    /// 6m-e3: the left button came up at `pos` after `press_decoration` (and no move began).
    pub fn release_decoration(&mut self, pos: Point<f64, Logical>) {
        let Some((window, part, _)) = self.bar_input.pressed.take() else { return };
        if !window.alive() {
            return;
        }
        match part {
            titlebar::Part::Bar => {
                let now = std::time::Instant::now();
                let double = self.bar_input.last_bar_click.take().is_some_and(|(w, t)| {
                    w == window && now.duration_since(t).as_millis() <= titlebar::DOUBLE_CLICK_MS
                });
                if double {
                    // 11c: what the setting says (macOS: Zoom, Minimize or Do Nothing).
                    match self.prefs.double_click {
                        crate::prefs::DoubleClick::Zoom => {
                            let on = !self.is_filled(&window);
                            tracing::info!("title bar double-click: {} {:?}", if on { "maximize" } else { "restore" }, app_id(&window));
                            self.animated_fill(&window, on);
                        }
                        crate::prefs::DoubleClick::Minimize => {
                            tracing::info!("title bar double-click: minimize {:?}", app_id(&window));
                            self.minimize_window(&window);
                        }
                        crate::prefs::DoubleClick::Nothing => {
                            tracing::info!("title bar double-click: nothing (the setting) {:?}", app_id(&window));
                        }
                    }
                } else {
                    self.bar_input.last_bar_click = Some((window, now));
                }
            }
            button if self.decoration_hit(pos).is_some_and(|(w, p)| w == window && p == button) => {
                tracing::info!("title bar button {button:?} on {:?}", app_id(&window));
                match button {
                    titlebar::Part::Close => {
                        if let Some(t) = window.toplevel() {
                            t.send_close();
                        }
                    }
                    titlebar::Part::Minimize => self.minimize_window(&window),
                    _ => {
                        let on = !self.is_filled(&window);
                        self.animated_fill(&window, on);
                    }
                }
            }
            _ => {} // released off the button: nothing, as on macOS
        }
    }

    /// 6m-e3: the resize arrow to show over a decorated window's resize margin at `pos`.
    pub fn update_bar_hover(&mut self, pos: Point<f64, Logical>) {
        let hit = self.decoration_hit(pos);
        self.bar_input.hover = match hit {
            Some((_, titlebar::Part::Edge(e))) => Some(e),
            _ => None,
        };
        self.bar_input.buttons = match hit {
            Some((w, titlebar::Part::Close | titlebar::Part::Minimize | titlebar::Part::Maximize)) => Some(w),
            _ => None,
        };
    }

    /// The top-most layer surface on `bands` (searched in that order) under `pos` whose input
    /// region contains it.
    fn layer_hit(&self, pos: Point<f64, Logical>, bands: &[Layer]) -> Option<(LayerSurface, WlSurface, Point<f64, Logical>)> {
        let output = self.output_at(pos)?;
        let origin = self.output_geometry_of(Some(output)).loc;
        let local = pos - origin.to_f64();
        let map = layer_map_for_output(output);
        for band in bands {
            let Some(layer) = map.layer_under(*band, local) else { continue };
            let Some(geo) = map.layer_geometry(layer) else { continue };
            if let Some((surface, p)) = layer.surface_under(local - geo.loc.to_f64(), WindowSurfaceType::ALL) {
                return Some((layer.clone(), surface, (p + geo.loc + origin).to_f64()));
            }
        }
        None
    }

    /// 6p-d: add this commit's new damage to the upload meter (`uploads.rs`), under the layer
    /// namespace or app-id of the surface's root. Called right after Smithay's buffer handler.
    pub fn meter_upload(&mut self, surface: &WlSurface) {
        use smithay::backend::renderer::{
            buffer_type,
            utils::{CommitCounter, RendererSurfaceStateUserData},
            BufferType,
        };
        /// The last commit counted for a surface.
        struct Counted(std::cell::Cell<Option<CommitCounter>>);
        let damage = with_states(surface, |states| {
            let data = states.data_map.get::<RendererSurfaceStateUserData>()?;
            let state = data.lock().ok()?;
            states.data_map.insert_if_missing(|| Counted(Default::default()));
            let counted = &states.data_map.get::<Counted>()?.0;
            let now = state.current_commit();
            if counted.get() == Some(now) {
                return None;
            }
            // The first commit seen counts as all of it (that's what gets uploaded then).
            let px = crate::uploads::pixels(state.damage_since(counted.get()).iter().map(|r| (r.size.w, r.size.h)));
            counted.set(Some(now));
            let shm = matches!(state.buffer().and_then(|b| buffer_type(b)), Some(BufferType::Shm));
            Some((px, shm))
        });
        let Some((px, shm)) = damage else { return };
        let mut root = surface.clone();
        while let Some(parent) = smithay::wayland::compositor::get_parent(&root) {
            root = parent;
        }
        let owner = match self.layer_for_surface(&root) {
            Some(layer) => layer.namespace().to_string(),
            None => self.window_for_surface(&root).and_then(|w| app_id(&w)).unwrap_or_else(|| "other".into()),
        };
        self.uploads.get_mut().add(&owner, px, shm);
    }

    /// The layer surface (on any output) whose main surface is `surface`.
    fn layer_for_surface(&self, surface: &WlSurface) -> Option<LayerSurface> {
        self.outputs.iter().find_map(|o| {
            layer_map_for_output(o).layer_for_surface(surface, WindowSurfaceType::TOPLEVEL).cloned()
        })
    }

    /// The layer surface that holds the keyboard exclusively right now, if any (6c).
    pub fn exclusive_layer_surface(&self) -> Option<LayerSurface> {
        let mut layers = Vec::new();
        for o in &self.outputs {
            let map = layer_map_for_output(o);
            for band in [Layer::Top, Layer::Overlay] {
                layers.extend(map.layers_on(band).map(|l| (l.clone(), true, layer_keys(l))));
            }
        }
        crate::focus::exclusive_layer(&layers)
    }

    /// Give the keyboard to a layer surface; windows lose the activated look meanwhile.
    /// 6m-e5b: the desktop was clicked. The window with the keyboard loses it, and every window
    /// is told it isn't activated (only a real change is configured), so apps that draw their own
    /// bar dim it too. A layer surface with the keyboard (the launcher) is left alone.
    pub fn clear_window_focus(&mut self) {
        if self.lock.is_locked() {
            return;
        }
        let Some(keyboard) = self.seat.get_keyboard() else { return };
        if let Some(window) = self.focused_window() {
            tracing::info!("focus: desktop clicked, {:?} loses the keyboard", app_id(&window));
            keyboard.set_focus(self, None, smithay::utils::SERIAL_COUNTER.next_serial());
            self.clipboard_unfocus();
        }
        for window in self.space.elements() {
            window.set_activated(false);
            if let Some(toplevel) = window.toplevel() {
                toplevel.send_pending_configure();
            }
        }
    }

    /// 7a: nothing has the keyboard; nobody is offered the clipboard (7b: nor the primary selection). (Smithay 0.4.4 calls
    /// `focus_changed` only when focus moves to a surface, not when it's cleared.)
    fn clipboard_unfocus(&self) {
        smithay::wayland::selection::data_device::set_data_device_focus(&self.display_handle, &self.seat, None);
        smithay::wayland::selection::primary_selection::set_primary_focus(&self.display_handle, &self.seat, None);
    }

    pub fn focus_layer(&mut self, layer: &LayerSurface, why: &str) {
        if self.lock.is_locked() {
            return;
        }
        let Some(keyboard) = self.seat.get_keyboard() else { return };
        if keyboard.current_focus().as_ref() == Some(layer.wl_surface()) {
            return;
        }
        let serial = smithay::utils::SERIAL_COUNTER.next_serial();
        keyboard.set_focus(self, Some(layer.wl_surface().clone()), serial);
        for w in self.space.elements() {
            Self::deactivate(w);
        }
        tracing::info!("keyboard focus to layer surface {:?} ({why})", layer.namespace());
    }

    /// After a layer surface committed or went away (6c): an exclusive one gets the keyboard;
    /// if the keyboard is on a layer surface that is `gone` or no longer takes it, it goes back to
    /// the top-most window.
    pub fn refresh_layer_focus(&mut self, gone: Option<&WlSurface>) {
        if let Some(layer) = self.exclusive_layer_surface() {
            self.focus_layer(&layer, "exclusive");
            return;
        }
        let Some(focus) = self.seat.get_keyboard().and_then(|k| k.current_focus()) else { return };
        let lost = gone == Some(&focus)
            || self.layer_for_surface(&focus).is_some_and(|l| layer_keys(&l) == LayerKeys::None);
        if lost {
            let top = self.space.elements().last().cloned();
            tracing::info!("keyboard focus back from layer surface to {:?}", top.as_ref().and_then(app_id));
            self.set_focus(top.as_ref());
        }
    }

    /// The toplevel for `surface`, whether on the active workspace or parked on another one.
    pub fn window_for_surface(&self, surface: &WlSurface) -> Option<Window> {
        let is_it = |w: &&Window| w.toplevel().map(|t| t.wl_surface() == surface).unwrap_or(false);
        self.space
            .elements()
            .find(is_it)
            .or_else(|| self.workspaces.parked().find(is_it))
            .cloned()
    }

    /// Set a popup's pending position from `positioner`, flipped/slid to stay on the output.
    /// Popups of toplevels and of layer surfaces (panels) are both handled; if the root can't be
    /// found the positioner's plain geometry is used.
    pub fn position_popup(&self, popup: &PopupSurface, positioner: PositionerState) {
        let kind = PopupKind::Xdg(popup.clone());
        let root_loc = find_popup_root_surface(&kind)
            .ok()
            .and_then(|root| self.popup_root_location(&root));
        let target = root_loc.map(|loc| {
            let output = self.output_geometry_of(self.output_at_or_active(loc));
            crate::popup::constraint_target(output, loc, get_popup_toplevel_coords(&kind))
        });
        popup.with_pending_state(|s| {
            s.positioner = positioner;
            s.geometry = match target {
                Some(t) => positioner.get_unconstrained_geometry(t),
                None => positioner.get_geometry(),
            };
        });
    }

    /// Global position of a popup chain's root: a toplevel's window geometry, or a layer
    /// surface's position on the output.
    fn popup_root_location(&self, root: &WlSurface) -> Option<Point<i32, Logical>> {
        if let Some(w) = self.window_for_surface(root) {
            return self.window_location(&w);
        }
        self.outputs.iter().find_map(|output| {
            let map = layer_map_for_output(output);
            let layer = map.layer_for_surface(root, WindowSurfaceType::TOPLEVEL)?;
            Some(map.layer_geometry(layer)?.loc + self.output_geometry_of(Some(output)).loc)
        })
    }

    /// Where a window is: in the space if it's on the active workspace, else where it's parked.
    pub fn window_location(&self, window: &Window) -> Option<Point<i32, Logical>> {
        self.space
            .element_location(window)
            .or_else(|| self.workspaces.find(window).map(|(_, loc)| loc))
    }

    /// 6m-e2b-2: the size to ask the client for when its frame (window + kompact's title bar, if
    /// it has one) should be `frame` big (not for fullscreen, which has no bar). Takes the
    /// toplevel's lock: call it before `with_pending_state`, never inside.
    fn client_size(window: &Window, frame: Size<i32, Logical>) -> Size<i32, Logical> {
        let bar = if titlebar::has_bar_mode(window) { titlebar::HEIGHT } else { 0 };
        titlebar::client_of(Rectangle::new((0, 0).into(), frame), bar).size
    }

    /// Move a window's frame (6m-e2b-2: the window plus kompact's title bar, if it has one) to
    /// `loc`. On the active workspace that's `map_element` (which also puts it on top); a window
    /// parked on another workspace just gets its parked position updated, so e.g. a hidden
    /// window asking to be maximized doesn't pop up on the current workspace.
    fn place_window(&mut self, window: &Window, loc: Point<i32, Logical>, activate: bool) {
        let loc = loc + Point::from((0, titlebar::bar_height(window)));
        if self.space.element_location(window).is_some() {
            self.space.map_element(window.clone(), loc, activate);
        } else {
            self.workspaces.set_location(window, loc);
        }
    }

    /// A button is held or a popup grab is active; workspace changes would strand it on a
    /// hidden window.
    fn pointer_grabbed(&self) -> bool {
        self.seat.get_pointer().is_some_and(|p| p.is_grabbed())
    }

    fn deactivate(window: &Window) {
        if window.set_activated(false) {
            if let Some(t) = window.toplevel() {
                t.send_pending_configure();
            }
        }
    }

    /// `Super+N`: show workspace `to` (0-based).
    pub fn switch_workspace(&mut self, to: usize) {
        if self.drag.is_some() || self.pointer_grabbed() {
            return; // finish the move/resize (or close the open menu) first
        }
        let visible: Vec<(Window, Point<i32, Logical>)> = self
            .space
            .elements()
            .map(|w| (w.clone(), self.space.element_location(w).unwrap_or_default()))
            .collect();
        let focused = self.focused_window();
        let from = self.workspaces.active();
        let Some(sw) = self.workspaces.switch(to, visible.clone(), focused) else { return };
        for (w, _) in &visible {
            Self::deactivate(w);
            self.space.unmap_elem(w);
        }
        for (w, loc) in &sw.show {
            self.space.map_element(w.clone(), *loc, false);
        }
        self.set_focus(sw.focus.as_ref());
        tracing::info!(
            "workspace {} -> {}: showing {} windows, parked {}; parked per workspace {:?}",
            from + 1,
            to + 1,
            sw.show.len(),
            visible.len(),
            self.workspaces.parked_counts()
        );
        self.refresh_pointer_focus();
    }

    /// 6m-0: the client asked to be minimized (its title-bar button): take it out of the space,
    /// remembering where it was, and focus the top-most window left.
    /// 11a: a window's frame (its title bar included) at `loc`, for an animation.
    fn frame_rectf(window: &Window, loc: Point<i32, Logical>) -> crate::anim::RectF {
        let geo = window.geometry();
        let bar = if titlebar::enabled() && titlebar::decorated(window) { titlebar::HEIGHT } else { 0 };
        crate::anim::RectF::new(loc.x as f64, (loc.y - bar) as f64, geo.size.w as f64, (geo.size.h + bar) as f64)
    }

    /// 11a: where `window` goes when minimized: its dock icon (the shell's `dock-icons`), else
    /// the bottom middle of the monitor it's on.
    fn dock_icon_rect(&self, window: &Window, frame: crate::anim::RectF) -> crate::anim::RectF {
        let (size, icons) = &self.dock_icons;
        let icon = icons.get(&window_id(window)).and_then(|&(x, y)| {
            self.outputs.iter().find_map(|o| {
                let map = layer_map_for_output(o);
                let layer = map.layers().find(|l| l.namespace() == "kompact-dock")?;
                let g = map.layer_geometry(layer)?;
                let og = self.space.output_geometry(o)?;
                Some(((og.loc.x + g.loc.x + x) as f64, (og.loc.y + g.loc.y + y) as f64))
            })
        });
        let centre = icon.unwrap_or_else(|| {
            let (cx, cy) = (frame.x + frame.w / 2.0, frame.y + frame.h / 2.0);
            let o = self
                .outputs
                .iter()
                .filter_map(|o| self.space.output_geometry(o))
                .find(|g| g.to_f64().contains((cx, cy)))
                .or_else(|| self.outputs.first().and_then(|o| self.space.output_geometry(o)))
                .unwrap_or_default();
            (o.loc.x as f64 + o.size.w as f64 / 2.0, (o.loc.y + o.size.h) as f64 - *size as f64)
        });
        crate::anim::icon_rect(frame, centre, *size as f64)
    }

    /// 11a: shrink `window` (at `loc`) into its dock icon, or grow it back out.
    fn start_animation(&mut self, window: &Window, loc: Point<i32, Logical>, kind: crate::anim::Kind) {
        if !self.animations || self.lock.is_locked() {
            return;
        }
        let now = std::time::Instant::now();
        self.anims.prune(now, |w| w.alive());
        let frame = Self::frame_rectf(window, loc);
        let icon = self.dock_icon_rect(window, frame);
        let to = if self.dock_icons.1.contains_key(&window_id(window)) { "its dock icon" } else { "the bottom of the monitor (no dock icon known)" };
        tracing::info!(
            "animation: {} {:?} {} {to} at {:.0},{:.0}",
            if kind == crate::anim::Kind::Minimize { "minimize" } else { "restore" },
            app_id(window),
            if kind == crate::anim::Kind::Minimize { "into" } else { "out of" },
            icon.x + icon.w / 2.0,
            icon.y + icon.h / 2.0
        );
        self.anims.start(window.clone(), crate::anim::Anim::new(kind, now, frame, icon));
    }

    /// 11b: the frame and size of a window about to be maximized, snapped or restored (none if
    /// animations are off, it isn't mapped or has no size yet).
    fn move_from(&self, window: &Window) -> Option<(crate::anim::RectF, (i32, i32))> {
        if !self.animations || self.lock.is_locked() {
            return None;
        }
        let loc = self.space.element_location(window)?;
        let size = window.geometry().size;
        (size.w > 0 && size.h > 0).then(|| (Self::frame_rectf(window, loc), (size.w, size.h)))
    }

    /// 11b: after the change: same size asked for → move now; else hold it where it was until
    /// the app has drawn at the new size (`animate_on_commit`).
    fn move_to(&mut self, window: &Window, from: Option<(crate::anim::RectF, (i32, i32))>) {
        let Some((from, size)) = from else { return };
        let Some(loc) = self.space.element_location(window) else { return };
        let now = std::time::Instant::now();
        let asked = window.toplevel().and_then(|t| t.with_pending_state(|s| s.size));
        if asked.is_some_and(|a| (a.w, a.h) == size) {
            let to = Self::frame_rectf(window, loc);
            if to != from {
                tracing::info!("animation: move {:?} (same size)", app_id(window));
                self.anims.start(window.clone(), crate::anim::Anim::moving(now, from, to));
            }
        } else {
            self.anims.hold(window.clone(), from, size, now);
        }
    }

    /// 11b: maximize or restore with the animation (title bar, keys, the app's own buttons).
    pub fn animated_fill(&mut self, window: &Window, on: bool) {
        let from = self.move_from(window);
        self.set_fill(window, on, false);
        self.move_to(window, from);
    }

    pub fn minimize_window(&mut self, window: &Window) {
        if self.drag.as_ref().is_some_and(|d| &d.window == window) {
            return; // being moved or resized right now
        }
        let Some(loc) = self.space.element_location(window) else { return };
        if !self.workspaces.minimize(window.clone(), loc) {
            return;
        }
        self.start_animation(window, loc, crate::anim::Kind::Minimize);
        Self::deactivate(window);
        self.space.unmap_elem(window);
        let next = self.space.elements().last().cloned();
        self.set_focus(next.as_ref());
        let (here, all) = self.workspaces.minimized_counts();
        tracing::info!(
            "minimized {:?} (was at {:?}); minimized here {here}, all {all} (Super+U restores)",
            app_id(window),
            loc
        );
        self.refresh_pointer_focus();
    }

    /// 6m-0, `Super+U`: bring back the most recently minimized window of this workspace, where
    /// it was, on top and focused.
    pub fn restore_minimized(&mut self) {
        if self.drag.is_some() || self.pointer_grabbed() {
            return;
        }
        let Some((window, loc)) = self.workspaces.restore_last() else {
            tracing::info!("restore: nothing minimized on this workspace");
            return;
        };
        self.space.map_element(window.clone(), loc, false);
        self.start_animation(&window, loc, crate::anim::Kind::Restore);
        self.set_focus(Some(&window));
        let (here, all) = self.workspaces.minimized_counts();
        tracing::info!("restored {:?} at {:?}; minimized here {here}, all {all}", app_id(&window), loc);
        self.refresh_pointer_focus();
    }

    /// 10a: minimize the window with the keyboard.
    pub fn minimize_focused(&mut self) {
        if let Some(w) = self.focused_window() {
            self.minimize_window(&w);
        }
    }

    /// 10a, Snap down: in the Windows preset a window that isn't snapped or maximized is
    /// minimized instead (as Win+Down does); otherwise the usual snapping.
    pub fn snap_down_or_minimize(&mut self) {
        if self.keymap.floating_down_minimizes() {
            if let Some(w) = self.focused_window() {
                if self.snapped_region(&w).is_none() && !self.is_filled(&w) {
                    self.minimize_window(&w);
                    return;
                }
            }
        }
        self.snap_focused(Direction::Down);
    }

    /// 10a: the previous (`-1`) or next (`+1`) workspace; no wrapping, as on macOS.
    pub fn step_workspace(&mut self, delta: i32) {
        let to = self.workspaces.active() as i32 + delta;
        if (0..crate::workspace::COUNT as i32).contains(&to) {
            self.switch_workspace(to as usize);
        }
    }

    /// 10a, Show desktop: minimize every window here; again (with nothing shown) brings them
    /// back where they were.
    pub fn toggle_show_desktop(&mut self) {
        if self.drag.is_some() || self.pointer_grabbed() {
            return;
        }
        let here = self.workspaces.active();
        let visible: Vec<Window> = self.space.elements().cloned().collect();
        if !visible.is_empty() {
            for w in &visible {
                self.minimize_window(w);
            }
            tracing::info!("show desktop: minimized {} windows (again to bring them back)", visible.len());
            self.desktop_shown = Some((here, visible));
            return;
        }
        let Some((ws, windows)) = self.desktop_shown.take().filter(|(ws, _)| *ws == here) else {
            tracing::info!("show desktop: nothing to bring back here");
            return;
        };
        let mut back = 0;
        for w in &windows {
            if let Some(loc) = self.workspaces.restore(w) {
                self.space.map_element(w.clone(), loc, false);
                self.start_animation(w, loc, crate::anim::Kind::Restore);
                back += 1;
            }
        }
        if let Some(top) = windows.iter().rev().find(|w| self.space.elements().any(|e| e == *w)).cloned() {
            self.set_focus(Some(&top));
        }
        tracing::info!("show desktop: brought back {back} of {} windows on workspace {}", windows.len(), ws + 1);
        self.refresh_pointer_focus();
    }

    /// `Super+Shift+N`: send the focused window to workspace `to` (0-based), staying here.
    pub fn send_focused_to_workspace(&mut self, to: usize) {
        if self.drag.is_some() || self.pointer_grabbed() {
            return;
        }
        let Some(w) = self.focused_window() else { return };
        let Some(loc) = self.space.element_location(&w) else { return };
        if !self.workspaces.send(w.clone(), loc, to) {
            return;
        }
        Self::deactivate(&w);
        self.space.unmap_elem(&w);
        let next = self.space.elements().last().cloned();
        self.set_focus(next.as_ref());
        tracing::info!(
            "sent window at {:?} to workspace {}; parked per workspace {:?}",
            loc,
            to + 1,
            self.workspaces.parked_counts()
        );
        self.refresh_pointer_focus();
    }
}

impl Kompact {
    pub fn begin_drag(&mut self, window: Window, kind: DragKind, client_initiated: bool) {
        // 11b: a window being moved or resized is drawn where it is, not mid-animation.
        self.anims.stop(&window);
        if self.is_filled(&window) {
            // Dragging a maximized window by its title bar takes it out of maximized, as on
            // macOS; fullscreen windows and resizes stay put.
            let fullscreen = window
                .toplevel()
                .is_some_and(|t| t.with_pending_state(|s| s.states.contains(xdg_toplevel::State::Fullscreen)));
            if kind != DragKind::Move || fullscreen {
                return;
            }
            self.set_fill(&window, false, false);
        }
        let Some(pointer) = self.seat.get_pointer() else { return };
        if self.space.element_location(&window).is_none() {
            return;
        }
        let frame = self.current_rect(&window);
        let (mut start_loc, mut start_size) = (frame.loc, frame.size);
        let ptr = pointer.current_location();
        if kind == DragKind::Move {
            if let Some((filled, restored)) = self.take_just_restored(&window) {
                // Only if it went back somewhere away from the pointer (a double-click restore
                // and then a drag of the restored window is an ordinary move).
                if filled.to_f64().contains(ptr) && !restored.to_f64().contains(ptr) {
                    start_loc = snap::grab_restored(filled, restored.size, ptr);
                    start_size = restored.size;
                    self.place_window(&window, start_loc, false);
                    tracing::info!("dragged out of maximized: window under the pointer at {:?}", start_loc);
                }
            }
        }
        if self.snapped_region(&window).is_some() {
            match kind {
                // Dragging a snapped window off its region gives it back its floating size, with
                // the pointer at the same relative spot across it so it stays "in hand".
                DragKind::Move => {
                    if let Some(r) = self.unsnap(&window) {
                        let frac = if start_size.w > 0 {
                            (ptr.x - start_loc.x as f64) / start_size.w as f64
                        } else {
                            0.0
                        };
                        let x = (ptr.x - frac * r.size.w as f64).round() as i32;
                        start_loc = Point::from((x, start_loc.y));
                        start_size = r.size;
                        self.place_window(&window, start_loc, false);
                    }
                }
                // Resizing a snapped window makes it floating at whatever size it ends up.
                DragKind::Resize(_) => self.forget_snap(&window),
            }
        }
        if let DragKind::Resize(edges) = kind {
            window
                .user_data()
                .insert_if_missing(|| ResizeAnchor(Mutex::new(None)));
            let anchor = window.user_data().get::<ResizeAnchor>().unwrap();
            *anchor.0.lock().unwrap() = edges.moves_origin().then_some(Anchor {
                start: Rectangle::new(start_loc, start_size),
                edges,
                finishing: false,
            });
            tracing::info!(
                "resize started ({}) from {:?}, edges {:?}",
                if client_initiated { "client border" } else { "Super+right-drag" },
                Rectangle::new(start_loc, start_size),
                edges
            );
        }
        self.space.raise_element(&window, true);
        self.drag = Some(Drag {
            window,
            kind,
            start_ptr: ptr,
            start_loc,
            start_size,
            client_initiated,
        });
    }

    /// Apply pointer motion to the active drag. Returns true if a drag consumed the motion.
    pub fn update_drag(&mut self, pos: Point<f64, Logical>) -> bool {
        let Some(d) = self.drag.clone() else { return false };
        // Phase 3a: if the window died mid-drag (client crashed or disconnected) don't touch it;
        // map_element on it would put a dead window back into the space.
        if !d.window.alive() || self.space.element_location(&d.window).is_none() {
            tracing::info!("drag cancelled: its window is gone");
            self.drag = None;
            return false;
        }
        let delta = pos - d.start_ptr;
        match d.kind {
            DragKind::Move => {
                let d_i: Point<i32, Logical> = delta.to_i32_round();
                let mut loc = d.start_loc + d_i;
                // 13f: not under the menu bar (or above the monitor's top). `loc` is the frame's
                // (kompact's title bar included); `place_window` adds the bar itself.
                let top = self.usable_area_of(self.output_at(pos)).loc.y;
                loc.y = snap::keep_below_top(loc.y, top);
                self.place_window(&d.window, loc, false);
            }
            DragKind::Resize(edges) => {
                let Some(t) = d.window.toplevel() else { return true };
                let (min, max) = with_states(t.wl_surface(), |states| {
                    let mut cached = states.cached_state.get::<SurfaceCachedState>();
                    let c = cached.current();
                    (c.min_size, c.max_size)
                });
                let start = Rectangle::new(d.start_loc, d.start_size);
                let rect = resize::resize_rect(start, edges, delta, min, max);
                let size = Self::client_size(&d.window, rect.size);
                t.with_pending_state(|s| {
                    s.states.set(xdg_toplevel::State::Resizing);
                    s.size = Some(size);
                });
                t.send_pending_configure();
            }
        }
        true
    }

    fn saved_geometry<'a>(&self, window: &'a Window) -> &'a SavedGeometry {
        window
            .user_data()
            .insert_if_missing(|| SavedGeometry(Mutex::new(None)));
        window.user_data().get::<SavedGeometry>().unwrap()
    }

    /// What the window filled and went back to, if it left maximized/fullscreen just now.
    fn take_just_restored(&self, window: &Window) -> Option<(Rectangle<i32, Logical>, Rectangle<i32, Logical>)> {
        let (filled, restored, at) = window.user_data().get::<JustRestored>()?.0.lock().unwrap().take()?;
        (at.elapsed().as_millis() <= RESTORE_GRAB_MS).then_some((filled, restored))
    }

    fn snap_slot<'a>(&self, window: &'a Window) -> &'a SnapRegion {
        window
            .user_data()
            .insert_if_missing(|| SnapRegion(Mutex::new(None)));
        window.user_data().get::<SnapRegion>().unwrap()
    }

    /// Maximized or fullscreen (has saved floating geometry but isn't in a snap region).
    pub fn is_filled(&self, window: &Window) -> bool {
        self.saved_geometry(window).0.lock().unwrap().is_some()
            && self.snapped_region(window).is_none()
    }

    pub fn snapped_region(&self, window: &Window) -> Option<Region> {
        *self.snap_slot(window).0.lock().unwrap()
    }

    /// Remember `floating` as the geometry to restore to, unless one is already remembered (a
    /// window going from one snap region to another, or snapped -> maximized, keeps the original).
    fn remember_floating(&self, window: &Window, floating: Rectangle<i32, Logical>) {
        let mut slot = self.saved_geometry(window).0.lock().unwrap();
        if slot.is_none() {
            *slot = Some(floating);
        }
    }

    fn current_rect(&self, window: &Window) -> Rectangle<i32, Logical> {
        let loc = self.window_location(window).unwrap_or_default();
        titlebar::frame_of(Rectangle::new(loc, window.geometry().size), titlebar::bar_height(window))
    }

    /// Make a window fill the output (maximized or fullscreen), or restore its old geometry.
    /// Maximized fills the usable area (panels stay visible); fullscreen covers the whole output.
    /// It fills the output it's on.
    pub fn set_fill(&mut self, window: &Window, on: bool, fullscreen: bool) {
        let output = self.window_output(window).cloned();
        self.set_fill_on(window, on, fullscreen, output.as_ref());
    }

    /// `set_fill` on a given output (a window dropped on an output's top edge fills that one,
    /// even if most of it hangs over the neighbour).
    pub fn set_fill_on(&mut self, window: &Window, on: bool, fullscreen: bool, output: Option<&Output>) {
        let Some(toplevel) = window.toplevel() else { return };
        let saved = self.saved_geometry(window);

        if on {
            let current = self.current_rect(window);
            let floating = if current.size.w < snap::MIN_FLOATING || current.size.h < snap::MIN_FLOATING {
                // Maximized before its first frame: there is no floating geometry yet.
                let remembered = app_id(window)
                    .and_then(|id| self.geometry_memory.recall(&id))
                    .map(|r| fit_inside(r, self.usable_area_at(r.loc)));
                let r = snap::floating_or_default(current, remembered, self.usable_area_of(output));
                tracing::info!("no floating geometry yet; will restore to {:?}", r);
                r
            } else {
                current
            };
            self.remember_floating(window, floating);
            *self.snap_slot(window).0.lock().unwrap() = None;
            let geo = if fullscreen { self.output_geometry_of(output) } else { self.usable_area_of(output) };
            let flag = if fullscreen {
                xdg_toplevel::State::Fullscreen
            } else {
                xdg_toplevel::State::Maximized
            };
            let size = if fullscreen { geo.size } else { Self::client_size(window, geo.size) };
            toplevel.with_pending_state(|s| {
                for t in TILED {
                    s.states.unset(t);
                }
                s.states.set(flag);
                s.size = Some(size);
            });
            toplevel.send_configure();
            self.place_window(window, geo.loc, true);
            tracing::info!("{} window at {:?}", if fullscreen { "fullscreened" } else { "maximized" }, geo);
        } else {
            let filled = self.current_rect(window);
            let restore = saved.0.lock().unwrap().take().map(|r| fit_inside(r, self.usable_area_at(r.loc)));
            if let Some(r) = restore {
                window
                    .user_data()
                    .insert_if_missing(|| JustRestored(Mutex::new(None)));
                *window.user_data().get::<JustRestored>().unwrap().0.lock().unwrap() =
                    Some((filled, r, std::time::Instant::now()));
            }
            *self.snap_slot(window).0.lock().unwrap() = None;
            let size = restore.map(|r| Self::client_size(window, r.size));
            toplevel.with_pending_state(|s| {
                s.states.unset(xdg_toplevel::State::Maximized);
                s.states.unset(xdg_toplevel::State::Fullscreen);
                for t in TILED {
                    s.states.unset(t);
                }
                s.size = size;
            });
            toplevel.send_configure();
            if let Some(r) = restore {
                self.place_window(window, r.loc, false);
                tracing::info!("restored window to {:?}", r);
            }
        }
    }

    /// Snap a window into `region` of the usable area. Its floating geometry is remembered (if
    /// not already) so `unsnap` can put it back.
    /// It snaps on the output it's on.
    pub fn snap_to(&mut self, window: &Window, region: Region) {
        let output = self.window_output(window).cloned();
        self.snap_to_on(window, region, output.as_ref());
    }

    pub fn snap_to_on(&mut self, window: &Window, region: Region, output: Option<&Output>) {
        let Some(toplevel) = window.toplevel() else { return };
        self.remember_floating(window, self.current_rect(window));
        *self.snap_slot(window).0.lock().unwrap() = Some(region);
        let rect = snap::region_rect(region, self.usable_area_of(output));
        let size = Self::client_size(window, rect.size);
        toplevel.with_pending_state(|s| {
            s.states.unset(xdg_toplevel::State::Maximized);
            s.states.unset(xdg_toplevel::State::Fullscreen);
            // Tiled tells clients like foot to use exactly this size (not round it to whole
            // character cells) and to drop shadows. Smithay filters it out for old clients.
            for t in TILED {
                s.states.set(t);
            }
            s.size = Some(size);
        });
        toplevel.send_configure();
        // map_element puts the window on top; it's the one being acted on, so that's right.
        self.place_window(window, rect.loc, false);
        tracing::info!("snapped window to {:?} at {:?}", region, rect);
    }

    /// Leave the snap region: ask for the remembered floating size again and return the
    /// remembered geometry. The caller decides where the window goes.
    fn unsnap(&mut self, window: &Window) -> Option<Rectangle<i32, Logical>> {
        let toplevel = window.toplevel()?;
        self.snap_slot(window).0.lock().unwrap().take()?;
        let saved = self.saved_geometry(window).0.lock().unwrap().take();
        let restore = saved.map(|r| fit_inside(r, self.usable_area_at(r.loc)));
        let size = restore.map(|r| Self::client_size(window, r.size));
        toplevel.with_pending_state(|s| {
            for t in TILED {
                s.states.unset(t);
            }
            s.size = size;
        });
        toplevel.send_configure();
        tracing::info!("unsnapped window, floating geometry {:?}", restore);
        restore
    }

    /// Drop snap state without restoring the old geometry (the window keeps its current size).
    fn forget_snap(&mut self, window: &Window) {
        let Some(toplevel) = window.toplevel() else { return };
        if self.snap_slot(window).0.lock().unwrap().take().is_none() {
            return;
        }
        self.saved_geometry(window).0.lock().unwrap().take();
        toplevel.with_pending_state(|s| {
            for t in TILED {
                s.states.unset(t);
            }
        });
        toplevel.send_configure();
        tracing::info!("window left its snap region by resize");
    }

    /// `Super+Arrow` on the focused window.
    pub fn snap_focused(&mut self, dir: Direction) {
        let Some(w) = self.focused_window() else { return };
        let layout = match self.snapped_region(&w) {
            Some(r) => Layout::Snapped(r),
            None if self.is_filled(&w) => Layout::Maximized,
            None => Layout::Floating,
        };
        let from = self.move_from(&w);
        match snap::key_action(layout, dir) {
            Action::Snap(r) => self.snap_to(&w, r),
            Action::Maximize => self.set_fill(&w, true, false),
            Action::Restore if layout == Layout::Maximized => self.set_fill(&w, false, false),
            Action::Restore => {
                if let Some(r) = self.unsnap(&w) {
                    self.place_window(&w, r.loc, false);
                    tracing::info!("restored window to {:?}", r);
                }
            }
            Action::Nothing => return,
        }
        self.move_to(&w, from);
    }

    /// Called when the button that started a drag is released at `pos`. A move dropped at a
    /// screen edge or corner snaps there; anything else stays where it was dropped.
    pub fn end_drag(&mut self, d: &Drag, pos: Point<f64, Logical>) {
        // The client may have gone away mid-drag; never map a dead or unmapped window again.
        if !d.window.alive() || self.space.element_location(&d.window).is_none() {
            return;
        }
        if let DragKind::Resize(_) = d.kind {
            self.end_resize(&d.window);
            return;
        }
        // The output the pointer was released on, not the one most of the window is over.
        let output = self.output_at(pos).cloned();
        let Some(target) = snap::drop_target(pos, self.output_geometry_of(output.as_ref())) else { return };
        // Restore to where the drag started, not to the edge where it was dropped.
        self.remember_floating(&d.window, Rectangle::new(d.start_loc, d.start_size));
        tracing::info!("drag released at {:?} -> {:?}", pos, target);
        let from = self.move_from(&d.window);
        match target {
            DropTarget::Region(r) => self.snap_to_on(&d.window, r, output.as_ref()),
            DropTarget::Maximize => self.set_fill_on(&d.window, true, false, output.as_ref()),
        }
        self.move_to(&d.window, from);
    }

    /// Tell the client the interactive resize is over. A left/top anchor stays active until the
    /// client has acked this, so its last resize commit is still positioned correctly.
    fn end_resize(&mut self, window: &Window) {
        let Some(t) = window.toplevel() else { return };
        t.with_pending_state(|s| {
            s.states.unset(xdg_toplevel::State::Resizing);
        });
        t.send_pending_configure();
        if let Some(anchor) = window.user_data().get::<ResizeAnchor>() {
            if let Some(a) = anchor.0.lock().unwrap().as_mut() {
                a.finishing = true;
            }
        }
        tracing::info!("resize finished, requested size {:?}", t.with_pending_state(|s| s.size));
    }

    /// The geometry a window would float at: its remembered pre-snap/maximize geometry if it has
    /// one, otherwise where it is now.
    fn floating_rect(&self, window: &Window) -> Rectangle<i32, Logical> {
        let saved = *self.saved_geometry(window).0.lock().unwrap();
        saved.unwrap_or_else(|| self.current_rect(window))
    }

    fn other_window_has_app_id(&self, window: &Window, id: &str) -> bool {
        self.space
            .elements()
            .chain(self.workspaces.parked())
            .any(|w| w != window && app_id(w).as_deref() == Some(id))
    }

    /// Remember this window's floating geometry under its app-id, and persist if it changed.
    pub fn remember_window(&mut self, window: &Window) {
        let Some(id) = app_id(window) else { return };
        if self.window_location(window).is_none() {
            return;
        }
        // Phase 5: relative to the monitor the window is on (the one with its centre).
        let floating = self.floating_rect(window);
        let centre = floating.loc + Point::from((floating.size.w / 2, floating.size.h / 2));
        let output = self.output_at_or_active(centre).cloned();
        let origin = self.output_geometry_of(output.as_ref()).loc;
        let rect = crate::geometry_memory::moved_between(floating, origin, (0, 0).into());
        if self.geometry_memory.remember(&id, rect) {
            let on = output.map(|o| o.name()).unwrap_or_default();
            tracing::info!("remembered geometry for {id:?}: {rect:?} (relative to {on})");
            self.save_geometry_memory();
        }
    }

    /// Remember every open window, e.g. right before quitting. Bottom-most first so that, if an
    /// app has several windows, the top-most one is what ends up remembered.
    /// Quit cleanly: Super+Escape, the shell's Log Out, or a SIGTERM (6p-b). Remembers window
    /// geometry and stops the portals while our display still exists (6m-d5); the shell is sent
    /// SIGTERM by `main` once the backend has returned.
    pub fn shut_down(&mut self, why: &str) {
        tracing::info!("{why}, shutting down");
        self.remember_all_windows();
        crate::session_env::stop_portals();
        self.loop_signal.stop();
    }

    pub fn remember_all_windows(&mut self) {
        // Parked (other workspaces) first, so the visible top-most window wins per app.
        let windows: Vec<Window> =
            self.workspaces.parked().chain(self.space.elements()).cloned().collect();
        for w in &windows {
            self.remember_window(w);
        }
    }

    fn save_geometry_memory(&self) {
        let Some(path) = &self.geometry_memory_path else { return };
        if let Err(e) = crate::geometry_memory::save(path, &self.geometry_memory) {
            tracing::warn!("could not save window geometry memory to {}: {e}", path.display());
        }
    }

    /// Remembered geometry to use for this new window, if its app has one and no other window
    /// of the same app is open (otherwise they'd all open on the same spot).
    fn recall_for_new_window(&self, window: &Window) -> Option<(String, Rectangle<i32, Logical>)> {
        let id = app_id(window)?;
        if self.other_window_has_app_id(window, &id) {
            return None;
        }
        let rect = self.geometry_memory.recall(&id)?;
        // Phase 5: on the monitor under the pointer (where the app was just opened from).
        let origin = self.output_geometry_of(self.active_output()).loc;
        let rect = crate::geometry_memory::moved_between(rect, (0, 0).into(), origin);
        Some((id, fit_inside(rect, self.usable_area())))
    }

    /// Phase 5: outputs were added, removed or moved. `moves` is every output from before the
    /// change with its geometry now (`None`: removed). Windows (on every workspace) on a moved
    /// output follow it, windows on a removed one go to the first remaining output; snapped and
    /// maximized/fullscreen windows are refitted to where they land. With no output left they
    /// keep their place, for when it comes back.
    pub fn outputs_changed(&mut self, moves: &[(Rectangle<i32, Logical>, Option<Rectangle<i32, Logical>>)]) {
        // 12h-1: the backend lays the outputs out side by side; they go where Settings →
        // Displays put them, and windows follow from their old place to that one.
        let arranged = self.apply_arrangement();
        let moves: Vec<_> = moves
            .iter()
            .map(|(old, new)| (*old, new.map(|n| arranged.iter().find(|(_, side, _)| *side == n).map_or(n, |(_, _, a)| *a))))
            .collect();
        self.relocate_after_output_change(&moves);
        self.send_displays();
    }

    /// 12h-1: move the outputs to where `outputs::arrange` puts them. Every output with its
    /// name and geometry before and after.
    fn apply_arrangement(&mut self) -> Vec<(String, Rectangle<i32, Logical>, Rectangle<i32, Logical>)> {
        let current: Vec<(Output, Rectangle<i32, Logical>)> =
            self.outputs.iter().filter_map(|o| Some((o.clone(), self.space.output_geometry(o)?))).collect();
        let names: Vec<String> = current.iter().map(|(o, _)| o.name()).collect();
        let list: Vec<(&str, smithay::utils::Size<i32, Logical>)> =
            names.iter().zip(&current).map(|(n, (_, r))| (n.as_str(), r.size)).collect();
        let places = crate::outputs::arrange(&list, &self.arrangement);
        let mut out = Vec::new();
        for ((name, (o, r)), p) in names.into_iter().zip(&current).zip(places) {
            if r.loc != p {
                o.change_current_state(None, None, None, Some(p));
                self.space.map_output(o, p);
                tracing::info!("displays: {name} at {},{} (was {},{})", p.x, p.y, r.loc.x, r.loc.y);
            }
            out.push((name, *r, Rectangle::new(p, r.size)));
        }
        out
    }

    /// 12h-1: Settings → Displays dropped `name` at `x`,`y`: it snaps edge to edge with the
    /// others, every monitor's place is saved, and windows, panels and the pointer move with
    /// their monitor (as when one is plugged in).
    pub fn move_display(&mut self, name: &str, x: i32, y: i32) {
        let current: Vec<(String, Rectangle<i32, Logical>)> =
            self.outputs.iter().filter_map(|o| Some((o.name(), self.space.output_geometry(o)?))).collect();
        let Some(moved) = current.iter().find(|(n, _)| n == name).map(|(_, r)| *r) else {
            tracing::warn!("displays: no monitor {name:?}");
            self.send_displays();
            return;
        };
        let others: Vec<Rectangle<i32, Logical>> = current.iter().filter(|(n, _)| n != name).map(|(_, r)| *r).collect();
        let to = crate::outputs::snap(moved.size, Point::from((x, y)), &others);
        for (n, r) in &current {
            self.arrangement.set(n, if n == name { to } else { r.loc });
        }
        let changed = self.apply_arrangement();
        // Saved as placed (moved so the top-left is 0,0).
        for (n, _, after) in &changed {
            self.arrangement.set(n, after.loc);
        }
        tracing::info!("displays: {name} dropped at {x},{y}, snapped to {},{}", to.x, to.y);
        match crate::outputs::save_arrangement(&self.arrangement) {
            Ok(path) => tracing::info!("displays: saved {:?} to {}", self.arrangement.places, path.display()),
            Err(e) => tracing::warn!("displays: can't save the arrangement: {e}"),
        }
        let moves: Vec<_> = changed.iter().map(|(_, before, after)| (*before, Some(*after))).collect();
        self.relocate_after_output_change(&moves);
        self.send_displays();
    }

    /// 12h-1: the `displays` message, for Settings → Displays.
    pub fn send_displays(&mut self) {
        if !self.ipc.has_clients() {
            return;
        }
        let list: Vec<crate::ipc::DisplayInfo> = self
            .outputs
            .iter()
            .filter_map(|o| {
                let g = self.space.output_geometry(o)?;
                let mm = o.physical_properties().size;
                Some(crate::ipc::DisplayInfo {
                    name: o.name(),
                    rect: (g.loc.x, g.loc.y, g.size.w, g.size.h),
                    refresh_mhz: o.current_mode().map_or(0, |m| m.refresh),
                    mm: (mm.w, mm.h),
                })
            })
            .collect();
        let main = self.main_output().map(|o| o.name()).unwrap_or_default();
        self.ipc.broadcast(&crate::ipc::encode_displays(&list, &main, self.arrangement.every_menu_bar));
    }

    /// 12h-2: Settings → Displays made `name` the main display: saved; the dock goes there
    /// (`dock_to`, the shell remaps it), and the shell remaps its menu bars when it sees the new
    /// main display in the `displays` message.
    pub fn set_main_display(&mut self, name: &str) {
        if !self.outputs.iter().any(|o| o.name() == name) {
            tracing::warn!("displays: no monitor {name:?} to make the main display");
            self.send_displays();
            return;
        }
        self.arrangement.primary = Some(name.to_string());
        self.save_displays();
        tracing::info!("displays: {name} is the main display");
        self.dock_target = Some(name.to_string());
        if self.dock_output().as_deref() != Some(name) {
            tracing::info!("dock: moving to the main display {name}");
            self.ipc.broadcast(&crate::ipc::encode_dock_to(name));
        }
        self.send_displays();
    }

    /// 12h-2: a menu bar on every display, or only the main one (the shell makes them).
    pub fn set_menu_bars(&mut self, every: bool) {
        self.arrangement.every_menu_bar = every;
        self.save_displays();
        tracing::info!("displays: menu bar on {}", if every { "every display" } else { "the main display only" });
        self.send_displays();
    }

    fn save_displays(&self) {
        match crate::outputs::save_arrangement(&self.arrangement) {
            Ok(path) => tracing::info!("displays: saved to {}", path.display()),
            Err(e) => tracing::warn!("displays: can't save the arrangement: {e}"),
        }
    }

    /// Phase 5 / 12h-1: windows, panels and the pointer after outputs moved, came or went.
    fn relocate_after_output_change(&mut self, moves: &[(Rectangle<i32, Logical>, Option<Rectangle<i32, Logical>>)]) {
        // 12h-2: windows off a removed monitor go to the main display.
        let fallback = self.main_output().map(|o| self.output_geometry_of(Some(o)));
        let now: Vec<(String, Rectangle<i32, Logical>)> =
            self.outputs.iter().filter_map(|o| Some((o.name(), self.space.output_geometry(o)?))).collect();
        // 5i: outputs gone since the last change, by name (they're already out of the space).
        let removed: Vec<(String, Rectangle<i32, Logical>)> =
            self.last_layout.iter().filter(|(n, _)| !now.iter().any(|(m, _)| m == n)).cloned().collect();
        // Bottom to top, so re-mapping each (which puts it on top) keeps the stacking order.
        let windows: Vec<Window> =
            self.space.elements().chain(self.workspaces.parked()).cloned().collect();
        self.displaced.retain(|(w, _)| windows.contains(w));
        for w in &windows {
            let rect = self.current_rect(w);
            let Some(loc) = crate::outputs::relocate(rect, moves, fallback) else { continue };
            self.place_window(w, loc, false);
            self.refit_after_move(w);
            tracing::info!("output change: moved window {:?} from {:?} to {:?}", app_id(w), rect.loc, loc);
            let center = rect.loc + Point::from((rect.size.w / 2, rect.size.h / 2));
            let from = removed.iter().find(|(_, r)| r.contains(center));
            if let (Some((name, home)), Some((on, at))) = (from, self.place_on_output(w)) {
                let rect = crate::geometry_memory::moved_between(rect, home.loc, (0, 0).into());
                tracing::info!("output change: {:?} goes back to {name} if it returns and the window isn't moved", app_id(w));
                self.displaced.retain(|(x, _)| x != w);
                self.displaced.push((w.clone(), crate::outputs::Displaced { from: name.clone(), rect, on, at }));
            }
        }
        // 5i: windows moved off a monitor that is back now go home, unless the user moved them.
        let named: Vec<(&str, Rectangle<i32, Logical>)> = now.iter().map(|(n, r)| (n.as_str(), *r)).collect();
        for (w, d) in std::mem::take(&mut self.displaced) {
            let place = self.place_on_output(&w);
            match crate::outputs::return_home(&d, place.as_ref().map(|(n, p)| (n.as_str(), *p)), &named) {
                crate::outputs::Return::Wait => self.displaced.push((w, d)),
                crate::outputs::Return::Back(loc) => {
                    self.place_window(&w, loc, false);
                    self.refit_after_move(&w);
                    tracing::info!("output change: moved window {:?} back to {} at {:?}", app_id(&w), d.from, loc);
                }
                crate::outputs::Return::Forget => {
                    tracing::info!("output change: {:?} was moved since, stays off {}", app_id(&w), d.from);
                }
            }
        }
        self.last_layout = now;
        for o in self.outputs.clone() {
            layer_map_for_output(&o).arrange();
        }
        // The pointer moves with its output; off a removed one it goes to the first output.
        if let (Some(pointer), Some(geo)) = (self.seat.get_pointer(), fallback) {
            let pos = pointer.current_location();
            let target = moves.iter().find(|(old, _)| old.to_f64().contains(pos)).and_then(|(old, new)| match new {
                Some(new) if new.loc != old.loc => Some(pos + (new.loc - old.loc).to_f64()),
                Some(_) => None,
                None => Some((geo.loc + Point::from((geo.size.w / 2, geo.size.h / 2))).to_f64()),
            });
            if let Some(target) = target {
                self.warp_pointer(target);
            } else if self.output_at(pos).is_none() {
                self.warp_pointer((geo.loc + Point::from((geo.size.w / 2, geo.size.h / 2))).to_f64());
            }
        }
    }

    /// After an output change moved `w`: a snapped window re-snaps and a maximized/fullscreen
    /// one refills, on the output it's on now.
    fn refit_after_move(&mut self, w: &Window) {
        if let Some(region) = self.snapped_region(w) {
            self.snap_to(w, region);
        } else if self.is_filled(w) {
            let fullscreen = w
                .toplevel()
                .is_some_and(|t| t.with_pending_state(|s| s.states.contains(xdg_toplevel::State::Fullscreen)));
            self.set_fill(w, true, fullscreen);
        }
    }

    /// 5i: the output `w`'s centre is on (by name) and its frame's place relative to that output.
    fn place_on_output(&self, w: &Window) -> Option<(String, Point<i32, Logical>)> {
        let r = self.current_rect(w);
        let o = self.output_at((r.loc + Point::from((r.size.w / 2, r.size.h / 2))).to_f64())?;
        Some((o.name(), r.loc - self.space.output_geometry(o)?.loc))
    }

    /// A panel on `output` changed what it reserves (Phase 6a): the usable area went from `old`
    /// to `new` (global coordinates). Snapped and maximized windows on that output are refit to
    /// the new area; floating windows reaching into newly reserved space move clear of it.
    /// Fullscreen windows ignore panels and stay.
    pub fn usable_area_changed(&mut self, output: &Output, old: Rectangle<i32, Logical>, new: Rectangle<i32, Logical>) {
        // Bottom to top, re-placing every visible window (moved or not), because map_element
        // puts each on top: this keeps the stacking order as it was.
        let windows: Vec<Window> =
            self.space.elements().chain(self.workspaces.parked()).cloned().collect();
        for w in &windows {
            if self.window_output(w) != Some(output) || self.drag.as_ref().is_some_and(|d| &d.window == w) {
                if self.space.element_location(w).is_some() {
                    let loc = self.current_rect(w).loc;
                    self.place_window(w, loc, false);
                }
                continue;
            }
            let rect = self.current_rect(w);
            if let Some(region) = self.snapped_region(w) {
                self.snap_to_on(w, region, Some(output));
            } else if self.is_filled(w) {
                let fullscreen = w
                    .toplevel()
                    .is_some_and(|t| t.with_pending_state(|s| s.states.contains(xdg_toplevel::State::Fullscreen)));
                if fullscreen {
                    self.place_window(w, rect.loc, false);
                } else if let Some(toplevel) = w.toplevel() {
                    // Not set_fill_on: that activates the window, and a refit mustn't move focus.
                    let size = Self::client_size(w, new.size);
                    toplevel.with_pending_state(|s| s.size = Some(size));
                    toplevel.send_configure();
                    self.place_window(w, new.loc, false);
                    tracing::info!("panel change: refit maximized window {:?} to {:?}", app_id(w), new);
                }
            } else if let Some(loc) = crate::outputs::clear_of_reserved(rect, old, new) {
                self.place_window(w, loc, false);
                tracing::info!("panel change: moved window {:?} from {:?} to {:?}", app_id(w), rect.loc, loc);
            } else {
                self.place_window(w, rect.loc, false);
            }
        }
    }

    pub fn toggle_fill_focused(&mut self) {
        if let Some(w) = self.focused_window() {
            let on = !self.is_filled(&w);
            self.animated_fill(&w, on);
        }
    }
}

/// Map a freshly created toplevel and give it focus. It goes at the usable area's origin for now
/// (nothing is drawn until its first buffer arrives) and is moved to its real spot by
/// `place_if_pending` once its size is known.
pub fn map_new_window(state: &mut Kompact, window: Window) {
    window
        .user_data()
        .insert_if_missing(|| PlacementPending(AtomicBool::new(true)));
    let loc = state.usable_area().loc;
    state.space.map_element(window.clone(), loc, true);
    state.set_focus(Some(&window));
    tracing::info!("mapped new toplevel at {:?}, placement pending", loc);
}

/// Called on every toplevel commit. On the first commit that gives the window a real size, run
/// smart placement once and move it there. No-op for every later commit.
pub fn place_if_pending(state: &mut Kompact, window: &Window) {
    let Some(pending) = window.user_data().get::<PlacementPending>() else { return };
    if !pending.0.load(Ordering::Relaxed) {
        return;
    }
    if window.geometry().size.w <= 0 || window.geometry().size.h <= 0 {
        return; // initial (bufferless) commit; wait for the real one
    }
    pending.0.store(false, Ordering::Relaxed);
    // 6m-e2b-2: place the frame (with kompact's title bar, if it has one).
    let size = state.current_rect(window).size;

    // Already maximized/fullscreened, or grabbed by the user before its first frame: leave it.
    let dragging = state.drag.as_ref().is_some_and(|d| &d.window == window);
    if state.is_filled(window) || state.snapped_region(window).is_some() || dragging {
        tracing::info!("placement skipped (window is filled, snapped or being dragged)");
        return;
    }

    let area = state.usable_area();
    if let Some((id, remembered)) = state.recall_for_new_window(window) {
        // The size was requested in `request_remembered_size`; position with the size the
        // client actually came up with, kept on screen.
        let loc = fit_inside(Rectangle::new(remembered.loc, size), state.usable_area_at(remembered.loc)).loc;
        state.place_window(window, loc, false);
        tracing::info!("placed new {id:?} window at remembered {:?} (size {:?})", loc, size);
        return;
    }
    // Other windows, top-most first, as obstacles.
    let existing: Vec<_> = state
        .space
        .elements()
        .rev()
        .filter(|w| *w != window)
        .map(|w| state.current_rect(w))
        .collect();
    let loc = crate::placement::place(area, size, &existing);
    // map_element re-inserts on top of the stack; the new window is already top-most and
    // focused, so that's what we want here.
    state.place_window(window, loc, false);
    tracing::info!(
        "placed new toplevel at {:?} (size {:?}, usable area {:?}, {} other windows)",
        loc,
        size,
        area,
        existing.len()
    );
    let others: Vec<_> = existing
        .iter()
        .map(|r| format!("{},{} {}x{}", r.loc.x, r.loc.y, r.size.w, r.size.h))
        .collect();
    tracing::debug!("placement obstacles (top-most first): [{}]", others.join("; "));
}

pub fn unmap_window(state: &mut Kompact, surface: &WlSurface) {
    if let Some(window) = state.window_for_surface(surface) {
        // Phase 3a: a window closing mid-drag ends the drag now, not on the button release.
        if state.drag.as_ref().is_some_and(|d| d.window == window) {
            tracing::info!("drag cancelled: dragged window closed");
            state.drag = None;
        }
        state.remember_window(&window);
        // Closed while parked on another workspace: just forget it, focus here is unaffected.
        if let Some((ws, loc)) = state.workspaces.remove(&window) {
            tracing::info!("unmapped toplevel parked on workspace {} at {:?}", ws + 1, loc);
            return;
        }
        // Did the closing window (or one of its popups) have focus, or does nothing valid?
        let surface_of_window = window.toplevel().map(|t| t.wl_surface().clone());
        let needs_new_focus = match state.seat.get_keyboard().and_then(|k| k.current_focus()) {
            None => true,
            Some(f) => !f.alive() || Some(&f) == surface_of_window.as_ref(),
        };
        let loc = state.space.element_location(&window);
        state.space.unmap_elem(&window);
        tracing::info!("unmapped toplevel that was at {:?}", loc);
        let remaining: Vec<Window> = state.space.elements().cloned().collect();
        if let Some(next) = crate::focus::focus_after_close(&remaining, needs_new_focus) {
            tracing::info!(
                "focus after close: {}",
                if next.is_some() { "top-most remaining window" } else { "none (no windows left)" }
            );
            state.set_focus(next.as_ref());
        }
    }
}

pub fn handle_pointer_focus_on_click(state: &mut Kompact, pos: Point<f64, Logical>) {
    let (layer, window) = match state.hit(pos) {
        Some((Hit::Layer(l), _, _)) => {
            let keys = layer_keys(&l);
            let desktop = matches!(l.layer(), Layer::Bottom | Layer::Background);
            (Some((l, keys, desktop)), None)
        }
        Some((Hit::Window(w), _, _)) => (None, Some(w)),
        None => (None, None),
    };
    match crate::focus::click_focus(state.exclusive_layer_surface().is_some(), layer, window) {
        ClickFocus::Window(w) => state.set_focus(Some(&w)),
        ClickFocus::Layer(l) => state.focus_layer(&l, "clicked"),
        ClickFocus::Keep => {}
        ClickFocus::Clear => state.clear_window_focus(),
    }
}

/// 6m-e3: a press on the bar becomes a move after the pointer travels this far.
const BAR_DRAG_PX: f64 = 4.0;

/// 6m-e3: title-bar pointer state.
#[derive(Default)]
pub struct BarInput {
    /// Over a resize margin: which edges (for the cursor).
    pub hover: Option<Edges>,
    /// 6m-e6: over one of this window's buttons (they show their symbols).
    pub buttons: Option<Window>,
    /// Left button down on a bar or button, where.
    pub pressed: Option<(Window, titlebar::Part, Point<f64, Logical>)>,
    /// The last plain click on a bar, for double-click.
    pub last_bar_click: Option<(Window, std::time::Instant)>,
}

/// What `Kompact::hit` found on top under the pointer.
pub enum Hit {
    Layer(LayerSurface),
    Window(Window),
}

fn layer_keys(layer: &LayerSurface) -> LayerKeys {
    match layer.cached_state().keyboard_interactivity {
        KeyboardInteractivity::Exclusive => LayerKeys::Exclusive,
        KeyboardInteractivity::OnDemand => LayerKeys::OnDemand,
        _ => LayerKeys::None,
    }
}

/// When a new window's app-id becomes known (before its first buffer), ask it for the size its
/// app last had, so its first frame already comes up at that size.
pub fn request_remembered_size(state: &mut Kompact, window: &Window) {
    let pending = window
        .user_data()
        .get::<PlacementPending>()
        .is_some_and(|p| p.0.load(Ordering::Relaxed));
    if !pending || window.geometry().size.w > 0 {
        return; // already placed, or already has a buffer: too late to pick its size
    }
    let Some((id, rect)) = state.recall_for_new_window(window) else { return };
    let Some(toplevel) = window.toplevel() else { return };
    let size = Kompact::client_size(window, rect.size);
    toplevel.with_pending_state(|s| s.size = Some(size));
    toplevel.send_configure();
    tracing::info!("requested remembered size {:?} for new {id:?} window", rect.size);
}

/// The toplevel's xdg app-id, if it has set one.
pub fn app_id(window: &Window) -> Option<String> {
    let toplevel = window.toplevel()?;
    with_states(toplevel.wl_surface(), |states| {
        states
            .data_map
            .get::<XdgToplevelSurfaceData>()?
            .lock()
            .ok()?
            .app_id
            .clone()
    })
}

/// 6m-e7: the window's xdg title (for kompact's title bar). Takes the toplevel's state lock,
/// like `app_id`: never inside `with_pending_state`.
pub fn window_title(window: &Window) -> Option<String> {
    let toplevel = window.toplevel()?;
    with_states(toplevel.wl_surface(), |states| {
        states
            .data_map
            .get::<XdgToplevelSurfaceData>()?
            .lock()
            .ok()?
            .title
            .clone()
    })
}

/// Called on every toplevel commit: while a left/top resize anchor is set, keep the edges
/// opposite the dragged ones fixed for the size the client actually committed.
/// 11b: a held window (maximized, snapped, restored) whose app has now drawn at its new size
/// starts growing from where it was.
pub fn animate_on_commit(state: &mut Kompact, window: &Window) {
    if state.anims.is_empty() {
        return;
    }
    let Some(loc) = state.space.element_location(window) else { return };
    let size = window.geometry().size;
    let to = Kompact::frame_rectf(window, loc);
    if state.anims.committed(window, (size.w, size.h), to, std::time::Instant::now()) {
        tracing::info!("animation: move {:?} to {}x{} at {},{}", app_id(window), size.w, size.h, loc.x, loc.y);
    }
}

pub fn follow_resize_anchor(state: &mut Kompact, window: &Window) {
    let Some(slot) = window.user_data().get::<ResizeAnchor>() else { return };
    let Some(a) = *slot.0.lock().unwrap() else { return };
    if window.geometry().size.w <= 0 || window.geometry().size.h <= 0 {
        return;
    }
    let frame = state.current_rect(window);
    let loc = resize::anchored_location(a.start, a.edges, frame.size);
    if frame.loc != loc {
        // map_element raises; the window being resized was raised when the resize began.
        state.place_window(window, loc, false);
    }
    let still_resizing = window
        .toplevel()
        .is_some_and(|t| t.current_state().states.contains(xdg_toplevel::State::Resizing));
    if a.finishing && !still_resizing {
        *slot.0.lock().unwrap() = None;
    }
}

/// 6m-c: a window's id on the shell channel, given out on first use and kept in the window.
pub fn window_id(window: &Window) -> u64 {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    struct WindowId(u64);
    window
        .user_data()
        .insert_if_missing(|| WindowId(NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)));
    window.user_data().get::<WindowId>().map(|w| w.0).unwrap_or(0)
}

fn title(window: &Window) -> Option<String> {
    let toplevel = window.toplevel()?;
    with_states(toplevel.wl_surface(), |states| {
        states.data_map.get::<XdgToplevelSurfaceData>()?.lock().ok()?.title.clone()
    })
}

impl Kompact {
    /// 6m-c: every window for the shell: on screen bottom-to-top, then parked and minimized.
    pub fn window_infos(&self) -> Vec<crate::ipc::WindowInfo> {
        let active = self.workspaces.active();
        // While a shell surface (Launchpad, a menu) holds the keyboard, the window it came back
        // to still counts as the active app, as on macOS.
        let keyboard_on_layer = self.seat.get_keyboard().and_then(|k| k.current_focus()).is_some();
        let focused = self
            .focused_window()
            .or_else(|| keyboard_on_layer.then(|| self.space.elements().last().cloned()).flatten());
        let info = |w: &Window, workspace: usize, minimized: bool| {
            let fullscreen = w
                .toplevel()
                .is_some_and(|t| t.with_pending_state(|s| s.states.contains(xdg_toplevel::State::Fullscreen)));
            crate::ipc::WindowInfo {
                id: window_id(w),
                app_id: app_id(w).unwrap_or_default(),
                title: title(w).unwrap_or_default(),
                workspace: workspace + 1,
                minimized,
                focused: focused.as_ref() == Some(w),
                maximized: !fullscreen && self.is_filled(w),
                fullscreen,
                output: self.place_on_output(w).map(|(name, _)| name).unwrap_or_default(),
            }
        };
        let mut out: Vec<_> = self.space.elements().map(|w| info(w, active, false)).collect();
        for w in self.workspaces.parked() {
            let ws = self.workspaces.find(w).map(|(ws, _)| ws).unwrap_or(active);
            out.push(info(w, ws, self.workspaces.is_minimized(w)));
        }
        out
    }

    /// 6e: where each window is, in `window_infos` order.
    pub fn window_places(&self) -> Vec<crate::ipc::Placed> {
        self.space
            .elements()
            .chain(self.workspaces.parked())
            .map(|w| {
                let loc = self.window_location(w).unwrap_or_default();
                let size = w.geometry().size;
                crate::ipc::Placed { id: window_id(w), x: loc.x, y: loc.y, w: size.w, h: size.h }
            })
            .collect()
    }

    pub fn window_by_id(&self, id: u64) -> Option<Window> {
        self.space.elements().chain(self.workspaces.parked()).find(|w| window_id(w) == id).cloned()
    }

    /// 6m-c `activate`: show a window wherever it is (switching workspace, restoring it if
    /// minimized), on top and focused. The dock's click.
    pub fn activate_window(&mut self, window: &Window) {
        if self.drag.is_some() || self.pointer_grabbed() {
            return;
        }
        if let Some((ws, _)) = self.workspaces.find(window) {
            if ws != self.workspaces.active() {
                self.switch_workspace(ws);
            }
            if let Some(loc) = self.workspaces.restore(window) {
                self.space.map_element(window.clone(), loc, false);
                self.start_animation(window, loc, crate::anim::Kind::Restore);
                tracing::info!("restored {:?} at {:?} (shell)", app_id(window), loc);
            }
        }
        if self.space.element_location(window).is_some() {
            self.set_focus(Some(window));
            self.refresh_pointer_focus();
        }
    }

    /// Run one shell request.
    pub fn handle_shell_request(&mut self, req: crate::ipc::Request) {
        use crate::ipc::Request;
        tracing::info!("shell channel: {req:?}");
        // 8d: nothing the shell asks for happens behind the lock screen.
        if self.lock.is_locked() {
            tracing::info!("shell channel: ignored, the screen is locked");
            return;
        }
        let window = |s: &Self, id: u64| {
            let w = s.window_by_id(id);
            if w.is_none() {
                tracing::warn!("shell channel: no window {id}");
            }
            w
        };
        match req {
            Request::Activate(id) => {
                if let Some(w) = window(self, id) {
                    self.activate_window(&w);
                }
            }
            Request::Close(id) => {
                if let Some(t) = window(self, id).as_ref().and_then(|w| w.toplevel().cloned()) {
                    t.send_close();
                }
            }
            Request::Minimize(id) => {
                if let Some(w) = window(self, id) {
                    self.minimize_window(&w);
                }
            }
            Request::Thumb(id) => self.thumbs.request(id),
            Request::Workspace(n) => self.switch_workspace(n - 1),
            Request::Layout => {
                let o = self.output_geometry_of(self.active_output());
                let msg = crate::ipc::encode_layout((o.loc.x, o.loc.y, o.size.w, o.size.h), &self.window_places());
                self.ipc.broadcast(&msg);
            }
            Request::Lock => self.start_lock("the shell's menu"),
            Request::Token => {
                // 8a: made by kompact itself (no app behind it): good for MAX_AGE.
                let token = self.activation_state.create_external_token(None).0.to_string();
                tracing::info!("activation: token for an app the shell starts");
                self.ipc.broadcast(&crate::ipc::encode_token(&token));
            }
            Request::Logout => {
                self.shut_down("log out requested by the shell");
            }
            Request::Shot { x, y, w, h } => self.region_shot(x, y, w, h),
            Request::Keys(k) => self.keys_request(k),
            Request::Set(None) => self.send_settings(None),
            Request::KeyboardLayout(n) => self.choose_layout(n),
            Request::Displays => self.send_displays(),
            Request::DisplayMove { name, x, y } => self.move_display(&name, x, y),
            Request::DisplayMain(name) => self.set_main_display(&name),
            Request::MenuBars(every) => self.set_menu_bars(every),
            Request::KeepAwake(apps) => {
                tracing::info!("idle: the shell says {} keep the screen awake (D-Bus)", if apps.is_empty() { "no apps".to_string() } else { apps.join(", ") });
                self.bus_awake = apps;
            }
            Request::Set(Some((key, value))) => self.change_setting(&key, &value),
            Request::SetPath(key, path) => self.change_setting_with(&key, &path, true),
            Request::DockIcons { size, icons } => {
                tracing::debug!("dock icons: {} windows, size {size}", icons.len());
                self.dock_icons = (size, icons.into_iter().map(|(id, x, y)| (id, (x, y))).collect());
            }
            Request::DockSide(side) => {
                if self.dock_side != side {
                    tracing::info!("dock: on the {side:?} edge (the Dock settings)");
                    self.dock_side = side;
                    self.dock_push = Default::default();
                }
            }
            Request::DockFollow(on) => {
                let word = if on { "on" } else { "off" };
                if self.dock_follow_from_env {
                    tracing::info!("dock follows the pointer: the Dock settings say {word}, KOMPACT_DOCK_FOLLOW wins ({})", if self.dock_follow { "on" } else { "off" });
                } else if self.dock_follow != on {
                    self.dock_follow = on;
                    self.dock_push = Default::default();
                    tracing::info!("dock follows the pointer: {word} (the Dock settings)");
                }
            }
        }
    }

    /// 10b: the settings app reads and changes the shortcuts (`keymap.rs`). Changes apply at
    /// once and are saved to `~/.config/kompact/keys`.
    fn keys_request(&mut self, req: crate::ipc::KeysRequest) {
        use crate::ipc::{encode_keymap, encode_keys_result, encode_recorded, KeysRequest as K};
        use crate::keymap::{format_combos, normalize_for, parse_combos, Action, Keymap, Preset};
        let what = match &req {
            K::Get => "keys".to_string(),
            K::Preset { preset, keep } => format!("preset {preset} {}", if *keep { "keep" } else { "drop" }),
            K::Bind { action, keys } => format!("bind {action} {keys}"),
            K::Reset(a) => format!("reset {a}"),
            K::ResetAll => "reset-all".into(),
            K::Record => "record".into(),
            K::RecordCancel => "record-cancel".into(),
        };
        let action = |id: &str| Action::from_id(id).ok_or_else(|| format!("no action {id:?}"));
        // Ok(Some(message)): changed, save and send the keymap; Ok(None): nothing changed.
        let outcome: Result<Option<String>, String> = match req {
            K::Get => {
                self.ipc.broadcast(&encode_keymap(&self.keymap));
                return;
            }
            K::Preset { preset, keep } => match Preset::from_id(&preset) {
                None => Err(format!("no preset {preset:?} (macos, windows, classic)")),
                Some(p) => {
                    let (map, dropped) = self.keymap.with_preset(p, keep);
                    self.keymap = map;
                    Ok(Some(if dropped.is_empty() {
                        format!("preset {} ({})", p.id(), if keep { "your changes kept" } else { "your changes dropped" })
                    } else {
                        format!("preset {}; these changes didn't fit and were dropped: {}", p.id(), dropped.join("; "))
                    }))
                }
            },
            K::Bind { action: id, keys } => action(&id).and_then(|a| {
                let combos = normalize_for(a, parse_combos(&keys)?);
                self.keymap.set(a, combos)?;
                Ok(Some(format!("{} = {}", a.id(), format_combos(self.keymap.combos(a)))))
            }),
            K::Reset(id) => action(&id).and_then(|a| {
                self.keymap.reset(a)?;
                Ok(Some(format!("{} = {} (the preset's)", a.id(), format_combos(self.keymap.combos(a)))))
            }),
            K::ResetAll => {
                self.keymap = Keymap::new(self.keymap.preset());
                Ok(Some(format!("every key back to the {} preset", self.keymap.preset().id())))
            }
            K::Record => {
                if self.key_recorder.is_active() {
                    Err("already recording".into())
                } else {
                    self.key_recorder.start(std::time::Instant::now());
                    tracing::info!("keys: recording the next combination (Esc cancels, {} s)", crate::keymap::RECORD_TIMEOUT.as_secs());
                    Ok(None)
                }
            }
            K::RecordCancel => {
                if let Some(r) = self.key_recorder.cancel("cancelled") {
                    tracing::info!("keys: recording cancelled by the settings app");
                    self.ipc.broadcast(&encode_recorded(&r));
                }
                Ok(None)
            }
        };
        match outcome {
            Ok(Some(msg)) => {
                let saved = match crate::keymap::save(&self.keymap) {
                    Ok(path) => format!("saved to {}", path.display()),
                    Err(e) => {
                        tracing::warn!("keys: not saved: {e}");
                        format!("not saved: {e}")
                    }
                };
                tracing::info!("keys: {msg} ({saved})");
                self.ipc.broadcast(&encode_keys_result(&what, true, &format!("{msg} ({saved})")));
                self.ipc.broadcast(&encode_keymap(&self.keymap));
            }
            Ok(None) => self.ipc.broadcast(&encode_keys_result(&what, true, "")),
            Err(e) => {
                tracing::info!("keys: {what} refused: {e}");
                self.ipc.broadcast(&encode_keys_result(&what, false, &e));
            }
        }
    }

    /// 10b: a recording ended (a combination, Esc, a timeout, the lock screen): tell the shell.
    pub fn finish_recording(&mut self, r: crate::keymap::Recorded) {
        match &r {
            crate::keymap::Recorded::Combo(c) => tracing::info!("keys: recorded {c}"),
            crate::keymap::Recorded::Cancelled(why) => tracing::info!("keys: recording ended ({why})"),
        }
        self.ipc.broadcast(&crate::ipc::encode_recorded(&r));
    }

    /// 9c: Shift+Print. The shell shows the selection (its overlay's `select` mode) and sends
    /// the region back (`Request::Shot`); without a shell, the monitor under the pointer.
    pub fn select_screenshot_region(&mut self) {
        if self.ipc.has_clients() {
            self.shell_key("select");
        } else {
            tracing::info!("screenshot: no shell to select a region with; taking the monitor under the pointer");
            self.shots.request(crate::screenshot::Shot::PointerOutput);
        }
    }

    /// 9c: the shell's selection (global logical coordinates): queue it as a region of the
    /// output its centre is on, clipped to that output.
    fn region_shot(&mut self, x: i32, y: i32, w: i32, h: i32) {
        let centre = Point::<f64, Logical>::from(((x + w / 2) as f64, (y + h / 2) as f64));
        let found = self.output_at(centre).and_then(|o| Some((o.name(), self.space.output_geometry(o)?)));
        let Some((output, g)) = found else {
            tracing::info!("screenshot: region {x},{y} {w}x{h} is on no output, ignored");
            return;
        };
        let Some(rect) = crate::screencopy::clip_region(g.size.w, g.size.h, x - g.loc.x, y - g.loc.y, w, h) else {
            tracing::info!("screenshot: region {x},{y} {w}x{h} is off {output}, ignored");
            return;
        };
        tracing::info!("screenshot: region of {output} {}x{} at {},{} asked by the shell", rect.size.w, rect.size.h, rect.loc.x, rect.loc.y);
        self.shots.request(crate::screenshot::Shot::Region { output, rect });
    }

    /// 12i-1: a key, pointer move, click or scroll.
    pub fn note_activity(&mut self) {
        self.idle.activity(std::time::Instant::now());
        if self.screens_off {
            self.screens_off = false;
            tracing::info!("idle: input, turning the screens on");
        }
        let seat = self.seat.clone();
        self.idle_notifier_state.notify_activity(&seat);
    }

    /// 12i-1: the apps keeping the screen awake (`zwp_idle_inhibit`) whose window is on screen
    /// (not minimized or on another workspace); a layer surface counts as "?". 12i-1b: and those
    /// asking over D-Bus (the shell's `keep-awake`; they have no window to check), while a
    /// shell is connected.
    fn awake_apps(&mut self) -> Vec<String> {
        self.idle_inhibitors.retain(|s| s.alive());
        if !self.bus_awake.is_empty() && !self.ipc.has_clients() {
            tracing::info!("idle: the shell is gone, dropping its keep-awake apps");
            self.bus_awake.clear();
        }
        let mut apps = self.bus_awake.clone();
        apps.dedup();
        for s in &self.idle_inhibitors {
            let mut root = s.clone();
            while let Some(parent) = smithay::wayland::compositor::get_parent(&root) {
                root = parent;
            }
            let window = self.space.elements().find(|w| w.toplevel().is_some_and(|t| t.wl_surface() == &root));
            let parked = self.workspaces.parked().any(|w| w.toplevel().is_some_and(|t| t.wl_surface() == &root));
            let name = match window {
                Some(w) => app_id(w).unwrap_or_else(|| "?".into()),
                None if parked => continue,
                None => "?".into(),
            };
            if !apps.contains(&name) {
                apps.push(name);
            }
        }
        apps
    }

    /// 12i-1: keep-awake requests, and the lock after `lock-after` idle minutes.
    fn idle_tick(&mut self) {
        let awake = self.awake_apps();
        let inhibited = !awake.is_empty();
        self.idle_notifier_state.set_is_inhibited(inhibited);
        if awake != self.awake_apps {
            match awake.as_slice() {
                [] => tracing::info!("idle: nothing keeps the screen awake"),
                list => tracing::info!("idle: kept awake by {}", list.join(", ")),
            }
            self.awake_apps = awake;
            self.ipc.broadcast(&crate::ipc::encode_idle(&self.awake_apps));
            self.ipc.broadcast(&crate::ipc::encode_lock(self.lock.phase == crate::lock::Phase::Locked));
        }
        let now = std::time::Instant::now();
        // 12i-3: also while locked (the lock screen goes dark too).
        if self.idle.screen_off_due(now, self.prefs.screen_off_after, inhibited) {
            tracing::info!("idle: {} min without input, turning the screens off", self.prefs.screen_off_after);
            self.screens_off = true;
        }
        if self.lock.is_locked() {
            return;
        }
        if self.idle.lock_due(now, self.prefs.lock_after, inhibited) {
            self.start_lock(&format!("idle for {} min", self.prefs.lock_after));
        }
    }

    /// The 50 ms shell-channel tick: accept, run requests, send the window list on change.
    pub fn ipc_tick(&mut self) {
        // 10d-3: Auto appearance turns light at 7:00 and dark at 19:00.
        if self.prefs.appearance == crate::prefs::Appearance::Auto && self.appearance_checked.elapsed() >= std::time::Duration::from_secs(30) {
            self.appearance_checked = std::time::Instant::now();
            if self.apply_appearance() {
                self.sync_app_theme();
                self.send_settings(None);
            }
        }
        // 11a: finished animations (and those of closed windows) go.
        if !self.anims.is_empty() {
            self.anims.prune(std::time::Instant::now(), |w| w.alive());
        }
        // 14c-2: the login screen is done: quit, so greetd starts the session.
        if crate::greeter::take_done() {
            self.shut_down("the login screen exited (greeter mode)");
            return;
        }
        // 6p-b: a SIGTERM/SIGINT/SIGHUP since the last tick.
        if let Some(sig) = crate::quit_signal::take() {
            self.shut_down(&format!("{} received", crate::quit_signal::name(sig)));
            return;
        }
        self.lock_tick();
        self.idle_tick();
        self.ipc.ensure_listening();
        let connected = self.ipc.accept();
        // 10b: a recording doesn't outlive its time or the lock screen.
        let now = std::time::Instant::now();
        let ended = if self.lock.is_locked() { self.key_recorder.cancel("locked") } else { self.key_recorder.timed_out(now) };
        if let Some(r) = ended {
            self.finish_recording(r);
        }
        if connected {
            self.ipc.broadcast(&crate::ipc::encode_keymap(&self.keymap));
            self.send_settings(None);
            self.send_keyboard();
            self.send_displays();
            self.ipc.broadcast(&crate::ipc::encode_idle(&self.awake_apps));
        }
        for req in self.ipc.read_requests() {
            self.handle_shell_request(req);
        }
        // 9c: tell the shell about written screenshots (it shows a banner); 10d-4: unless
        // Settings → Screenshots turned the banner off.
        for saved in self.shots.take_saved() {
            if self.ipc.has_clients() && self.prefs.shot_banner {
                self.ipc.broadcast(&crate::ipc::encode_screenshot(&saved.path.to_string_lossy(), saved.width, saved.height));
            }
        }
        if self.ipc.has_clients() {
            let dock = self.dock_output();
            let msg = crate::ipc::encode_windows(self.workspaces.active() + 1, &self.window_infos(), dock.as_deref());
            self.ipc.send_windows_if_changed(msg);
            self.send_pointer_output();
            self.dock_follow_tick(dock.as_deref());
            // 9e-c: the panel's screen-sharing icon.
            let shared = self.shots.streams.outputs();
            if self.ipc.send_sharing_if_changed(crate::ipc::encode_sharing(&shared)) {
                match shared.as_slice() {
                    [] => tracing::info!("sharing indicator: off"),
                    list => tracing::info!("sharing indicator: on ({})", list.join(", ")),
                }
            }
        }
    }

    /// 5j-3: the pointer held at the bottom (10d-1c: the dock's edge) of a monitor the dock isn't
    /// on brings the dock there.
    fn dock_follow_tick(&mut self, dock: Option<&str>) {
        if !self.dock_follow {
            return;
        }
        let edge = self.seat.get_pointer().map(|p| p.current_location()).and_then(|pos| {
            let o = self.output_at(pos)?;
            let g = self.space.output_geometry(o)?;
            crate::dock_edge::on_edge(self.dock_side, (pos.x, pos.y), (g.loc.x, g.loc.y, g.size.w, g.size.h)).then(|| o.name())
        });
        if let Some(to) = self.dock_push.update(edge.as_deref(), dock, std::time::Instant::now()) {
            tracing::info!("dock: pointer held at the {:?} edge of {to}, moving the dock there (from {})", self.dock_side, dock.unwrap_or("?"));
            // 12h-2: the new dock surface goes there (it no longer depends on the pointer).
            self.dock_target = Some(to.clone());
            self.ipc.broadcast(&crate::ipc::encode_dock_to(&to));
        }
    }

    /// 8d: tell the lock client the screen is locked once the cleared frame has been drawn.
    fn lock_tick(&mut self) {
        if crate::lock::confirm_due(self.lock.phase, std::time::Instant::now()) {
            self.lock.phase = crate::lock::Phase::Locked;
            if let Some(locker) = self.lock.pending.take() {
                locker.lock();
                tracing::info!("lock screen: locked");
            }
            // 12i-2: the shell lets the machine sleep once the screen is locked.
            self.ipc.broadcast(&crate::ipc::encode_lock(true));
        }
    }

    /// 8d: run the lock client (`Super+L`, the shell's menu). While locked, only if the one
    /// holding the lock is gone (it crashed: the screen is black and stays locked).
    pub fn start_lock(&mut self, why: &str) {
        if !self.lock.enabled {
            tracing::info!("lock screen: off (KOMPACT_LOCK=off), {why} ignored");
            return;
        }
        if self.lock.is_locked() && self.lock.holder_alive() {
            return;
        }
        tracing::info!("lock screen: starting {:?} ({why})", self.lock.cmd);
        let cmd = self.lock.cmd.clone();
        self.spawn(&cmd);
    }

    /// 8d: on locking, drop drags, grabs and focus; the lock surface gets the keyboard when it
    /// appears.
    pub fn lock_input(&mut self) {
        self.drag = None;
        if let Some(keyboard) = self.seat.get_keyboard() {
            keyboard.unset_grab(self);
            keyboard.set_focus(self, None, smithay::utils::SERIAL_COUNTER.next_serial());
        }
        if let Some(pointer) = self.seat.get_pointer() {
            pointer.unset_grab(self, smithay::utils::SERIAL_COUNTER.next_serial(), self.start_time.elapsed().as_millis() as u32);
        }
        self.clipboard_unfocus();
        self.refresh_pointer_focus();
    }

    /// 8d: the keyboard goes to the lock surface on the pointer's output (else any).
    pub fn focus_lock_surface(&mut self) {
        let Some(keyboard) = self.seat.get_keyboard() else { return };
        let on_pointer = self.active_output().and_then(|o| self.lock.surface_on(o)).cloned();
        let Some(surface) = on_pointer.or_else(|| self.lock.surfaces.iter().find(|(s, _)| s.alive()).map(|(s, _)| s.clone())) else {
            return;
        };
        if keyboard.current_focus().as_ref() != Some(surface.wl_surface()) {
            keyboard.set_focus(self, Some(surface.wl_surface().clone()), smithay::utils::SERIAL_COUNTER.next_serial());
        }
    }

    /// 5j-2: the output the shell's dock (layer namespace `kompact-dock`) is on.
    fn dock_output(&self) -> Option<String> {
        self.outputs
            .iter()
            .find(|o| layer_map_for_output(o).layers().any(|l| l.namespace() == "kompact-dock"))
            .map(|o| o.name())
    }

    /// 5j: tell the shell which output the pointer is on, if that changed.
    fn send_pointer_output(&mut self) {
        let Some(o) = self.active_output() else { return };
        let Some(g) = self.space.output_geometry(o) else { return };
        let msg = crate::ipc::encode_pointer_output(&o.name(), (g.loc.x, g.loc.y, g.size.w, g.size.h));
        self.ipc.send_pointer_output_if_changed(msg);
    }

    /// A shortcut for the shell (`Super+Space` → Spotlight).
    pub fn shell_key(&mut self, action: &str) {
        if self.ipc.has_clients() {
            tracing::info!("shell key: {action}");
            // 5j: the overlay opens on the pointer's output, so the shell must know it first.
            self.send_pointer_output();
            self.ipc.broadcast(&crate::ipc::encode_key(action));
        } else {
            tracing::info!("shell key {action}: no shell connected");
        }
    }
}

/// 12f: the pointer settings on one libinput device, as far as it has them; what was set, for
/// the log.
fn configure_device(p: &crate::prefs::Prefs, d: &mut smithay::reexports::input::Device) -> String {
    use smithay::reexports::input::AccelProfile;
    let mut done = Vec::new();
    if d.config_accel_is_available() {
        let _ = d.config_accel_set_speed(p.pointer_speed as f64 / 10.0);
        let _ = d.config_accel_set_profile(if p.pointer_accel { AccelProfile::Adaptive } else { AccelProfile::Flat });
        done.push(format!("speed {:.1}{}", p.pointer_speed as f64 / 10.0, if p.pointer_accel { "" } else { " flat" }));
    }
    if d.config_scroll_has_natural_scroll() {
        // 13h: touchpads (they tap) and mice each have their own direction.
        let touchpad = d.config_tap_finger_count() > 0;
        let natural = if touchpad { p.natural_scroll } else { p.mouse_natural_scroll };
        let _ = d.config_scroll_set_natural_scroll_enabled(natural);
        done.push(format!("{} natural scrolling {}", if touchpad { "touchpad" } else { "mouse" }, if natural { "on" } else { "off" }));
    }
    if d.config_left_handed_is_available() {
        let _ = d.config_left_handed_set(p.left_handed);
        if p.left_handed {
            done.push("left-handed".into());
        }
    }
    if d.config_tap_finger_count() > 0 {
        let _ = d.config_tap_set_enabled(p.tap_to_click);
        done.push(format!("tap to click {}", if p.tap_to_click { "on" } else { "off" }));
    }
    if done.is_empty() { "no pointer settings".into() } else { done.join(", ") }
}
