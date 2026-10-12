use smithay::{
    backend::input::{
        AbsolutePositionEvent, ButtonState, Event, InputBackend, InputEvent, KeyState,
        KeyboardKeyEvent, PointerButtonEvent, PointerMotionEvent,
    },
    input::{
        keyboard::FilterResult,
        pointer::{ButtonEvent, MotionEvent},
    },
    utils::{Point, SERIAL_COUNTER},
};

use std::time::Instant;

use crate::perf::InputKind;
use crate::snap::Direction;
use crate::state::Kompact;

/// Compositor-level modifier: Logo/Super, so client apps keep Alt+Tab etc. for themselves.
/// Held together with a key, these are swallowed by the compositor instead of reaching clients.
const BTN_LEFT: u32 = 0x110;
const BTN_RIGHT: u32 = 0x111;


impl Kompact {
    /// Route one input event from either backend (winit or libinput) through keybindings,
    /// pointer motion, or straight to the focused client.
    pub fn process_input_event<B: InputBackend>(&mut self, event: InputEvent<B>)
    where
        B::Device: 'static,
    {
        // 12f: libinput's devices (not winit's), for the pointer settings.
        match &event {
            InputEvent::DeviceAdded { device } => {
                if let Some(d) = (device as &dyn std::any::Any).downcast_ref::<smithay::reexports::input::Device>() {
                    self.add_input_device(d.clone());
                }
            }
            InputEvent::DeviceRemoved { device } => {
                if let Some(d) = (device as &dyn std::any::Any).downcast_ref::<smithay::reexports::input::Device>() {
                    self.input_devices.retain(|x| x != d);
                }
            }
            _ => {}
        }
        // 12i-1: input is activity (the idle lock, ext-idle-notify).
        if matches!(
            event,
            InputEvent::Keyboard { .. }
                | InputEvent::PointerMotion { .. }
                | InputEvent::PointerMotionAbsolute { .. }
                | InputEvent::PointerButton { .. }
                | InputEvent::PointerAxis { .. }
        ) {
            self.note_activity();
        }
        // Phase 4b: input-to-photon timing (perf.rs). Key releases don't change the screen.
        let timed = match &event {
            InputEvent::Keyboard { event }
                if event.state() == KeyState::Pressed
                    && !crate::perf::is_modifier_key(event.key_code().raw()) =>
            {
                Some((InputKind::Key, Event::time(event)))
            }
            InputEvent::PointerMotion { event } => Some((InputKind::Pointer, Event::time(event))),
            InputEvent::PointerMotionAbsolute { event } => {
                Some((InputKind::Pointer, Event::time(event)))
            }
            InputEvent::PointerButton { event } => Some((InputKind::Pointer, Event::time(event))),
            InputEvent::PointerAxis { event } => Some((InputKind::Pointer, Event::time(event))),
            _ => None,
        };
        if let Some((kind, event_us)) = timed {
            // Only libinput times are CLOCK_MONOTONIC; perf.rs sanity-checks before using them.
            self.frame_timer.get_mut().input(kind, Instant::now(), Some(event_us));
        }
        // Phase 4c: a press may be what opens the next popup.
        if let InputEvent::PointerButton { event } = &event {
            if event.state() == ButtonState::Pressed {
                self.frame_timer.get_mut().button_pressed(Instant::now(), Some(Event::time(event)));
            }
        }

        match event {
            InputEvent::Keyboard { event, .. } => self.on_keyboard(event),
            InputEvent::PointerMotion { event, .. } => self.on_pointer_motion_relative(event),
            InputEvent::PointerMotionAbsolute { event, .. } => {
                self.on_pointer_motion_absolute(event)
            }
            InputEvent::PointerButton { event, .. } => self.on_pointer_button(event),
            InputEvent::PointerAxis { event, .. } => self.on_pointer_axis(event),
            _ => {}
        }
    }

    fn on_keyboard<B: InputBackend>(&mut self, event: impl KeyboardKeyEvent<B>) {
        let keycode = event.key_code();
        let state = event.state();

        // Dedup against duplicate devices (see `pressed_keys` doc comment on Kompact): a second
        // press with no release in between, or a release with no matching tracked press, is
        // dropped before it ever reaches keyboard.input() / the client.
        match state {
            KeyState::Pressed if !self.pressed_keys.insert(keycode) => return,
            KeyState::Released if !self.pressed_keys.remove(&keycode) => return,
            _ => {}
        }

        let serial = SERIAL_COUNTER.next_serial();
        let time = event.time_msec();

        let Some(keyboard) = self.seat.get_keyboard() else { return };

        keyboard.input::<(), _>(
            self,
            keycode,
            state,
            serial,
            time,
            |compositor_state, mods, handle| {
                use crate::keymap::{Mods, Press};
                let sym = handle.modified_sym();
                // The key's own symbol without Shift or other levels: Super+Shift+3 is "3",
                // Alt+Print is Print (not Sys_Req), whatever the layout.
                let raw_sym = handle.raw_latin_sym_or_raw_current_sym().map(|s| s.raw()).unwrap_or(sym.raw());

                if state != KeyState::Pressed {
                    // 10a: a lone tap of Super, on its release (the press went to the app).
                    if compositor_state.key_tap.release(raw_sym) && !compositor_state.lock.is_locked() {
                        // 10b: being recorded, it's the answer instead.
                        if let Some(r) = compositor_state.key_recorder.tap() {
                            compositor_state.finish_recording(r);
                        } else if let Some(action) = compositor_state.keymap.tap_action() {
                            compositor_state.run_key_action(action, None, " (Super tap)");
                        }
                    }
                    return FilterResult::Forward;
                }
                let held = Mods { ctrl: mods.ctrl, alt: mods.alt, shift: mods.shift, logo: mods.logo };
                // Modifiers before this key went down (the state already includes a Super press).
                let before = Mods { logo: mods.logo && !crate::keymap::is_super_key(raw_sym), ..held };
                compositor_state.key_tap.press(raw_sym, before);

                // Ctrl+Alt+F1..F12 arrive as XF86Switch_VT_1..12; always ours, never rebindable.
                let raw = sym.raw();
                if (0x1008FE01..=0x1008FE0C).contains(&raw) {
                    compositor_state.pending_vt = Some((raw - 0x1008FE01 + 1) as i32);
                    return FilterResult::Intercept(());
                }

                // 10a: the keymap (`keymap.rs`) decides; unbound keys go to the app.
                let press = Press { sym: raw_sym, keycode: handle.raw_code().raw(), mods: held };

                // 10b: the settings app is recording: the combination is the answer, not acted
                // on and not sent to the app (modifiers alone still are, so apps see them come
                // and go).
                if compositor_state.key_recorder.is_active() && !compositor_state.lock.is_locked() {
                    return match compositor_state.key_recorder.press(&press) {
                        Some(r) => {
                            compositor_state.finish_recording(r);
                            FilterResult::Intercept(())
                        }
                        None => {
                            // A modifier on its own (the recording waits for the key with it).
                            tracing::info!(
                                "keys: recording, {} went down",
                                smithay::input::keyboard::xkb::keysym_get_name(smithay::input::keyboard::Keysym::new(raw_sym))
                            );
                            FilterResult::Forward
                        }
                    };
                }
                let found = compositor_state.keymap.lookup(&press);

                // 8d: while locked, every other key goes to the lock surface. The Lock key only
                // starts a new lock client if the one holding the lock is gone.
                if compositor_state.lock.is_locked() {
                    if matches!(found, Some((crate::keymap::Action::Lock, _))) && !compositor_state.lock.holder_alive() {
                        compositor_state.start_lock("the lock key, the lock client is gone");
                        return FilterResult::Intercept(());
                    }
                    return FilterResult::Forward;
                }

                let Some((action, workspace)) = found else { return FilterResult::Forward };
                if compositor_state.run_key_action(action, workspace, "") {
                    FilterResult::Intercept(())
                } else {
                    FilterResult::Forward
                }
            },
        );
        // 12f: after the key that asked for it, so the switch doesn't land mid-keystroke.
        if std::mem::take(&mut self.layout_cycle_pending) {
            self.cycle_layout();
        }
    }

    /// 10a: do what a shortcut is bound to. False if it isn't ours to take after all
    /// (screenshot keys with `KOMPACT_SCREENSHOTS=off` go to the app, as before).
    /// `how` is added to the log line (" (Super tap)").
    pub fn run_key_action(&mut self, action: crate::keymap::Action, workspace: Option<usize>, how: &str) -> bool {
        // 14c-2: on the login screen no shortcut does anything (the key goes to the page).
        if crate::greeter::active() {
            tracing::info!("greeter mode: {} ignored{how}", action.id());
            return false;
        }
        use crate::keymap::Action as A;
        use crate::screenshot::Shot;
        if matches!(action, A::ShotAll | A::ShotPointerOutput | A::ShotRegion) && !self.shots.enabled {
            return false;
        }
        match workspace {
            Some(n) => tracing::info!("key: {} {}{how}", action.id(), n + 1),
            None => tracing::info!("key: {}{how}", action.id()),
        }
        match action {
            A::Spotlight => self.shell_key("spotlight"),
            // 12f: with one layout the key is the app's (Ctrl+Space in an editor).
            A::NextLayout if self.prefs.layout_count() < 2 => return false,
            A::NextLayout => self.layout_cycle_pending = true,
            // 12c: the shell changes the volume (wpctl) and shows the level.
            A::VolumeUp => self.shell_key("volume-up"),
            A::VolumeDown => self.shell_key("volume-down"),
            A::VolumeMute => self.shell_key("volume-mute"),
            A::MicMute => self.shell_key("mic-mute"),
            A::SwitchWindows => self.focus_next_window(),
            A::CloseWindow => self.close_focused_window(),
            A::Minimize => self.minimize_focused(),
            A::RestoreMinimized => self.restore_minimized(),
            A::ToggleFill => self.toggle_fill_focused(),
            A::SnapLeft => self.snap_focused(Direction::Left),
            A::SnapRight => self.snap_focused(Direction::Right),
            A::SnapUp => self.snap_focused(Direction::Up),
            A::SnapDown => self.snap_down_or_minimize(),
            A::ShowWorkspace => {
                if let Some(n) = workspace {
                    self.switch_workspace(n);
                }
            }
            A::SendToWorkspace => {
                if let Some(n) = workspace {
                    self.send_focused_to_workspace(n);
                }
            }
            A::PrevWorkspace => self.step_workspace(-1),
            A::NextWorkspace => self.step_workspace(1),
            A::Overview => self.shell_key("overview"),
            A::ShowDesktop => self.toggle_show_desktop(),
            // 12j: Settings → Default Apps' terminal, else the `--spawn` one.
            A::Terminal => {
                let cmd = self.prefs.terminal.clone().unwrap_or_else(|| self.terminal_cmd.clone());
                self.spawn(&cmd)
            }
            A::Files => {
                let home = std::env::var("HOME").unwrap_or_else(|_| "/".into());
                self.spawn_argv(vec!["xdg-open".into(), home]);
            }
            A::Lock => self.start_lock("the lock key"),
            A::Quit => self.shut_down("quit key pressed"),
            A::ShotAll => self.shots.request(Shot::AllOutputs),
            A::ShotPointerOutput => self.shots.request(Shot::PointerOutput),
            A::ShotRegion => self.select_screenshot_region(),
            A::FocusFollowsMouse => self.toggle_focus_follows_mouse(),
            A::DebugOverlay => self.toggle_overlay(),
        }
        true
    }

    fn on_pointer_motion_relative<B: InputBackend>(&mut self, event: impl PointerMotionEvent<B>) {
        let Some(pointer) = self.seat.get_pointer() else { return };
        let delta: Point<f64, smithay::utils::Logical> =
            (event.delta_x(), event.delta_y()).into();
        let current = pointer.current_location();
        let raw = current + delta;
        // Phase 5: free to cross onto any output, clamped at the outer edges.
        let mut outputs: Vec<_> =
            self.outputs.iter().filter_map(|o| self.space.output_geometry(o)).collect();
        if outputs.is_empty() {
            outputs.push(self.output_geometry());
        }
        let new_pos = crate::outputs::clamp_pointer(raw, current, &outputs);
        self.move_pointer(new_pos, event.time_msec());
    }

    fn on_pointer_motion_absolute<B: InputBackend>(
        &mut self,
        event: impl AbsolutePositionEvent<B>,
    ) {
        let output_geo = self.output_geometry();
        let pos = Point::from((
            event.x_transformed(output_geo.size.w),
            event.y_transformed(output_geo.size.h),
        ));
        self.move_pointer(pos, event.time_msec());
    }

    /// Move the pointer to `pos` as if the user had (Phase 5: off an unplugged output).
    pub fn warp_pointer(&mut self, pos: Point<f64, smithay::utils::Logical>) {
        let time = self.start_time.elapsed().as_millis() as u32;
        self.move_pointer(pos, time);
    }

    /// Re-send pointer focus for the pointer's current spot, e.g. after a workspace switch
    /// swapped the windows under it, so the next click goes to what's actually shown there.
    pub fn refresh_pointer_focus(&mut self) {
        let Some(pointer) = self.seat.get_pointer() else { return };
        let time = self.start_time.elapsed().as_millis() as u32;
        self.move_pointer(pointer.current_location(), time);
    }

    fn move_pointer(&mut self, pos: Point<f64, smithay::utils::Logical>, time: u32) {
        let Some(pointer) = self.seat.get_pointer() else { return };
        // 8d: locked: only the lock surface under the pointer gets it, and the keyboard follows.
        if self.lock.is_locked() {
            let under = self.output_at(pos).cloned().and_then(|o| {
                let origin = self.space.output_geometry(&o)?.loc.to_f64();
                Some((self.lock.surface_on(&o)?.wl_surface().clone(), origin))
            });
            if under.is_none() {
                self.cursor_status = smithay::input::pointer::CursorImageStatus::default_named();
            }
            pointer.motion(self, under, &MotionEvent { location: pos, serial: SERIAL_COUNTER.next_serial(), time });
            pointer.frame(self);
            self.focus_lock_surface();
            return;
        }
        self.bar_moved(pos);
        let dragging = self.update_drag(pos);
        if !dragging {
            self.focus_follows_pointer(pos);
        }
        if dragging {
            self.bar_input.hover = None;
            self.bar_input.buttons = None;
        } else {
            self.update_bar_hover(pos);
        }
        let under = if dragging { None } else { self.surface_under(pos) };
        if under.is_none() && !dragging {
            // Over empty desktop no client owns the cursor; go back to the arrow.
            self.cursor_status = smithay::input::pointer::CursorImageStatus::default_named();
        }
        pointer.motion(
            self,
            under,
            &MotionEvent {
                location: pos,
                serial: SERIAL_COUNTER.next_serial(),
                time,
            },
        );
        pointer.frame(self);
    }

    fn on_pointer_button<B: InputBackend>(&mut self, event: impl PointerButtonEvent<B>) {
        let button = event.button_code();

        // Same duplicate-device dedup as on_keyboard, for the button equivalent of a phantom
        // second click.
        match event.state() {
            ButtonState::Pressed if !self.pressed_buttons.insert(button) => return,
            ButtonState::Released if !self.pressed_buttons.remove(&button) => return,
            _ => {}
        }
        self.key_tap.cancel(); // 10a: Super+click or Super+drag isn't a tap

        let Some(pointer) = self.seat.get_pointer() else { return };
        let serial = SERIAL_COUNTER.next_serial();

        // 8d: locked: no window management, the click goes to the lock surface.
        if self.lock.is_locked() {
            pointer.button(self, &ButtonEvent { serial, time: event.time_msec(), button, state: event.state() });
            pointer.frame(self);
            return;
        }

        if event.state() == ButtonState::Pressed {
            // Super+left-drag moves, Super+right-drag resizes; the click never reaches the client.
            let super_held = self
                .seat
                .get_keyboard()
                .map(|k| k.modifier_state().logo)
                .unwrap_or(false);
            if super_held && (button == BTN_LEFT || button == BTN_RIGHT) {
                // Only a window that's top-most there (not one behind the panel).
                let window = match self.hit(pointer.current_location()) {
                    Some((crate::shell::Hit::Window(w), _, _)) => Some(w),
                    _ => None,
                };
                if let Some(window) = window {
                    self.set_focus(Some(&window));
                    let kind = if button == BTN_LEFT {
                        crate::shell::DragKind::Move
                    } else {
                        // Grab whichever edge/corner the pointer is nearest.
                        let rect = self.space.element_geometry(&window).unwrap_or_default();
                        let edges = crate::resize::nearest_edges(pointer.current_location(), rect);
                        crate::shell::DragKind::Resize(edges)
                    };
                    self.begin_drag(window, kind, false);
                    return;
                }
            }
            // 6m-e3: kompact's title bar, its buttons and the resize margin around it.
            if button == BTN_LEFT && !pointer.is_grabbed() {
                if let Some((window, part)) = self.decoration_hit(pointer.current_location()) {
                    self.press_decoration(window, part, pointer.current_location());
                    return;
                }
            }
            if !pointer.is_grabbed() {
                crate::shell::handle_pointer_focus_on_click(self, pointer.current_location());
            }
        } else if button == BTN_LEFT && self.bar_input.pressed.is_some() {
            self.release_decoration(pointer.current_location());
            return; // we swallowed the press
        } else if let Some(d) = self.drag.take() {
            self.end_drag(&d, pointer.current_location());
            if !d.client_initiated {
                return; // we swallowed the press, so swallow the release too
            }
        }

        pointer.button(
            self,
            &ButtonEvent {
                serial,
                time: event.time_msec(),
                button,
                state: event.state(),
            },
        );
        pointer.frame(self);
    }

    fn on_pointer_axis<B: InputBackend>(
        &mut self,
        event: impl smithay::backend::input::PointerAxisEvent<B>,
    ) {
        use smithay::backend::input::{Axis, AxisSource};
        use smithay::input::pointer::AxisFrame;

        self.key_tap.cancel(); // 10a: scrolling with Super held isn't a tap
        let Some(pointer) = self.seat.get_pointer() else { return };
        // 13h: the distance and the notches separately (as Smithay's anvil), at the Settings
        // speed; a touchpad's lift ends the scroll (kinetic scrolling in apps).
        let source = event.source();
        let mut frame = AxisFrame::new(event.time_msec()).source(source);
        for axis in [Axis::Horizontal, Axis::Vertical] {
            let (amount, v120) = (event.amount(axis), event.amount_v120(axis));
            let (value, steps) = crate::scroll::scaled(amount, v120, self.prefs.scroll_speed);
            if value != 0.0 {
                frame = frame.value(axis, value);
                if let Some(s) = steps {
                    frame = frame.v120(axis, s);
                }
            } else if source == AxisSource::Finger && amount.is_some() {
                frame = frame.stop(axis);
            }
        }
        pointer.axis(self, frame);
        pointer.frame(self);
    }
}
