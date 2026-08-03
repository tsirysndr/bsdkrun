//! Native application menu — the macOS top bar (and the window menu on
//! Windows/Linux), Docker Desktop-style. Custom items emit a `menu://<id>`
//! event the webview listens for to navigate or trigger actions.

use tauri::menu::{
    AboutMetadata, Menu, MenuBuilder, MenuItemBuilder, PredefinedMenuItem, SubmenuBuilder,
};
use tauri::{AppHandle, Emitter, Wry};

/// Build + attach the app menu and route custom clicks to the frontend.
pub fn install(app: &AppHandle) -> tauri::Result<()> {
    let menu = build(app)?;
    app.set_menu(menu)?;
    app.on_menu_event(|app, event| {
        let id = event.id().0.as_str();
        // Custom items carry an `app:` prefix; forward them to the webview.
        if let Some(action) = id.strip_prefix("app:") {
            let _ = app.emit("menu://action", action.to_string());
        }
    });
    Ok(())
}

fn build(app: &AppHandle) -> tauri::Result<Menu<Wry>> {
    let pkg = app.package_info();
    let name = pkg.name.clone();

    // --- bsdkrun Desktop (app) menu -------------------------------------
    let about = AboutMetadata {
        name: Some("bsdkrun Desktop".into()),
        version: Some(pkg.version.to_string()),
        comments: Some("A Docker Desktop-style GUI for bsdkrun microVMs.".into()),
        website: Some("https://github.com/tsirysndr/bsdkrun".into()),
        website_label: Some("bsdkrun on GitHub".into()),
        ..Default::default()
    };
    let settings = MenuItemBuilder::new("Settings…")
        .id("app:settings")
        .accelerator("CmdOrCtrl+,")
        .build(app)?;
    let app_menu = SubmenuBuilder::new(app, &name)
        .about(Some(about))
        .separator()
        .item(&settings)
        .separator()
        .services()
        .separator()
        .hide()
        .hide_others()
        .show_all()
        .separator()
        .quit()
        .build()?;

    // --- Machine menu ---------------------------------------------------
    let run_new = MenuItemBuilder::new("Run New Machine…")
        .id("app:run")
        .accelerator("CmdOrCtrl+N")
        .build(app)?;
    let refresh = MenuItemBuilder::new("Refresh")
        .id("app:refresh")
        .accelerator("CmdOrCtrl+R")
        .build(app)?;
    let stop_all = MenuItemBuilder::new("Stop All Running")
        .id("app:stop-all")
        .accelerator("CmdOrCtrl+Shift+S")
        .build(app)?;
    let machine_menu = SubmenuBuilder::new(app, "Machine")
        .item(&run_new)
        .item(&refresh)
        .separator()
        .item(&stop_all)
        .build()?;

    // --- View menu (navigation) ----------------------------------------
    let nav_machines = MenuItemBuilder::new("Machines")
        .id("app:nav:machines")
        .accelerator("CmdOrCtrl+1")
        .build(app)?;
    let nav_images = MenuItemBuilder::new("Images")
        .id("app:nav:images")
        .accelerator("CmdOrCtrl+2")
        .build(app)?;
    let nav_volumes = MenuItemBuilder::new("Volumes")
        .id("app:nav:volumes")
        .accelerator("CmdOrCtrl+3")
        .build(app)?;
    let palette = MenuItemBuilder::new("Command Palette…")
        .id("app:palette")
        .accelerator("CmdOrCtrl+K")
        .build(app)?;
    let view_menu = SubmenuBuilder::new(app, "View")
        .item(&nav_machines)
        .item(&nav_images)
        .item(&nav_volumes)
        .separator()
        .item(&palette)
        .separator()
        .item(&PredefinedMenuItem::fullscreen(app, None)?)
        .build()?;

    // --- Edit + Window (predefined) ------------------------------------
    let edit_menu = SubmenuBuilder::new(app, "Edit")
        .undo()
        .redo()
        .separator()
        .cut()
        .copy()
        .paste()
        .select_all()
        .build()?;
    let window_menu = SubmenuBuilder::new(app, "Window")
        .minimize()
        .maximize()
        .separator()
        .close_window()
        .build()?;

    // --- Help -----------------------------------------------------------
    let shortcuts = MenuItemBuilder::new("Keyboard Shortcuts")
        .id("app:shortcuts")
        .accelerator("CmdOrCtrl+/")
        .build(app)?;
    let docs = MenuItemBuilder::new("bsdkrun Documentation")
        .id("app:docs")
        .build(app)?;
    let help_menu = SubmenuBuilder::new(app, "Help")
        .item(&shortcuts)
        .item(&docs)
        .build()?;

    MenuBuilder::new(app)
        .items(&[
            &app_menu,
            &edit_menu,
            &machine_menu,
            &view_menu,
            &window_menu,
            &help_menu,
        ])
        .build()
}
