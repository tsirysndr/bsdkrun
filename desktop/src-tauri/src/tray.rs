//! macOS-style system tray (menu-bar), Docker-Desktop-like: a monochrome
//! template icon, an engine-status line (green/red dot), and quick actions.
//! Closing the window hides it to the tray instead of quitting.

use std::sync::Mutex;

use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Emitter, Manager, Wry};

/// Handle to the tray's status line so the frontend can update it live.
#[derive(Default)]
pub struct TrayStatus(pub Mutex<Option<MenuItem<Wry>>>);

pub fn install(app: &AppHandle) -> tauri::Result<()> {
    let status = MenuItem::with_id(app, "tray:status", "Checking engine…", false, None::<&str>)?;
    let open = MenuItem::with_id(app, "tray:show", "Open Dashboard", true, None::<&str>)?;
    let run = MenuItem::with_id(app, "tray:run", "Run a machine…", true, None::<&str>)?;
    let settings = MenuItem::with_id(app, "tray:settings", "Settings…", true, None::<&str>)?;
    let docs = MenuItem::with_id(app, "tray:docs", "Documentation", true, None::<&str>)?;
    let quit = PredefinedMenuItem::quit(app, Some("Quit bsdkrun Desktop"))?;

    let menu = Menu::with_items(
        app,
        &[
            &status,
            &PredefinedMenuItem::separator(app)?,
            &open,
            &run,
            &settings,
            &docs,
            &PredefinedMenuItem::separator(app)?,
            &quit,
        ],
    )?;

    *app.state::<TrayStatus>().0.lock().unwrap() = Some(status);

    TrayIconBuilder::with_id("main")
        .icon(tauri::include_image!("icons/tray.png"))
        .icon_as_template(true) // adapts to the light/dark menu bar
        .tooltip("bsdkrun Desktop")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id().as_ref() {
            "tray:show" => show_main(app),
            "tray:run" => {
                show_main(app);
                let _ = app.emit("menu://action", "run".to_string());
            }
            "tray:settings" => {
                show_main(app);
                let _ = app.emit("menu://action", "settings".to_string());
            }
            "tray:docs" => {
                let _ = app.emit("menu://action", "docs".to_string());
            }
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                show_main(tray.app_handle());
            }
        })
        .build(app)?;
    Ok(())
}

/// Update the tray status line from the frontend's probe result.
pub fn set_status(app: &AppHandle, ok: bool, detail: &str) {
    if let Some(item) = app.state::<TrayStatus>().0.lock().unwrap().as_ref() {
        let text = if ok {
            "🟢 Engine running".to_string()
        } else if detail.is_empty() {
            "🔴 Engine unavailable".to_string()
        } else {
            format!("🔴 {detail}")
        };
        let _ = item.set_text(text);
    }
}

/// Reveal + focus the main window (from the tray icon or its menu).
fn show_main(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.show();
        let _ = w.unminimize();
        let _ = w.set_focus();
    }
}
