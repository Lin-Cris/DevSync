use std::{
    fs,
    time::{SystemTime, UNIX_EPOCH},
};

use serde_json::Value;
use tauri::AppHandle;
use tokio::process::Command;

use super::{
    DevSyncError,
    locking::ProcessLock,
    models::{DevSyncDeviceInfo, DeviceSelection},
    paths::DevSyncPaths,
    workspace,
};

const STORE_KEY: &str = "deviceSelection";

#[tauri::command]
pub async fn list_devsync_devices() -> Result<Vec<DevSyncDeviceInfo>, DevSyncError> {
    let mut devices = list_devices().await?;
    resolve_device_reachability(&mut devices).await;
    Ok(devices)
}

pub async fn list_devices() -> Result<Vec<DevSyncDeviceInfo>, DevSyncError> {
    let json_path = temp_json_path("devices");
    let output_path = json_path.to_string_lossy().to_string();
    let output = Command::new("xcrun")
        .args([
            "devicectl",
            "list",
            "devices",
            "--timeout",
            "10",
            "--json-output",
            output_path.as_str(),
        ])
        .output()
        .await
        .map_err(|error| {
            DevSyncError::new(
                "devicectl_unavailable",
                format!("Unable to launch devicectl: {error}"),
            )
        })?;
    let json = fs::read_to_string(&json_path).ok();
    let _ = fs::remove_file(&json_path);
    if !output.status.success() {
        let details = json
            .as_deref()
            .and_then(devicectl_details)
            .unwrap_or_else(|| String::from_utf8_lossy(&output.stderr).trim().to_string());
        return Err(DevSyncError::new(
            "device_discovery_failed",
            if details.is_empty() {
                "devicectl could not list paired devices.".into()
            } else {
                details
            },
        ));
    }
    let json = json.ok_or_else(|| {
        DevSyncError::new(
            "malformed_devicectl_json",
            "devicectl did not produce its requested JSON output.",
        )
    })?;
    let value: Value = serde_json::from_str(&json).map_err(|error| {
        DevSyncError::new(
            "malformed_devicectl_json",
            format!("devicectl returned invalid JSON: {error}"),
        )
    })?;
    Ok(parse_devices(&value))
}

#[tauri::command]
pub async fn get_devsync_device_selection(app: AppHandle) -> Result<DeviceSelection, DevSyncError> {
    load_selection(&app)
}

#[tauri::command]
pub async fn select_devsync_device(
    app: AppHandle,
    device: DevSyncDeviceInfo,
) -> Result<DeviceSelection, DevSyncError> {
    let selection = remember_device(&app, &device)?;
    Ok(selection)
}

pub fn remember_device(
    app: &AppHandle,
    device: &DevSyncDeviceInfo,
) -> Result<DeviceSelection, DevSyncError> {
    remember_device_at(&DevSyncPaths::from_app(app)?, device)
}

pub fn remember_device_at(
    paths: &DevSyncPaths,
    device: &DevSyncDeviceInfo,
) -> Result<DeviceSelection, DevSyncError> {
    let selection = DeviceSelection {
        schema_version: 1,
        selected_device_id: Some(device.id.clone()),
        selected_device_name: Some(device.name.clone()),
        updated_at: Some(workspace::now()),
    };
    save_selection_at(paths, &selection)?;
    Ok(selection)
}

pub fn load_selection(app: &AppHandle) -> Result<DeviceSelection, DevSyncError> {
    load_selection_at(&DevSyncPaths::from_app(app)?)
}

pub fn load_selection_at(paths: &DevSyncPaths) -> Result<DeviceSelection, DevSyncError> {
    let path = paths.device_store_path();
    if !path.is_file() {
        return Ok(DeviceSelection {
            schema_version: 1,
            ..Default::default()
        });
    }
    let contents = fs::read_to_string(&path).map_err(|error| {
        DevSyncError::new(
            "storage_error",
            format!("Unable to read device storage: {error}"),
        )
    })?;
    let document: Value = serde_json::from_str(&contents).map_err(|error| {
        DevSyncError::new(
            "storage_error",
            format!("Saved device data is invalid: {error}"),
        )
    })?;
    let mut selection: DeviceSelection = serde_json::from_value(
        document.get(STORE_KEY).cloned().unwrap_or(document),
    )
    .map_err(|error| {
        DevSyncError::new(
            "storage_error",
            format!("Saved device data is invalid: {error}"),
        )
    })?;
    if selection.schema_version == 0 {
        selection.schema_version = 1;
    }
    Ok(selection)
}

pub fn save_selection_at(
    paths: &DevSyncPaths,
    selection: &DeviceSelection,
) -> Result<(), DevSyncError> {
    let _lock = ProcessLock::try_acquire(&paths.device_store_lock_path())?.ok_or_else(|| {
        DevSyncError::new(
            "storage_busy",
            "Device storage is currently being updated by another DevSync process.",
        )
    })?;
    fs::create_dir_all(&paths.app_data_dir).map_err(|error| {
        DevSyncError::new(
            "storage_error",
            format!("Unable to create device storage directory: {error}"),
        )
    })?;
    let document = serde_json::json!({
        STORE_KEY: serde_json::to_value(selection).map_err(|error| {
            DevSyncError::new(
                "storage_error",
                format!("Unable to serialize device selection: {error}"),
            )
        })?,
    });
    let temp_path = paths
        .device_store_path()
        .with_extension(format!("json.{}.tmp", std::process::id()));
    fs::write(
        &temp_path,
        serde_json::to_string_pretty(&document).map_err(|error| {
            DevSyncError::new(
                "storage_error",
                format!("Unable to serialize device selection: {error}"),
            )
        })?,
    )
    .map_err(|error| {
        DevSyncError::new(
            "storage_error",
            format!("Unable to write device storage: {error}"),
        )
    })?;
    fs::rename(&temp_path, paths.device_store_path()).map_err(|error| {
        DevSyncError::new(
            "storage_error",
            format!("Unable to commit device storage: {error}"),
        )
    })
}

pub fn parse_devices(value: &Value) -> Vec<DevSyncDeviceInfo> {
    let mut devices = Vec::new();
    find_device_arrays(value, &mut devices);
    devices
}

fn find_device_arrays(value: &Value, devices: &mut Vec<DevSyncDeviceInfo>) {
    match value {
        Value::Object(map) => {
            if let Some(Value::Array(items)) = map.get("devices") {
                for item in items {
                    if let Some(device) = parse_device(item) {
                        devices.push(device);
                    }
                }
            }
            for (key, nested) in map {
                if key != "devices" {
                    find_device_arrays(nested, devices);
                }
            }
        }
        Value::Array(items) => {
            for item in items {
                find_device_arrays(item, devices);
            }
        }
        _ => {}
    }
}

fn parse_device(value: &Value) -> Option<DevSyncDeviceInfo> {
    let root = value.as_object()?;
    let device = root.get("deviceProperties").and_then(Value::as_object);
    let hardware = root.get("hardwareProperties").and_then(Value::as_object);
    let connection = root.get("connectionProperties").and_then(Value::as_object);
    let get = |keys: &[&str]| {
        keys.iter().find_map(|key| {
            root.get(*key)
                .and_then(Value::as_str)
                .or_else(|| device.and_then(|map| map.get(*key).and_then(Value::as_str)))
                .or_else(|| hardware.and_then(|map| map.get(*key).and_then(Value::as_str)))
                .or_else(|| connection.and_then(|map| map.get(*key).and_then(Value::as_str)))
                .map(str::to_string)
        })
    };
    let id = get(&["identifier", "udid", "uuid"])?;
    let platform = get(&["platform", "operatingSystem"]);
    let model = get(&["marketingName", "model", "productType", "deviceType"]);
    if let Some(platform) = &platform
        && !platform.to_lowercase().contains("ios")
        && !platform.to_lowercase().contains("iphone")
        && !platform.to_lowercase().contains("ipad")
    {
        return None;
    }
    let pairing_state = get(&["pairingState", "pairingStatus"]);
    // Xcode 26 reports a paired local-network phone as `tunnelState:
    // disconnected` while the CoreDevice tunnel is being established. That
    // is not evidence that the phone is unavailable, so leave it Unknown and
    // let the details probe establish reachability.
    let connection_state = get(&[
        "connectionState",
        "connectionStatus",
        "state",
        "availability",
    ]);
    let tunnel_state = get(&["tunnelState"]);
    let state = match connection_state
        .as_deref()
        .or(tunnel_state.as_deref())
        .map(str::to_lowercase)
        .as_deref()
    {
        Some("connected") | Some("available") | Some("online") => "Connected".into(),
        Some("disconnected") if pairing_state.as_deref() == Some("paired") => "Unknown".into(),
        Some("unavailable") | Some("offline") | Some("disconnected") => "Unavailable".into(),
        _ => "Unknown".into(),
    };
    Some(DevSyncDeviceInfo {
        id,
        name: get(&["name", "deviceName"]).unwrap_or_else(|| "iPhone".into()),
        model,
        os_version: get(&[
            "osVersion",
            "osVersionNumber",
            "operatingSystemVersion",
            "productVersion",
        ]),
        connection_state: state,
        connection_type: get(&["transportType", "connectionType"])
            .and_then(|value| normalize_connection_type(&value)),
        last_seen: workspace::now(),
    })
}

pub fn is_connected(device: &DevSyncDeviceInfo) -> bool {
    matches!(
        device.connection_state.trim().to_ascii_lowercase().as_str(),
        "connected" | "available" | "online"
    )
}

async fn resolve_device_reachability(devices: &mut [DevSyncDeviceInfo]) {
    for device in devices.iter_mut() {
        if is_connected(device) {
            continue;
        }
        if probe_device(device).await.is_ok() {
            device.connection_state = "Connected".into();
        } else {
            device.connection_state = "Unavailable".into();
        }
    }
}

/// A device list can contain a paired local-network device without a reliable
/// connectionState. Ask CoreDevice to resolve it before allowing deployment.
pub async fn probe_device(device: &DevSyncDeviceInfo) -> Result<(), DevSyncError> {
    if is_connected(device) {
        return Ok(());
    }
    let json_path = temp_json_path("probe");
    let json_arg = json_path.to_string_lossy().to_string();
    let output = Command::new("xcrun")
        .args([
            "devicectl",
            "device",
            "info",
            "details",
            "--device",
            device.id.as_str(),
            "--timeout",
            "10",
            "--json-output",
            json_arg.as_str(),
        ])
        .output()
        .await
        .map_err(|error| {
            DevSyncError::new(
                "devicectl_unavailable",
                format!("Unable to launch devicectl: {error}"),
            )
        })?;
    let structured = fs::read_to_string(&json_path).ok();
    let _ = fs::remove_file(&json_path);
    let stderr = String::from_utf8_lossy(&output.stderr);
    if !output.status.success()
        || structured
            .as_deref()
            .is_some_and(|json| !devicectl_succeeded(json))
    {
        let details = structured
            .as_deref()
            .and_then(devicectl_details)
            .or_else(|| {
                stderr
                    .lines()
                    .map(str::trim)
                    .find(|line| !line.is_empty())
                    .map(str::to_string)
            })
            .unwrap_or_else(|| "The selected iPhone is currently unavailable.".into());
        return Err(DevSyncError::new("device_unavailable", details));
    }
    Ok(())
}

fn normalize_connection_type(value: &str) -> Option<String> {
    match value.to_lowercase().as_str() {
        "wired" | "usb" => Some("USB".into()),
        "network" | "wifi" | "wi-fi" | "localnetwork" => Some("Network / Wi-Fi".into()),
        _ => None,
    }
}

fn devicectl_details(json: &str) -> Option<String> {
    let value: Value = serde_json::from_str(json).ok()?;
    value
        .pointer("/info/details")
        .and_then(json_text)
        .or_else(|| {
            value
                .pointer("/error/userInfo/NSLocalizedDescription")
                .and_then(json_text)
        })
}

fn devicectl_succeeded(json: &str) -> bool {
    let Ok(value) = serde_json::from_str::<Value>(json) else {
        return false;
    };
    matches!(
        value
            .pointer("/info/outcome")
            .and_then(Value::as_str)
            .map(|outcome| outcome.to_ascii_lowercase())
            .as_deref(),
        Some("success") | Some("succeeded") | Some("ok")
    )
}

fn json_text(value: &Value) -> Option<String> {
    value.as_str().map(str::to_string).or_else(|| {
        value
            .get("string")
            .and_then(Value::as_str)
            .map(str::to_string)
    })
}

fn temp_json_path(kind: &str) -> std::path::PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    std::env::temp_dir().join(format!(
        "devsync-{kind}-{}-{nanos}.json",
        std::process::id()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parses_devices_and_connection_details() {
        let json = serde_json::json!({"result": {"devices": [{"identifier": "abc", "deviceProperties": {"name": "Chris iPhone", "osVersion": "18.0"}, "hardwareProperties": {"platform": "iOS", "model": "iPhone17,1"}, "connectionProperties": {"transportType": "network", "connectionState": "connected"}}]}});
        let devices = parse_devices(&json);
        assert_eq!(devices.len(), 1);
        assert_eq!(
            devices[0].connection_type.as_deref(),
            Some("Network / Wi-Fi")
        );
        assert!(is_connected(&devices[0]));
    }

    #[test]
    fn parses_xcode_26_local_network_device_shape() {
        let json = serde_json::json!({"result": {"devices": [{
            "identifier": "core-device-id",
            "deviceProperties": {"name": "iPhone", "osVersionNumber": "26.4.1"},
            "hardwareProperties": {"platform": "iOS", "marketingName": "iPhone 13 Pro Max"},
            "connectionProperties": {"transportType": "localNetwork", "pairingState": "paired", "tunnelState": "connected"}
        }]}});
        let devices = parse_devices(&json);
        assert_eq!(devices[0].model.as_deref(), Some("iPhone 13 Pro Max"));
        assert_eq!(devices[0].os_version.as_deref(), Some("26.4.1"));
        assert_eq!(
            devices[0].connection_type.as_deref(),
            Some("Network / Wi-Fi")
        );
        assert_eq!(devices[0].connection_state, "Connected");
    }

    #[test]
    fn treats_a_paired_disconnected_tunnel_as_unknown_until_probed() {
        let json = serde_json::json!({"result": {"devices": [{
            "identifier": "core-device-id",
            "deviceProperties": {"name": "iPhone", "osVersionNumber": "26.4.1"},
            "hardwareProperties": {"platform": "iOS"},
            "connectionProperties": {"transportType": "localNetwork", "pairingState": "paired", "tunnelState": "disconnected"}
        } ]}});
        let devices = parse_devices(&json);
        assert_eq!(devices[0].connection_state, "Unknown");
        assert!(!is_connected(&devices[0]));
    }

    #[test]
    fn requires_a_success_outcome_for_structured_probe_results() {
        assert!(devicectl_succeeded(r#"{"info":{"outcome":"success"}}"#));
        assert!(!devicectl_succeeded(r#"{"info":{"outcome":"timeout"}}"#));
        assert!(!devicectl_succeeded(r#"{"result":{}}"#));
    }

    #[test]
    fn accepts_connection_state_aliases() {
        for state in ["Connected", "connected", "Available", "online"] {
            let device = DevSyncDeviceInfo {
                id: "device".into(),
                name: "iPhone".into(),
                model: None,
                os_version: None,
                connection_state: state.into(),
                connection_type: None,
                last_seen: String::new(),
            };
            assert!(is_connected(&device), "state should be connected: {state}");
        }
    }
    #[test]
    fn handles_zero_devices_and_ignores_non_ios_devices() {
        assert!(parse_devices(&serde_json::json!({"result": {"devices": []}})).is_empty());
        assert!(parse_devices(&serde_json::json!({"result": {"devices": [{"identifier": "mac", "hardwareProperties": {"platform": "macOS"}}]}})).is_empty());
    }
}
