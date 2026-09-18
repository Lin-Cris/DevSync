use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
    process::Stdio,
};

use serde_json::Value;
use tokio::process::Command;

use super::{
    DevSyncError,
    models::{XcodeContainer, XcodeMetadata},
};

const MAX_DISCOVERY_DEPTH: usize = 3;
const IGNORED_DIRECTORIES: &[&str] = &[
    "DerivedData",
    "build",
    "Build",
    ".build",
    "Pods",
    "Carthage",
    ".git",
    "node_modules",
];

pub fn discover_containers(folder: &Path) -> Result<Vec<XcodeContainer>, DevSyncError> {
    if !folder.is_dir() {
        return Err(DevSyncError::new(
            "workspace_unavailable",
            "The selected workspace folder is unavailable.",
        ));
    }
    let mut containers = Vec::new();
    scan_directory(folder, 0, &mut containers)?;
    containers.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(containers)
}

fn scan_directory(
    directory: &Path,
    depth: usize,
    containers: &mut Vec<XcodeContainer>,
) -> Result<(), DevSyncError> {
    let entries = fs::read_dir(directory).map_err(|e| {
        DevSyncError::new(
            "workspace_unreadable",
            format!("Unable to read workspace folder: {e}"),
        )
    })?;
    for entry in entries {
        let entry = entry.map_err(|e| {
            DevSyncError::new(
                "workspace_unreadable",
                format!("Unable to read workspace entry: {e}"),
            )
        })?;
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();
        if path.is_dir() && name.ends_with(".xcworkspace") {
            containers.push(container(path, "workspace"));
        } else if path.is_dir() && name.ends_with(".xcodeproj") {
            containers.push(container(path, "project"));
        } else if path.is_dir()
            && depth < MAX_DISCOVERY_DEPTH
            && !IGNORED_DIRECTORIES.contains(&name.as_str())
        {
            scan_directory(&path, depth + 1, containers)?;
        }
    }
    Ok(())
}

fn container(path: PathBuf, container_type: &str) -> XcodeContainer {
    XcodeContainer {
        path: path.to_string_lossy().to_string(),
        container_type: container_type.to_string(),
    }
}

pub fn preferred_container(containers: &[XcodeContainer]) -> Option<XcodeContainer> {
    let workspaces: Vec<_> = containers
        .iter()
        .filter(|c| c.container_type == "workspace")
        .collect();
    if workspaces.len() == 1 {
        return Some((*workspaces[0]).clone());
    }
    if workspaces.is_empty() && containers.len() == 1 {
        return Some(containers[0].clone());
    }
    None
}

pub async fn inspect_container(
    container: &XcodeContainer,
    scheme: Option<&str>,
    configuration: Option<&str>,
) -> Result<XcodeMetadata, DevSyncError> {
    let args = container_args(container);
    let listing = run_xcodebuild(args.iter().map(String::as_str).chain(["-list", "-json"])).await?;
    let value: Value = serde_json::from_str(&listing).map_err(|e| {
        DevSyncError::new(
            "malformed_xcode_json",
            format!("xcodebuild returned invalid project metadata: {e}"),
        )
    })?;
    let mut metadata = parse_list_metadata(&value);
    if let Some(scheme) = scheme {
        let config = configuration.unwrap_or("Debug");
        let mut settings_args = container_args(container);
        settings_args.extend([
            "-scheme".into(),
            scheme.into(),
            "-configuration".into(),
            config.into(),
            "-showBuildSettings".into(),
            "-json".into(),
        ]);
        let output = run_xcodebuild(settings_args.iter().map(String::as_str)).await?;
        let settings: Value = serde_json::from_str(&output).map_err(|e| {
            DevSyncError::new(
                "malformed_xcode_json",
                format!("xcodebuild returned invalid build settings: {e}"),
            )
        })?;
        apply_build_settings(&mut metadata, &settings);
    }
    Ok(metadata)
}

pub fn container_args(container: &XcodeContainer) -> Vec<String> {
    match container.container_type.as_str() {
        "workspace" => vec!["-workspace".into(), container.path.clone()],
        _ => vec!["-project".into(), container.path.clone()],
    }
}

async fn run_xcodebuild<'a>(
    args: impl IntoIterator<Item = &'a str>,
) -> Result<String, DevSyncError> {
    let output = Command::new("xcodebuild")
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await
        .map_err(|e| {
            DevSyncError::new(
                "xcode_unavailable",
                format!("Unable to launch xcodebuild: {e}"),
            )
        })?;
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let combined = format!("{stdout}{}", String::from_utf8_lossy(&output.stderr));
    if output.status.success() {
        Ok(stdout)
    } else {
        Err(DevSyncError::new(
            "xcodebuild_failed",
            relevant_diagnostic(&combined),
        ))
    }
}

pub fn parse_list_metadata(value: &Value) -> XcodeMetadata {
    let mut schemes = BTreeSet::new();
    let mut configurations = BTreeSet::new();
    let mut targets = BTreeSet::new();
    collect_named_values(value, "schemes", &mut schemes);
    collect_named_values(value, "configurations", &mut configurations);
    collect_named_values(value, "targets", &mut targets);
    XcodeMetadata {
        schemes: schemes.into_iter().collect(),
        configurations: configurations.into_iter().collect(),
        targets: targets.into_iter().collect(),
        ..Default::default()
    }
}

fn collect_named_values(value: &Value, key: &str, values: &mut BTreeSet<String>) {
    match value {
        Value::Object(map) => {
            if let Some(Value::Array(items)) = map.get(key) {
                for item in items {
                    if let Some(value) = item.as_str() {
                        values.insert(value.to_string());
                    }
                }
            }
            for nested in map.values() {
                collect_named_values(nested, key, values);
            }
        }
        Value::Array(items) => {
            for item in items {
                collect_named_values(item, key, values);
            }
        }
        _ => {}
    }
}

pub fn apply_build_settings(metadata: &mut XcodeMetadata, value: &Value) {
    let settings = value
        .as_array()
        .and_then(|items| items.first())
        .and_then(|item| item.get("buildSettings"))
        .and_then(Value::as_object);
    let Some(settings) = settings else {
        return;
    };
    let get = |key: &str| {
        settings
            .get(key)
            .and_then(Value::as_str)
            .map(str::to_string)
    };
    metadata.product_name = get("PRODUCT_NAME");
    metadata.bundle_identifier = get("PRODUCT_BUNDLE_IDENTIFIER");
    metadata.sdk_root = get("SDKROOT");
    metadata.supported_platforms = get("SUPPORTED_PLATFORMS");
    metadata.development_team = get("DEVELOPMENT_TEAM");
    metadata.code_sign_style = get("CODE_SIGN_STYLE");
    metadata.product_type = get("PRODUCT_TYPE");
}

pub fn relevant_diagnostic(log: &str) -> String {
    // Xcode always appends a summary such as `** BUILD FAILED **` after the
    // useful compiler or signing diagnostics. Keep that summary as a final
    // fallback only; otherwise it hides the actionable error in the UI.
    let line = log
        .lines()
        .rev()
        .find(|line| {
            let lower = line.to_ascii_lowercase();
            (lower.contains("error:") || lower.contains("fatal error:"))
                && !lower.contains("** build failed **")
        })
        .or_else(|| {
            log.lines().rev().find(|line| {
                let lower = line.to_ascii_lowercase();
                lower.contains("signing") && !lower.contains("** build failed **")
            })
        })
        .or_else(|| {
            log.lines().rev().find(|line| {
                let lower = line.to_ascii_lowercase();
                !line.trim().is_empty() && !lower.contains("** build failed **")
            })
        })
        .or_else(|| log.lines().rev().find(|line| !line.trim().is_empty()))
        .unwrap_or("xcodebuild failed.");
    if line.to_lowercase().contains("signing") {
        format!("{line}\nOpen this project in Xcode and configure Signing & Capabilities first.")
    } else {
        line.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{env, fs};

    fn fixture(name: &str) -> PathBuf {
        let path = env::temp_dir().join(format!(
            "devsync-xcode-test-{}-{}",
            std::process::id(),
            name
        ));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).unwrap();
        path
    }

    #[test]
    fn discovers_direct_containers_and_prefers_a_single_workspace() {
        let path = fixture("workspace");
        fs::create_dir(path.join("App.xcodeproj")).unwrap();
        fs::create_dir(path.join("App.xcworkspace")).unwrap();
        let containers = discover_containers(&path).unwrap();
        assert_eq!(containers.len(), 2);
        assert_eq!(
            preferred_container(&containers).unwrap().container_type,
            "workspace"
        );
        fs::remove_dir_all(path).unwrap();
    }

    #[test]
    fn leaves_multiple_projects_ambiguous() {
        let path = fixture("ambiguous");
        fs::create_dir(path.join("One.xcodeproj")).unwrap();
        fs::create_dir(path.join("Two.xcodeproj")).unwrap();
        assert!(preferred_container(&discover_containers(&path).unwrap()).is_none());
        fs::remove_dir_all(path).unwrap();
    }

    #[test]
    fn parses_list_and_build_setting_json() {
        let mut metadata = parse_list_metadata(
            &serde_json::json!({"project": {"schemes": ["Demo"], "configurations": ["Debug"], "targets": ["Demo"]}}),
        );
        apply_build_settings(
            &mut metadata,
            &serde_json::json!([{"buildSettings": {"PRODUCT_NAME": "Demo", "PRODUCT_BUNDLE_IDENTIFIER": "me.example.demo", "DEVELOPMENT_TEAM": "TEAM"}}]),
        );
        assert_eq!(metadata.schemes, vec!["Demo"]);
        assert_eq!(
            metadata.bundle_identifier.as_deref(),
            Some("me.example.demo")
        );
    }

    #[test]
    fn constructs_xcodebuild_container_arguments() {
        let args = container_args(&XcodeContainer {
            path: "/tmp/Demo.xcworkspace".into(),
            container_type: "workspace".into(),
        });
        assert_eq!(
            args,
            vec![
                "-workspace".to_string(),
                "/tmp/Demo.xcworkspace".to_string()
            ]
        );
    }

    #[test]
    fn prefers_actionable_compiler_error_over_build_summary() {
        let log = "SwiftCompile normal arm64 Foo.swift\n".to_owned()
            + "Foo.swift:12:3: error: cannot find 'Bar' in scope\n"
            + "** BUILD FAILED **\n";
        assert_eq!(
            relevant_diagnostic(&log),
            "Foo.swift:12:3: error: cannot find 'Bar' in scope"
        );
    }
}
