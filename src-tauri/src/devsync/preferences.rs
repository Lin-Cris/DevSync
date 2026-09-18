use tauri::AppHandle;
use tauri_plugin_autostart::ManagerExt;

use super::DevSyncError;

#[tauri::command]
pub async fn get_devsync_launch_at_login(app: AppHandle) -> Result<bool, DevSyncError> {
    app.autolaunch().is_enabled().map_err(|error| {
        DevSyncError::new(
            "autostart_error",
            format!("Unable to read Launch at Login: {error}"),
        )
    })
}

#[tauri::command]
pub async fn set_devsync_launch_at_login(
    app: AppHandle,
    enabled: bool,
) -> Result<bool, DevSyncError> {
    let manager = app.autolaunch();
    if enabled {
        manager.enable()
    } else {
        manager.disable()
    }
    .map_err(|error| {
        DevSyncError::new(
            "autostart_error",
            format!("Unable to update Launch at Login: {error}"),
        )
    })?;
    manager.is_enabled().map_err(|error| {
        DevSyncError::new(
            "autostart_error",
            format!("Unable to verify Launch at Login: {error}"),
        )
    })
}
