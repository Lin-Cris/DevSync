use std::{
    fs,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use plist::Value;
use serde_json::Value as JsonValue;
use tokio::process::Command;

use super::{
    DevSyncError,
    models::{DevSyncDeviceInfo, InstallResult, Workspace},
    workspace,
};

pub async fn install_latest_artifact(
    workspace_record: &Workspace,
    device: &DevSyncDeviceInfo,
) -> Result<InstallResult, DevSyncError> {
    if device.connection_state != "Connected" {
        return Err(DevSyncError::new(
            "device_unavailable",
            "The selected iPhone is currently unavailable.",
        ));
    }
    let artifact = validate_latest_artifact(workspace_record)?;
    let json_path = temp_json_path("install");
    let json_arg = json_path.to_string_lossy().to_string();
    let args = install_command_args(&device.id, &artifact, &json_arg);
    let output = Command::new("xcrun")
        .args(&args)
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
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    let log = format!(
        "$ xcrun {}\n\n{}{}\n{}",
        args.join(" "),
        String::from_utf8_lossy(&output.stdout),
        stderr,
        structured.as_deref().unwrap_or("")
    );
    let log_path = artifact
        .parent()
        .unwrap_or(Path::new("/private/tmp"))
        .join("DevSync-install.log");
    fs::write(&log_path, &log).map_err(|error| {
        DevSyncError::new(
            "filesystem_error",
            format!("Unable to save install log: {error}"),
        )
    })?;
    let succeeded = output.status.success()
        && structured
            .as_deref()
            .map(devicectl_succeeded)
            .unwrap_or(true);
    let diagnostic = if succeeded {
        None
    } else {
        Some(user_facing_diagnostic(&devicectl_diagnostic(
            structured.as_deref(),
            Some(&stderr),
            &log,
        )))
    };
    Ok(InstallResult {
        succeeded,
        log,
        diagnostic,
        installed_at: succeeded.then(workspace::now),
        artifact_path: artifact.to_string_lossy().to_string(),
        log_path: log_path.to_string_lossy().to_string(),
    })
}

pub fn validate_latest_artifact(workspace_record: &Workspace) -> Result<PathBuf, DevSyncError> {
    if workspace_record.last_build_status.as_deref() != Some("succeeded") {
        return Err(DevSyncError::new(
            "stale_artifact",
            "Build the workspace successfully before installing.",
        ));
    }
    let artifact = workspace_record
        .last_artifact_path
        .as_ref()
        .ok_or_else(|| {
            DevSyncError::new(
                "artifact_missing",
                "The latest successful build did not record an app artifact.",
            )
        })?;
    let artifact = PathBuf::from(artifact);
    if !artifact.is_dir()
        || artifact
            .extension()
            .is_none_or(|extension| extension != "app")
    {
        return Err(DevSyncError::new(
            "artifact_missing",
            "The app artifact from the latest build is no longer available.",
        ));
    }
    let plist_path = artifact.join("Info.plist");
    let plist = Value::from_file(&plist_path).map_err(|error| {
        DevSyncError::new(
            "artifact_invalid",
            format!("Unable to read the app bundle metadata: {error}"),
        )
    })?;
    let actual_bundle_id = plist
        .as_dictionary()
        .and_then(|dictionary| dictionary.get("CFBundleIdentifier"))
        .and_then(Value::as_string)
        .ok_or_else(|| {
            DevSyncError::new(
                "artifact_invalid",
                "The app bundle does not contain CFBundleIdentifier.",
            )
        })?;
    if let Some(expected_bundle_id) = &workspace_record.bundle_identifier
        && actual_bundle_id != expected_bundle_id
    {
        return Err(DevSyncError::new(
            "stale_artifact",
            format!(
                "The app artifact bundle ID ({actual_bundle_id}) does not match this workspace ({expected_bundle_id})."
            ),
        ));
    }
    let dictionary = plist.as_dictionary().ok_or_else(|| {
        DevSyncError::new(
            "artifact_invalid",
            "The app bundle metadata is not a property-list dictionary.",
        )
    })?;
    let is_device_app = dictionary
        .get("CFBundleSupportedPlatforms")
        .and_then(Value::as_array)
        .is_some_and(|platforms| {
            platforms
                .iter()
                .any(|value| value.as_string() == Some("iPhoneOS"))
        });
    if !is_device_app {
        return Err(DevSyncError::new(
            "artifact_invalid",
            "The selected artifact is not an iPhoneOS app bundle.",
        ));
    }
    if dictionary
        .get("CFBundlePackageType")
        .and_then(Value::as_string)
        != Some("APPL")
    {
        return Err(DevSyncError::new(
            "artifact_invalid",
            "The selected artifact is not an application bundle.",
        ));
    }
    let executable = dictionary
        .get("CFBundleExecutable")
        .and_then(Value::as_string)
        .filter(|name| !name.is_empty())
        .ok_or_else(|| {
            DevSyncError::new(
                "artifact_invalid",
                "The app bundle does not declare an executable.",
            )
        })?;
    if !artifact.join(executable).is_file() {
        return Err(DevSyncError::new(
            "artifact_invalid",
            "The app bundle executable is missing.",
        ));
    }
    Ok(artifact)
}

pub fn install_command_args(device_id: &str, artifact: &Path, json_path: &str) -> Vec<String> {
    vec![
        "devicectl".into(),
        "device".into(),
        "install".into(),
        "app".into(),
        "--device".into(),
        device_id.into(),
        artifact.to_string_lossy().to_string(),
        "--timeout".into(),
        "120".into(),
        "--json-output".into(),
        json_path.into(),
    ]
}

fn devicectl_succeeded(json: &str) -> bool {
    let value: JsonValue = match serde_json::from_str(json) {
        Ok(value) => value,
        Err(_) => return false,
    };
    !matches!(
        value
            .pointer("/info/outcome")
            .and_then(JsonValue::as_str)
            .map(|outcome| outcome.to_ascii_lowercase())
            .as_deref(),
        Some("error") | Some("failed") | Some("failure") | Some("timeout")
    )
}

fn devicectl_diagnostic(json: Option<&str>, stderr: Option<&str>, fallback: &str) -> String {
    json.and_then(|json| serde_json::from_str::<JsonValue>(json).ok())
        .and_then(|value| {
            value
                .pointer("/info/details")
                .and_then(json_text)
                .or_else(|| {
                    value
                        .pointer("/error/userInfo/NSLocalizedDescription")
                        .and_then(json_text)
                })
                .or_else(|| {
                    value
                        .pointer("/error/userInfo/NSLocalizedFailureReason")
                        .and_then(json_text)
                })
        })
        .or_else(|| stderr.and_then(last_diagnostic_line))
        .unwrap_or_else(|| {
            fallback
                .lines()
                .rev()
                .find(|line| {
                    let line = line.trim();
                    !line.is_empty() && !matches!(line, "{" | "}" | "[" | "]" | ",")
                })
                .unwrap_or("devicectl installation failed.")
                .to_string()
        })
}

fn json_text(value: &JsonValue) -> Option<String> {
    value.as_str().map(str::to_string).or_else(|| {
        value
            .get("string")
            .and_then(JsonValue::as_str)
            .map(str::to_string)
    })
}

fn last_diagnostic_line(text: &str) -> Option<String> {
    text.lines()
        .rev()
        .map(str::trim)
        .find(|line| {
            !line.is_empty()
                && !matches!(*line, "{" | "}" | "[" | "]" | ",")
                && !line.starts_with('$')
        })
        .map(str::to_string)
}

pub fn is_transient_device_error(diagnostic: Option<&str>) -> bool {
    let Some(diagnostic) = diagnostic else {
        return false;
    };
    let normalized = diagnostic.to_ascii_lowercase();
    normalized.contains("coredeviceservice")
        || normalized.contains("unable to locate a device")
        || normalized.contains("device identifier")
        || normalized.contains("timed out waiting for coredevice")
        || normalized.contains("coredeviceerror")
}

pub fn user_facing_diagnostic(raw: &str) -> String {
    let normalized = raw.to_lowercase();
    if normalized.contains("maximum number of installed apps")
        || normalized.contains("maximum number of apps")
        || normalized.contains("free provisioning") && normalized.contains("limit")
    {
        "Your iPhone has reached Apple's free Personal Team app limit.".into()
    } else {
        raw.into()
    }
}

fn temp_json_path(kind: &str) -> PathBuf {
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
    use std::env;

    #[test]
    fn creates_the_supported_install_command() {
        let args = install_command_args(
            "iphone-udid",
            Path::new("/tmp/Demo.app"),
            "/tmp/result.json",
        );
        assert_eq!(
            args,
            vec![
                "devicectl",
                "device",
                "install",
                "app",
                "--device",
                "iphone-udid",
                "/tmp/Demo.app",
                "--timeout",
                "120",
                "--json-output",
                "/tmp/result.json"
            ]
            .into_iter()
            .map(String::from)
            .collect::<Vec<_>>()
        );
    }
    #[test]
    fn rejects_a_stale_or_missing_artifact() {
        let workspace = Workspace {
            id: "workspace".into(),
            folder_path: "/tmp".into(),
            display_name: "Demo".into(),
            created_at: "".into(),
            updated_at: "".into(),
            xcode_container_path: None,
            container_type: None,
            selected_scheme: None,
            pre_build_command: None,
            pre_build_working_directory: None,
            pre_build_enabled: Some(false),
            product_name: None,
            bundle_identifier: Some("me.example.demo".into()),
            build_configuration: None,
            signing_team: None,
            signing_status: None,
            auto_sync: false,
            changes_detected: false,
            source_revision: 0,
            deployed_revision: 0,
            background_state: None,
            deployment_state: None,
            deployment_message: None,
            activities: vec![],
            last_build_status: Some("failed".into()),
            last_build_at: None,
            last_artifact_path: None,
            last_build_log_path: None,
            last_artifact_fingerprint_path: None,
            last_install_status: None,
            last_install_at: None,
            last_install_device_id: None,
            last_installed_artifact_path: None,
            last_install_log_path: None,
            metadata_error: None,
            unavailable: false,
        };
        assert_eq!(
            validate_latest_artifact(&workspace).unwrap_err().code,
            "stale_artifact"
        );
    }

    #[test]
    fn rejects_a_simulator_or_non_application_artifact() {
        let root = env::temp_dir().join(format!(
            "devsync-artifact-validation-test-{}",
            std::process::id()
        ));
        let app = root.join("Demo.app");
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&app).unwrap();
        fs::write(app.join("Demo"), "not a Mach-O fixture").unwrap();
        fs::write(
            app.join("Info.plist"),
            r#"<?xml version="1.0"?><plist version="1.0"><dict>
                <key>CFBundleIdentifier</key><string>me.example.demo</string>
                <key>CFBundleExecutable</key><string>Demo</string>
                <key>CFBundlePackageType</key><string>APPL</string>
                <key>CFBundleSupportedPlatforms</key><array><string>iPhoneSimulator</string></array>
            </dict></plist>"#,
        )
        .unwrap();
        let workspace = Workspace {
            id: "workspace".into(),
            folder_path: "/tmp".into(),
            display_name: "Demo".into(),
            created_at: "".into(),
            updated_at: "".into(),
            xcode_container_path: None,
            container_type: None,
            selected_scheme: Some("Demo".into()),
            pre_build_command: None,
            pre_build_working_directory: None,
            pre_build_enabled: Some(false),
            product_name: Some("Demo".into()),
            bundle_identifier: Some("me.example.demo".into()),
            build_configuration: Some("Debug".into()),
            signing_team: Some("TEAM".into()),
            signing_status: None,
            auto_sync: false,
            changes_detected: false,
            source_revision: 1,
            deployed_revision: 0,
            background_state: None,
            deployment_state: None,
            deployment_message: None,
            activities: vec![],
            last_build_status: Some("succeeded".into()),
            last_build_at: None,
            last_artifact_path: Some(app.to_string_lossy().to_string()),
            last_build_log_path: None,
            last_artifact_fingerprint_path: None,
            last_install_status: None,
            last_install_at: None,
            last_install_device_id: None,
            last_installed_artifact_path: None,
            last_install_log_path: None,
            metadata_error: None,
            unavailable: false,
        };
        assert_eq!(
            validate_latest_artifact(&workspace).unwrap_err().code,
            "artifact_invalid"
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn translates_the_free_personal_team_app_limit() {
        assert_eq!(
            user_facing_diagnostic("The maximum number of installed apps has been reached."),
            "Your iPhone has reached Apple's free Personal Team app limit."
        );
    }

    #[test]
    fn reads_xcode_26_nested_error_description_and_failed_outcome() {
        let json = r#"{
          "error": { "userInfo": {
            "NSLocalizedDescription": { "string": "CoreDeviceService could not find the iPhone." }
          }},
          "info": { "outcome": "failed" }
        }"#;
        assert!(!devicectl_succeeded(json));
        assert_eq!(
            devicectl_diagnostic(Some(json), None, "{\n}\n"),
            "CoreDeviceService could not find the iPhone."
        );
    }

    #[test]
    fn does_not_use_a_json_closing_brace_as_the_diagnostic() {
        assert_eq!(
            devicectl_diagnostic(None, Some("ERROR: device is unavailable\n"), "{\n}\n"),
            "ERROR: device is unavailable"
        );
    }

    #[test]
    fn recognizes_coredevice_connection_failures_as_transient() {
        assert!(is_transient_device_error(Some(
            "CoreDeviceService was unable to locate a device matching the requested device identifier"
        )));
        assert!(!is_transient_device_error(Some(
            "The application failed code signature verification"
        )));
    }
}
