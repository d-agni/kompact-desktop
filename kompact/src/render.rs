use smithay::{
    backend::allocator::Fourcc,
    backend::renderer::{
        element::{
            memory::{MemoryRenderBuffer, MemoryRenderBufferRenderElement}, render_elements,
            surface::{render_elements_from_surface_tree, WaylandSurfaceRenderElement},
            utils::RescaleRenderElement,
            AsRenderElements, Kind,
        },
        gles::GlesRenderer,
    },
    desktop::{
        layer_map_for_output,
        space::{space_render_elements, SpaceRenderElements},
    },
    input::pointer::CursorImageSurfaceData,
    output::Output,
    utils::{IsAlive, Logical, Rectangle, Transform},
    wayland::{compositor::with_states, shell::wlr_layer::Layer as WlrLayer},
};

use crate::state::{CursorDraw, Kompact};

render_elements! {
    /// Everything we draw, front-to-back: cursor, then top/overlay layer-shell surfaces
    /// (panels/menus that should sit above windows), then windows, then background/bottom
    /// layer-shell surfaces (wallpaper etc, below everything else).
    pub KompactElement<=GlesRenderer>;
    Space=SpaceRenderElements<GlesRenderer, WaylandSurfaceRenderElement<GlesRenderer>>,
    Cursor=MemoryRenderBufferRenderElement<GlesRenderer>,
    /// Layer surfaces, client cursor surfaces and (6m-e2a) windows: all plain surface trees.
    Layer=WaylandSurfaceRenderElement<GlesRenderer>,
    /// 11a: a window shrinking into the dock or growing out of it (`anim.rs`).
    Scaled=RescaleRenderElement<WaylandSurfaceRenderElement<GlesRenderer>>,
}

/// Render elements for every mapped layer-shell surface on the given layer band. Layers without
/// a committed buffer yet (no geometry assigned) are skipped automatically since `layer_geometry`
/// returns `None` for them.
fn layer_elements(renderer: &mut GlesRenderer, output: &Output, band: WlrLayer) -> Vec<KompactElement> {
    let map = layer_map_for_output(output);
    map.layers_on(band)
        .filter_map(|layer| {
            let loc = map.layer_geometry(layer)?.loc.to_physical(1);
            Some(layer.render_elements::<KompactElement>(renderer, loc, 1.0.into(), 1.0))
        })
        .flatten()
        .collect()
}

/// 6m-e2a: draw each layer surface once (default); `KOMPACT_LAYERS_ONCE=off` is the old path.
pub fn layers_once_from_env(value: Option<&str>) -> bool {
    !matches!(value.map(|v| v.trim().to_ascii_lowercase()).as_deref(), Some("off" | "0" | "no" | "false"))
}

fn layers_once() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| {
        let on = layers_once_from_env(std::env::var("KOMPACT_LAYERS_ONCE").ok().as_deref());
        if on {
            tracing::info!("render: each layer surface drawn once (6m-e2a; KOMPACT_LAYERS_ONCE=off for the old path)");
        } else {
            tracing::info!("render: old path, layer surfaces drawn twice (KOMPACT_LAYERS_ONCE=off)");
        }
        on
    })
}

/// 6m-e2b-1: a window's title-bar image, kept in its user data and redrawn only when its width
/// changes, or (6m-e5) whether it has the keyboard, or (6m-e6) whether the pointer is over its
/// buttons, or (6m-e7) its title: a new buffer then, so the damage tracker repaints it once.
struct BarImage(std::cell::RefCell<Option<(BarKey, MemoryRenderBuffer)>>);
/// Width, focused, pointer over the buttons, title, (10d-3) light/dark and buttons' side.
type BarKey = (i32, bool, bool, Option<String>, crate::titlebar::Style);

/// What `Space::render_elements_for_region` does (smithay 0.4.4 `desktop/space/mod.rs`: top
/// window first, skip those outside `region`, draw at location - geometry offset), plus each
/// decorated window's bar right after the window, so windows in front cover it.
fn windows_with_bars(renderer: &mut GlesRenderer, state: &Kompact, region: &Rectangle<i32, Logical>, out: &mut Vec<KompactElement>) {
    let focused = state.focused_window();
    let now = std::time::Instant::now();
    // 11a: windows being minimized, in front of the others (behind the dock and panel).
    if !state.anims.is_empty() {
        for (window, frame) in state.anims.ghosts(now) {
            if window.alive() {
                draw_window(renderer, state, &window, None, Some(frame), focused.as_ref(), region, out);
            }
        }
    }
    for window in state.space.elements().rev() {
        let Some(loc) = state.space.element_location(window) else { continue };
        let frame = if state.anims.is_empty() { None } else { state.anims.frame(window, now) };
        draw_window(renderer, state, window, Some(loc), frame, focused.as_ref(), region, out);
    }
}

/// One window and its bar: where it is (`loc`), or (11a) scaled into `anim`'s rectangle.
#[allow(clippy::too_many_arguments)]
fn draw_window(
    renderer: &mut GlesRenderer,
    state: &Kompact,
    window: &smithay::desktop::Window,
    loc: Option<smithay::utils::Point<i32, Logical>>,
    anim: Option<crate::anim::Frame>,
    focused: Option<&smithay::desktop::Window>,
    region: &Rectangle<i32, Logical>,
    out: &mut Vec<KompactElement>,
) {
    use crate::titlebar::HEIGHT;
    use smithay::utils::{Point, Size};
    let geo = window.geometry();
    let decorated = crate::titlebar::decorated(window);
    let bar_h = if decorated { HEIGHT } else { 0 };
    let frame_h = (geo.size.h + bar_h) as f64;
    // Where the frame (bar + window) is drawn, its scale (11b: across and down apart, a window
    // changes shape when it's maximized) and opacity.
    let anim = anim.filter(|_| geo.size.w > 0 && frame_h > 0.0);
    let (frame_loc, sx, sy, alpha) = match (anim, loc) {
        (Some(f), _) => (Point::<f64, Logical>::from((f.rect.x, f.rect.y)), f.rect.w / geo.size.w as f64, f.rect.h / frame_h, f.alpha),
        (None, Some(loc)) => (Point::<i32, Logical>::from((loc.x, loc.y - bar_h)).to_f64(), 1.0, 1.0, 1.0),
        (None, None) => return,
    };
    let plain = sx == 1.0 && sy == 1.0 && alpha == 1.0;
    if anim.is_none() {
        // As before 11a: skip windows outside this output.
        let Some(bbox) = state.space.element_bbox(window) else { return };
        let bar = Rectangle::new(frame_loc.to_i32_round(), (geo.size.w, bar_h).into());
        if !region.overlaps(bbox) && !(decorated && region.overlaps(bar)) {
            return;
        }
    } else {
        let r = Rectangle::new(frame_loc, (geo.size.w as f64 * sx, frame_h * sy).into());
        if !region.to_f64().overlaps(r) {
            return;
        }
    }
    // The frame's corner on this output; the window is laid out from it at full size, then
    // (animating) scaled around it.
    let origin = (frame_loc - region.loc.to_f64()).to_physical(1.0).to_i32_round();
    let at = origin + Point::<i32, Logical>::from((-geo.loc.x, bar_h - geo.loc.y)).to_physical(1);
    if plain {
        out.extend(window.render_elements::<KompactElement>(renderer, at, 1.0.into(), 1.0));
    } else {
        let els: Vec<WaylandSurfaceRenderElement<GlesRenderer>> = window.render_elements(renderer, at, 1.0.into(), alpha);
        let scale = smithay::utils::Scale { x: sx, y: sy };
        out.extend(els.into_iter().map(|e| KompactElement::Scaled(RescaleRenderElement::from_element(e, origin, scale))));
    }
    if !decorated || geo.size.w <= 0 {
        return;
    }
    {
        let bar = Rectangle::<i32, Logical>::new(frame_loc.to_i32_round(), (geo.size.w, HEIGHT).into());
        window.user_data().insert_if_missing(|| BarImage(Default::default()));
        let mut image = window.user_data().get::<BarImage>().unwrap().0.borrow_mut();
        let key: BarKey = (
            bar.size.w,
            focused == Some(window),
            state.bar_input.buttons.as_ref() == Some(window),
            crate::shell::window_title(window),
            crate::titlebar::style(),
        );
        if image.as_ref().map(|(k, _)| k) != Some(&key) {
            let text = key.3.as_deref().zip(crate::title_text::room(key.0)).and_then(|(t, room)| crate::title_text::layout(t, room));
            let pixels = crate::titlebar::pixels_styled(key.0, key.1, key.2, text.as_ref(), key.4);
            let buffer = MemoryRenderBuffer::from_slice(&pixels, Fourcc::Argb8888, (bar.size.w, HEIGHT), 1, Transform::Normal, None);
            *image = Some((key, buffer));
        }
        let (_, buffer) = image.as_ref().unwrap();
        let at = (frame_loc - region.loc.to_f64()).to_physical(1.0);
        let (size, alpha) = if plain {
            (None, None)
        } else {
            let w = (geo.size.w as f64 * sx).round().max(1.0) as i32;
            let h = (HEIGHT as f64 * sy).round().max(1.0) as i32;
            (Some(Size::<i32, Logical>::from((w, h))), Some(alpha))
        };
        match MemoryRenderBufferRenderElement::from_buffer(renderer, at, buffer, alpha, None, size, Kind::Unspecified) {
            Ok(e) => out.push(e.into()),
            Err(e) => tracing::warn!("title bar render element failed: {e}"),
        }
    }
}

pub fn build_elements(
    renderer: &mut GlesRenderer,
    state: &Kompact,
    output: &Output,
) -> Vec<KompactElement> {
    build_elements_with(renderer, state, output, true)
}

/// `build_elements`, optionally without the cursor (9a: screenshots leave it out).
pub fn build_elements_with(
    renderer: &mut GlesRenderer,
    state: &Kompact,
    output: &Output,
    cursor: bool,
) -> Vec<KompactElement> {
    // Phase 3c: time this pass for the debug overlay (only while it's on).
    let pass_start = state.overlay.enabled.then(std::time::Instant::now);
    let mut out: Vec<KompactElement> = Vec::new();
    // Element positions are relative to this output; the pointer and areas are global (Phase 5:
    // outputs other than the first don't sit at 0,0).
    let origin = state
        .space
        .output_geometry(output)
        .map(|g| g.loc)
        .unwrap_or_default();

    if let Some(pointer) = state.seat.get_pointer().filter(|_| cursor) {
        let pos = pointer.current_location() - origin.to_f64();
        match state.cursor_draw() {
            CursorDraw::Hidden => {}
            CursorDraw::Builtin(shape) => {
                // Draw so the image's hotspot (arrow tip, resize-arrow centre) sits on the pointer.
                let (buffer, hotspot) = state.cursors.get(shape);
                let loc = (pos - hotspot).to_physical(1.0);
                match MemoryRenderBufferRenderElement::from_buffer(
                    renderer,
                    loc,
                    buffer,
                    None,
                    None,
                    None,
                    Kind::Cursor,
                ) {
                    Ok(e) => out.push(e.into()),
                    Err(e) => tracing::warn!("cursor render element failed: {e}"),
                }
            }
            CursorDraw::Surface(surface) => {
                // The client's own cursor image (e.g. Firefox's hand over links), placed by the
                // hotspot it gave in wl_pointer.set_cursor. Goes in as a plain surface element
                // (the `Layer` variant has the same element type).
                let hotspot = with_states(&surface, |s| {
                    s.data_map
                        .get::<CursorImageSurfaceData>()
                        .map(|d| d.lock().unwrap().hotspot)
                        .unwrap_or_default()
                });
                let loc = (pos - hotspot.to_f64()).to_physical(1.0).to_i32_round();
                out.extend(render_elements_from_surface_tree::<_, KompactElement>(
                    renderer,
                    &surface,
                    loc,
                    1.0,
                    1.0,
                    Kind::Cursor,
                ));
            }
        }
    }

    // 8d: locked: the cursor and this output's lock surface, nothing else (black until the
    // lock client has drawn).
    if state.lock.is_locked() {
        if let Some(lock) = state.lock.surface_on(output) {
            out.extend(render_elements_from_surface_tree::<_, KompactElement>(
                renderer,
                lock.wl_surface(),
                (0, 0),
                1.0,
                1.0,
                Kind::Unspecified,
            ));
        }
        return out;
    }

    // Debug overlay (Phase 3c): below the cursor, above everything else. Top-right of the first
    // output's usable area, so it doesn't cover a top panel.
    if state.overlay.enabled && state.outputs.first() == Some(output) {
        let area = state.usable_area_of(Some(output));
        let x = area.loc.x - origin.x + area.size.w - crate::overlay::WIDTH as i32 - 8;
        let y = area.loc.y - origin.y + 8;
        match MemoryRenderBufferRenderElement::from_buffer(
            renderer,
            smithay::utils::Point::<f64, smithay::utils::Logical>::from((x as f64, y as f64)).to_physical(1.0),
            &state.overlay.buffer,
            None,
            None,
            None,
            Kind::Unspecified,
        ) {
            Ok(e) => out.push(e.into()),
            Err(e) => tracing::warn!("overlay render element failed: {e}"),
        }
    }

    out.extend(layer_elements(renderer, output, WlrLayer::Overlay));
    out.extend(layer_elements(renderer, output, WlrLayer::Top));

    if layers_once() {
        // 6m-e2a: just the windows (with their popups), top first, from the space. Smithay's
        // `space_render_elements` also returns every layer surface, which we add ourselves
        // around this, so each panel, dock, overlay and wallpaper was drawn twice a frame.
        if let Some(region) = state.space.output_geometry(output) {
            if crate::titlebar::enabled() {
                windows_with_bars(renderer, state, &region, &mut out);
            } else {
                out.extend(
                    state
                        .space
                        .render_elements_for_region(renderer, &region, 1.0, 1.0)
                        .into_iter()
                        .map(KompactElement::Layer),
                );
            }
        }
    } else if let Ok(els) = space_render_elements(renderer, [&state.space], output, 1.0) {
        out.extend(els.into_iter().map(Into::into));
    }

    out.extend(layer_elements(renderer, output, WlrLayer::Bottom));
    out.extend(layer_elements(renderer, output, WlrLayer::Background));

    if let Some(start) = pass_start {
        state.overlay.frames.borrow_mut().record(start, start.elapsed());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layers_once_by_default() {
        assert!(layers_once_from_env(None));
        assert!(layers_once_from_env(Some("on")));
        assert!(!layers_once_from_env(Some("off")));
        assert!(!layers_once_from_env(Some(" No ")));
    }
}
