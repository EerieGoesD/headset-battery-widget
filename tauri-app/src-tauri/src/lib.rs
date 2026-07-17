mod hid;

use tauri::menu::{ContextMenu, Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{Emitter, LogicalSize, Manager};
use tauri_plugin_autostart::ManagerExt as AutostartManagerExt;
use tauri_plugin_notification::NotificationExt;
use tauri_plugin_opener::OpenerExt;

// ---- Commands ----

#[tauri::command]
async fn read_battery() -> hid::BatteryReading {
    tauri::async_runtime::spawn_blocking(hid::read_battery)
        .await
        .unwrap_or_else(|_| hid::BatteryReading {
            success: false,
            percent: 0,
            message: "Read task failed.".into(),
            label: String::new(),
        })
}

#[tauri::command]
async fn list_devices() -> Vec<hid::DeviceRow> {
    tauri::async_runtime::spawn_blocking(hid::list_devices)
        .await
        .unwrap_or_default()
}

#[tauri::command]
fn notify(app: tauri::AppHandle, title: String, body: String) -> Result<(), String> {
    app.notification()
        .builder()
        .title(title)
        .body(body)
        .show()
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn get_autostart(app: tauri::AppHandle) -> bool {
    app.autolaunch().is_enabled().unwrap_or(false)
}

#[tauri::command]
fn set_autostart(app: tauri::AppHandle, enabled: bool) -> Result<(), String> {
    let manager = app.autolaunch();
    let result = if enabled {
        manager.enable()
    } else {
        manager.disable()
    };
    result.map_err(|e| e.to_string())
}

#[tauri::command]
fn open_external(app: tauri::AppHandle, url: String) -> Result<(), String> {
    app.opener()
        .open_url(url, None::<String>)
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn set_window_size(app: tauri::AppHandle, width: f64, height: f64) -> Result<(), String> {
    if let Some(window) = app.get_webview_window("main") {
        window
            .set_size(LogicalSize::new(width, height))
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}

#[tauri::command]
fn quit(app: tauri::AppHandle) {
    app.exit(0);
}

#[tauri::command]
fn show_context_menu(app: tauri::AppHandle) -> Result<(), String> {
    let menu = app.state::<Menu<tauri::Wry>>();
    if let Some(window) = app.get_webview_window("main") {
        let webview: &tauri::Webview = window.as_ref();
        menu.popup(webview.window()).map_err(|e| e.to_string())?;
    }
    Ok(())
}

// ---- Helpers ----

fn toggle_main(app: &tauri::AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        if window.is_visible().unwrap_or(true) {
            let _ = window.hide();
        } else {
            let _ = window.show();
            let _ = window.set_focus();
        }
    }
}

// ---- App entry ----

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let mut builder = tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_notification::init());

    #[cfg(desktop)]
    {
        builder = builder
            .plugin(
                tauri_plugin_window_state::Builder::default()
                    .with_state_flags(tauri_plugin_window_state::StateFlags::POSITION)
                    .build(),
            )
            .plugin(tauri_plugin_autostart::init(
                tauri_plugin_autostart::MacosLauncher::LaunchAgent,
                None::<Vec<&str>>,
            ));
    }

    builder
        .setup(|app| {
            let show_hide =
                MenuItem::with_id(app, "show_hide", "Show/Hide", true, None::<&str>)?;
            let refresh = MenuItem::with_id(app, "refresh", "Refresh now", true, None::<&str>)?;
            let diagnostics =
                MenuItem::with_id(app, "diagnostics", "Diagnostics", true, None::<&str>)?;
            let separator = PredefinedMenuItem::separator(app)?;
            let quit = MenuItem::with_id(app, "quit", "Exit", true, None::<&str>)?;
            let menu = Menu::with_items(
                app,
                &[&show_hide, &refresh, &diagnostics, &separator, &quit],
            )?;

            let mut tray_builder = TrayIconBuilder::with_id("main-tray")
                .tooltip("Headset Battery Widget")
                .menu(&menu)
                .show_menu_on_left_click(false);

            if let Some(icon) = app.default_window_icon() {
                tray_builder = tray_builder.icon(icon.clone());
            }

            tray_builder
                .on_menu_event(|app, event| match event.id.as_ref() {
                    "show_hide" => toggle_main(app),
                    "refresh" => {
                        if let Some(window) = app.get_webview_window("main") {
                            let _ = window.emit("refresh-now", ());
                        }
                    }
                    "diagnostics" => {
                        if let Some(window) = app.get_webview_window("main") {
                            let _ = window.show();
                            let _ = window.set_focus();
                            let _ = window.emit("open-diagnostics", ());
                        }
                    }
                    "quit" => app.exit(0),
                    _ => {}
                })
                .on_tray_icon_event(|tray, event| {
                    if let TrayIconEvent::Click {
                        button: MouseButton::Left,
                        button_state: MouseButtonState::Up,
                        ..
                    } = event
                    {
                        toggle_main(tray.app_handle());
                    }
                })
                .build(app)?;

            // Right-click context menu for the widget (events go to the tray handler above).
            let ctx_refresh =
                MenuItem::with_id(app, "refresh", "Refresh now", true, None::<&str>)?;
            let ctx_diagnostics =
                MenuItem::with_id(app, "diagnostics", "Diagnostics", true, None::<&str>)?;
            let ctx_separator = PredefinedMenuItem::separator(app)?;
            let ctx_quit = MenuItem::with_id(app, "quit", "Exit", true, None::<&str>)?;
            let context_menu = Menu::with_items(
                app,
                &[&ctx_refresh, &ctx_diagnostics, &ctx_separator, &ctx_quit],
            )?;
            app.manage(context_menu);

            Ok(())
        })
        .on_window_event(|window, event| {
            // Close (e.g. Alt+F4) hides to tray instead of quitting; exit is via the tray.
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                let _ = window.hide();
                api.prevent_close();
            }
        })
        .invoke_handler(tauri::generate_handler![
            read_battery,
            list_devices,
            notify,
            get_autostart,
            set_autostart,
            open_external,
            set_window_size,
            quit,
            show_context_menu
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
