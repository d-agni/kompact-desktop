//! 19h: the assistant's HUD (`hud.html`), which replaced Siva the character (19b) on 2026-10-23:
//! a transparent layer surface on the top layer, anchored top-right under the menu bar, shown
//! only while the assistant is in use. It never takes the keyboard unless you type to it
//! (`hud_keyboard`), and the pointer only on what it draws (`hud_input`).
//!
//! The menu bar's assistant item calls `assistant_toggle`: it opens the HUD listening (or
//! typing, when voice isn't set up), and while listening it means "I'm done".

use tauri::{AppHandle, Emitter, Manager};
use tracing::info;

pub const LABEL: &str = "assistant";
/// The surface: room for the ring, the wave, what was heard, the reply and its buttons.
pub const WIDTH: i32 = 460;
pub const HEIGHT: i32 = 360;
/// From the top-right corner of the area under the menu bar.
pub const MARGIN_TOP: i32 = 6;
pub const MARGIN_RIGHT: i32 = 10;

fn window(app: &AppHandle) -> Result<tauri::WebviewWindow, String> {
    app.get_webview_window(LABEL).ok_or_else(|| "no assistant window".into())
}

/// Show it and tell the page to start: `listen`, or `type` (no voice, or asked for).
pub fn open(app: &AppHandle, how: &str) -> Result<(), String> {
    let w = window(app)?;
    w.show().map_err(|e| e.to_string())?;
    info!("assistant: HUD open ({how})");
    let _ = app.emit("kompact-assistant-open", how);
    Ok(())
}

/// The menu-bar item (and, 19d-2, Ctrl+Super+V): listening → done; otherwise open and listen.
#[tauri::command]
pub fn assistant_toggle(app: AppHandle) -> Result<(), String> {
    if super::voice::listening() {
        info!("assistant: done listening (menu bar)");
        super::voice::stop();
        return Ok(());
    }
    let how = if super::voice::state().ready { "listen" } else { "type" };
    open(&app, how)
}

#[tauri::command]
pub fn hud_hide(app: AppHandle) -> Result<(), String> {
    super::voice::cancel();
    super::speech::hush();
    let w = window(&app)?;
    let _ = set_keyboard(&w, false);
    w.hide().map_err(|e| e.to_string())?;
    info!("assistant: HUD closed");
    Ok(())
}

/// The page's drawn parts (`[x, y, w, h]` in surface px): only they take the pointer.
#[tauri::command]
pub fn hud_input(app: AppHandle, rects: Vec<[f64; 4]>) -> Result<(), String> {
    use gtk::prelude::*;
    let gtk_window = window(&app)?.gtk_window().map_err(|e| e.to_string())?;
    let region = gtk::cairo::Region::create();
    for [x, y, w, h] in rects.iter().take(16) {
        let r = gtk::cairo::RectangleInt::new(x.floor() as i32, y.floor() as i32, w.ceil() as i32, h.ceil() as i32);
        region.union_rectangle(&r).map_err(|e| e.to_string())?;
    }
    gtk_window.input_shape_combine_region(Some(&region));
    Ok(())
}

fn set_keyboard(window: &tauri::WebviewWindow, on: bool) -> Result<(), String> {
    use gtk_layer_shell::{KeyboardMode, LayerShell};
    let gtk_window = window.gtk_window().map_err(|e| e.to_string())?;
    gtk_window.set_keyboard_mode(if on { KeyboardMode::Exclusive } else { KeyboardMode::None });
    if on {
        let _ = window.set_focus();
    }
    Ok(())
}

/// The keyboard while you type to it (Exclusive, as the overlay does), and back to none after,
/// so the window that had it gets it again.
#[tauri::command]
pub fn hud_keyboard(app: AppHandle, on: bool) -> Result<(), String> {
    set_keyboard(&window(&app)?, on)
}
