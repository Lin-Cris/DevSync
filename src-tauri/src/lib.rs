#[macro_use]
mod account;
#[macro_use]
mod device;
#[macro_use]
mod sideload;
#[macro_use]
mod pairing;
#[macro_use]
mod secure_storage;
mod devsync;
mod error;
mod logging;
mod operation;

use crate::{
    account::{
        delete_account, delete_app_id, get_certificates, invalidate_account, list_app_ids,
        logged_in_as, login_new, login_stored, reset_anisette_state, revoke_certificate,
    },
    device::{
        DeviceInfoMutex, PairingCancelToken, cancel_pairing, list_devices, set_selected_device,
    },
    devsync::{
        add_devsync_workspace, build_devsync_workspace, clean_devsync_build_cache,
        clear_devsync_activity, deploy_devsync_workspace, disable_background_service,
        enable_background_service, get_background_service_status, get_devsync_active_workspace,
        get_devsync_build_cache_usage, get_devsync_device_selection, get_devsync_launch_at_login,
        list_devsync_devices, list_devsync_workspaces, reconcile_refresh_scheduler,
        refresh_devsync_workspace, remove_devsync_workspace, select_devsync_container,
        select_devsync_device, select_devsync_scheme, select_devsync_workspace,
        set_devsync_auto_sync, set_devsync_launch_at_login, set_devsync_pre_build_command,
    },
    pairing::{
        delete_stored_rppairing, export_pairing_cmd, has_stored_rppairing, installed_pairing_apps,
        place_pairing_cmd,
    },
    secure_storage::{force_disable_keyring, keyring_available},
    sideload::{SideloaderMutex, install_sidestore_operation, sideload_operation},
};
use tauri::{
    Manager, WindowEvent,
    menu::{Menu, MenuItem},
    tray::TrayIconBuilder,
};
use tracing_subscriber::{Layer, Registry, fmt, layer::SubscriberExt, util::SubscriberInitExt};

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_process::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_store::Builder::new().build())
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            None,
        ))
        .setup(|app| {
            setup_menu_bar(app)?;
            let log_dir = app
                .path()
                .app_data_dir()
                .expect("failed to get app data dir")
                .join("logs");

            std::fs::create_dir_all(&log_dir).ok();

            let file_appender = tracing_appender::rolling::RollingFileAppender::builder()
                .rotation(tracing_appender::rolling::Rotation::DAILY)
                .filename_prefix("devsync")
                .filename_suffix("log")
                .max_log_files(2)
                .build(&log_dir)
                .expect("failed to create log file appender");

            let file_layer = fmt::layer()
                .with_writer(file_appender)
                .with_target(true)
                .with_ansi(false)
                .with_filter(tracing_subscriber::filter::LevelFilter::DEBUG);

            let frontend_layer = logging::FrontendLoggingLayer::new(app.handle().clone())
                .with_filter(tracing_subscriber::filter::LevelFilter::DEBUG);

            Registry::default()
                .with(file_layer)
                .with(frontend_layer)
                .init();

            std::panic::set_hook(Box::new(|panic_info| {
                let thread = std::thread::current();
                let thread_name = thread.name().unwrap_or("<unnamed>");

                let message = if let Some(s) = panic_info.payload().downcast_ref::<&str>() {
                    s.to_string()
                } else if let Some(s) = panic_info.payload().downcast_ref::<String>() {
                    s.clone()
                } else {
                    "<non-string panic payload>".to_string()
                };

                let location = panic_info
                    .location()
                    .map(|loc| format!("{}:{}", loc.file(), loc.line()))
                    .unwrap_or_else(|| "<unknown>".to_string());

                let backtrace = std::backtrace::Backtrace::capture();

                tracing::error!(
                    target: "panic",
                    thread = thread_name,
                    location = location,
                    message = message,
                    backtrace = %backtrace,
                    "panic captured"
                );
            }));

            app.manage(DeviceInfoMutex::new(None));
            app.manage(SideloaderMutex::new(None));
            app.manage(PairingCancelToken::new(None));
            Ok(())
        })
        .on_window_event(|window, event| {
            if let WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                let _ = window.hide();
            }
        })
        .invoke_handler(tauri::generate_handler![
            login_new,
            invalidate_account,
            logged_in_as,
            login_stored,
            delete_account,
            list_devices,
            sideload_operation,
            set_selected_device,
            install_sidestore_operation,
            get_certificates,
            revoke_certificate,
            list_app_ids,
            delete_app_id,
            installed_pairing_apps,
            place_pairing_cmd,
            reset_anisette_state,
            export_pairing_cmd,
            delete_stored_rppairing,
            keyring_available,
            force_disable_keyring,
            cancel_pairing,
            has_stored_rppairing,
            list_devsync_workspaces,
            get_devsync_active_workspace,
            select_devsync_workspace,
            add_devsync_workspace,
            refresh_devsync_workspace,
            remove_devsync_workspace,
            select_devsync_container,
            select_devsync_scheme,
            set_devsync_auto_sync,
            set_devsync_pre_build_command,
            get_background_service_status,
            enable_background_service,
            disable_background_service,
            build_devsync_workspace,
            list_devsync_devices,
            get_devsync_device_selection,
            get_devsync_launch_at_login,
            set_devsync_launch_at_login,
            select_devsync_device,
            deploy_devsync_workspace,
            clean_devsync_build_cache,
            get_devsync_build_cache_usage,
            clear_devsync_activity,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

pub fn run_agent() -> Result<(), String> {
    devsync::run_agent().map_err(|error| error.to_string())
}

fn setup_menu_bar(app: &tauri::App) -> tauri::Result<()> {
    let open = MenuItem::with_id(app, "open", "Open DevSync", true, None::<&str>)?;
    let sync_all = MenuItem::with_id(app, "sync-all", "Sync Active Project", true, None::<&str>)?;
    let status = MenuItem::with_id(
        app,
        "status",
        "Device status updates every 30 minutes",
        false,
        None::<&str>,
    )?;
    let quit = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&status, &sync_all, &open, &quit])?;
    TrayIconBuilder::with_id("devsync-tray")
        .tooltip("DevSync")
        .menu(&menu)
        .show_menu_on_left_click(true)
        .on_menu_event(|app, event| match event.id.as_ref() {
            "open" => show_main_window(app),
            "sync-all" => {
                let handle = app.clone();
                tauri::async_runtime::spawn(async move {
                    let _ = reconcile_refresh_scheduler(&handle).await;
                });
                show_main_window(app);
            }
            "quit" => app.exit(0),
            _ => {}
        })
        .build(app)?;
    Ok(())
}

fn show_main_window(app: &tauri::AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.set_focus();
    }
}
