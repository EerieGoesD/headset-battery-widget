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

// ---- Licensing: 7-day trial + one-time lifetime unlock ----
//
// Two App Store in-app purchases:
//   - TRIAL_ID  : free ($0) non-consumable. "Buying" it starts the 7-day trial,
//                 tied to the Apple ID, so reinstalling does not reset it.
//   - UNLOCK_ID : the paid one-time unlock.
// On macOS the source of truth is StoreKit; license.json is just a local cache.

#[allow(dead_code)]
const UNLOCK_ID: &str = "com.eeriegoesd.headset-battery-widget.unlock";
#[allow(dead_code)]
const TRIAL_ID: &str = "com.eeriegoesd.headset-battery-widget.trial";

fn data_dir(app: &tauri::AppHandle) -> std::path::PathBuf {
    let dir = app
        .path()
        .app_local_data_dir()
        .expect("could not resolve app data dir");
    std::fs::create_dir_all(&dir).ok();
    dir
}

fn license_path(app: &tauri::AppHandle) -> std::path::PathBuf {
    data_dir(app).join("license.json")
}

// Returns (owned, trial_start_ms). trial_start_ms is 0 when not started.
fn read_license(app: &tauri::AppHandle) -> (bool, i64) {
    if let Ok(s) = std::fs::read_to_string(license_path(app)) {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&s) {
            let owned = v.get("owned").and_then(|x| x.as_bool()).unwrap_or(false);
            let trial = v.get("trialStartMs").and_then(|x| x.as_i64()).unwrap_or(0);
            return (owned, trial);
        }
    }
    (false, 0)
}

#[allow(dead_code)]
fn write_license(app: &tauri::AppHandle, owned: bool, trial_start_ms: i64) {
    let _ = std::fs::write(
        license_path(app),
        format!("{{\"owned\":{},\"trialStartMs\":{}}}", owned, trial_start_ms),
    );
}

#[allow(dead_code)]
fn now_ms() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

// owned = paid unlock; trialStartMs = when the trial began (0 if not yet).
// On macOS this refreshes from StoreKit (source of truth) and caches it.
#[tauri::command]
async fn iap_status(app: tauri::AppHandle) -> String {
    #[cfg(target_os = "macos")]
    {
        use tauri_plugin_iap::IapExt;
        let (mut owned, mut trial) = read_license(&app);
        if let Ok(st) = app
            .iap()
            .get_product_status(UNLOCK_ID.to_string(), "inapp".to_string())
            .await
        {
            owned = st.is_owned;
        }
        if let Ok(st) = app
            .iap()
            .get_product_status(TRIAL_ID.to_string(), "inapp".to_string())
            .await
        {
            if st.is_owned {
                if let Some(pt) = st.purchase_time {
                    trial = pt;
                }
            }
        }
        write_license(&app, owned, trial);
        return format!("{{\"owned\":{},\"trialStartMs\":{}}}", owned, trial);
    }
    #[cfg(not(target_os = "macos"))]
    {
        let (owned, trial) = read_license(&app);
        format!("{{\"owned\":{},\"trialStartMs\":{}}}", owned, trial)
    }
}

// Localized App Store price of the unlock (e.g. "4,99 EUR"). Empty if unknown.
// Never hardcode a price; the store returns the right currency/amount per region.
#[tauri::command]
async fn iap_price(app: tauri::AppHandle) -> String {
    #[cfg(target_os = "macos")]
    {
        use tauri_plugin_iap::IapExt;
        if let Ok(resp) = app
            .iap()
            .get_products(vec![UNLOCK_ID.to_string()], "inapp".to_string())
            .await
        {
            if let Some(p) = resp.products.first() {
                return p.formatted_price.clone().unwrap_or_default();
            }
        }
        return String::new();
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = &app;
        String::new()
    }
}

// Starts the free trial by "buying" the $0 TRIAL_ID. Returns the trial start (ms).
#[tauri::command]
async fn iap_start_trial(app: tauri::AppHandle) -> Result<i64, String> {
    #[cfg(target_os = "macos")]
    {
        use tauri_plugin_iap::{IapExt, PurchaseRequest};
        app.iap()
            .purchase(PurchaseRequest {
                product_id: TRIAL_ID.to_string(),
                product_type: "inapp".to_string(),
                options: None,
            })
            .await
            .map_err(|e| e.to_string())?;
        let mut start = now_ms();
        if let Ok(st) = app
            .iap()
            .get_product_status(TRIAL_ID.to_string(), "inapp".to_string())
            .await
        {
            if let Some(pt) = st.purchase_time {
                start = pt;
            }
        }
        let owned = read_license(&app).0;
        write_license(&app, owned, start);
        return Ok(start);
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = &app;
        Err("Purchases are only available in the Mac App Store build.".into())
    }
}

// Buys the paid one-time unlock (UNLOCK_ID). Returns true if owned afterward.
#[tauri::command]
async fn iap_buy(app: tauri::AppHandle) -> Result<bool, String> {
    #[cfg(target_os = "macos")]
    {
        use tauri_plugin_iap::{IapExt, PurchaseRequest};
        app.iap()
            .purchase(PurchaseRequest {
                product_id: UNLOCK_ID.to_string(),
                product_type: "inapp".to_string(),
                options: None,
            })
            .await
            .map_err(|e| e.to_string())?;
        let mut owned = false;
        if let Ok(st) = app
            .iap()
            .get_product_status(UNLOCK_ID.to_string(), "inapp".to_string())
            .await
        {
            owned = st.is_owned;
        }
        if owned {
            let trial = read_license(&app).1;
            write_license(&app, true, trial);
        }
        return Ok(owned);
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = &app;
        Err("Purchases are only available in the Mac App Store build.".into())
    }
}

// Restores previous purchases (unlock and/or trial) for this Apple ID.
#[tauri::command]
async fn iap_restore(app: tauri::AppHandle) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        use tauri_plugin_iap::IapExt;
        app.iap()
            .restore_purchases("inapp".to_string())
            .await
            .map_err(|e| e.to_string())?;
        let mut owned = false;
        if let Ok(st) = app
            .iap()
            .get_product_status(UNLOCK_ID.to_string(), "inapp".to_string())
            .await
        {
            owned = st.is_owned;
        }
        let mut trial = 0i64;
        if let Ok(st) = app
            .iap()
            .get_product_status(TRIAL_ID.to_string(), "inapp".to_string())
            .await
        {
            if st.is_owned {
                if let Some(pt) = st.purchase_time {
                    trial = pt;
                }
            }
        }
        write_license(&app, owned, trial);
        return Ok(());
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = &app;
        Err("Purchases are only available in the Mac App Store build.".into())
    }
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

    #[cfg(target_os = "macos")]
    {
        builder = builder.plugin(tauri_plugin_iap::init());
    }

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

            // Ask for notification permission up front. Without this, macOS silently
            // drops the low-battery alert (it never shows and never prompts).
            let notify_handle = app.handle().clone();
            std::thread::spawn(move || {
                let _ = notify_handle.notification().request_permission();
            });

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
            show_context_menu,
            iap_status,
            iap_price,
            iap_start_trial,
            iap_buy,
            iap_restore
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
