use std::{
    collections::HashSet,
    fs,
    path::{Path, PathBuf},
    process::Stdio,
    sync::Mutex,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use plist::Value;
use tauri::AppHandle;
use tokio::process::Command;

use super::{
    DevSyncError,
    locking::ProcessLock,
    models::{BuildResult, Workspace, XcodeContainer},
    paths::DevSyncPaths,
    refresh_policy::DEFAULT_REFRESH_THRESHOLD_SECONDS,
    signing, workspace, xcode,
};

static ACTIVE_WORKSPACES: once_cell::sync::Lazy<Mutex<HashSet<String>>> =
    once_cell::sync::Lazy::new(|| Mutex::new(HashSet::new()));

/// Filesystem notifications can arrive just after a subprocess exits. Keep a
/// short drain period after an operation so build-generated source-like files
/// do not schedule a second Auto Sync.
const WATCHER_DRAIN_WINDOW: Duration = Duration::from_secs(5);

#[derive(Clone, Copy)]
pub enum BuildProgress {
    PreBuilding,
    PreBuildSucceeded,
    XcodeBuilding,
}

pub struct WorkspaceOperationGuard {
    workspace_id: String,
    paths: DevSyncPaths,
    _process_lock: ProcessLock,
}

impl Drop for WorkspaceOperationGuard {
    fn drop(&mut self) {
        if let Ok(mut active) = ACTIVE_WORKSPACES.lock() {
            active.remove(&self.workspace_id);
        }
        let deadline = SystemTime::now()
            .checked_add(WATCHER_DRAIN_WINDOW)
            .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
            .map(|duration| duration.as_secs())
            .unwrap_or_default();
        if let Some(parent) = self.paths.suppression_path(&self.workspace_id).parent() {
            let _ = fs::create_dir_all(parent);
        }
        let _ = fs::write(
            self.paths.suppression_path(&self.workspace_id),
            deadline.to_string(),
        );
    }
}

pub fn acquire_workspace_operation(
    paths: &DevSyncPaths,
    workspace_id: &str,
) -> Result<WorkspaceOperationGuard, DevSyncError> {
    let mut active = ACTIVE_WORKSPACES.lock().map_err(|_| {
        DevSyncError::new(
            "operation_lock_failed",
            "DevSync could not coordinate this workspace operation.",
        )
    })?;
    if !active.insert(workspace_id.to_string()) {
        return Err(DevSyncError::new(
            "operation_in_progress",
            "A build or deployment is already running for this workspace.",
        ));
    }
    let process_lock = match ProcessLock::try_acquire(&paths.operation_lock_path(workspace_id))? {
        Some(lock) => lock,
        None => {
            active.remove(workspace_id);
            return Err(DevSyncError::new(
                "operation_in_progress",
                "A build or deployment is already running for this workspace.",
            ));
        }
    };
    let _ = fs::remove_file(paths.suppression_path(workspace_id));
    Ok(WorkspaceOperationGuard {
        workspace_id: workspace_id.to_string(),
        paths: paths.clone(),
        _process_lock: process_lock,
    })
}

pub fn has_active_workspace_operations() -> bool {
    ACTIVE_WORKSPACES
        .lock()
        .map(|active| !active.is_empty())
        .unwrap_or(true)
}

/// A workspace watcher must not treat files produced by DevSync's own build
/// recipe as a user source change. This applies to every registered workspace,
/// rather than recognizing project names or framework-specific output paths.
pub fn is_workspace_watcher_suppressed(paths: &DevSyncPaths, workspace_id: &str) -> bool {
    if ACTIVE_WORKSPACES
        .lock()
        .map(|active| active.contains(workspace_id))
        .unwrap_or(true)
    {
        return true;
    }
    if ProcessLock::try_acquire(&paths.operation_lock_path(workspace_id))
        .map(|lock| lock.is_none())
        .unwrap_or(true)
    {
        return true;
    }
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(u64::MAX);
    fs::read_to_string(paths.suppression_path(workspace_id))
        .ok()
        .and_then(|deadline| deadline.trim().parse::<u64>().ok())
        .is_some_and(|deadline| deadline > now)
}

#[tauri::command]
pub async fn build_devsync_workspace(
    app: AppHandle,
    workspace_id: String,
) -> Result<BuildResult, DevSyncError> {
    let paths = DevSyncPaths::from_app(&app)?;
    let _guard = acquire_workspace_operation(&paths, &workspace_id)?;
    let store = workspace::load_store_at(&paths)?;
    let selected = store
        .workspaces
        .iter()
        .find(|item| item.id == workspace_id)
        .cloned()
        .ok_or_else(|| DevSyncError::new("workspace_not_found", "Workspace no longer exists."))?;
    let result = build_workspace(&paths, selected).await?;
    persist_build_result(&paths, &workspace_id, result)
}

pub fn persist_build_result(
    paths: &DevSyncPaths,
    workspace_id: &str,
    result: BuildResult,
) -> Result<BuildResult, DevSyncError> {
    let mut state = workspace::load_store_at(paths)?;
    let saved = {
        let saved = workspace::workspace_mut(&mut state, workspace_id)?;
        saved.last_build_at = Some(workspace::now());
        saved.last_build_status = Some(
            if result.succeeded {
                "succeeded"
            } else {
                "failed"
            }
            .into(),
        );
        saved.last_artifact_path = result
            .succeeded
            .then(|| result.workspace.last_artifact_path.clone())
            .flatten();
        saved.last_build_log_path = result.workspace.last_build_log_path.clone();
        saved.last_artifact_fingerprint_path = result
            .succeeded
            .then(|| result.workspace.last_artifact_fingerprint_path.clone())
            .flatten();
        saved.signing_status = result.workspace.signing_status.clone();
        saved.updated_at = workspace::now();
        saved.clone()
    };
    workspace::save_store_at(paths, &state)?;
    Ok(BuildResult {
        workspace: saved,
        ..result
    })
}

pub async fn build_workspace(
    paths: &DevSyncPaths,
    workspace: Workspace,
) -> Result<BuildResult, DevSyncError> {
    build_workspace_with_options(paths, workspace, false, |_| {}).await
}

pub async fn build_workspace_with_progress<F>(
    paths: &DevSyncPaths,
    workspace: Workspace,
    progress: F,
) -> Result<BuildResult, DevSyncError>
where
    F: Fn(BuildProgress),
{
    build_workspace_with_options(paths, workspace, false, progress).await
}

pub async fn build_workspace_for_signing_refresh<F>(
    paths: &DevSyncPaths,
    workspace: Workspace,
    progress: F,
) -> Result<BuildResult, DevSyncError>
where
    F: Fn(BuildProgress),
{
    build_workspace_with_options(paths, workspace, true, progress).await
}

async fn build_workspace_with_options<F>(
    paths: &DevSyncPaths,
    mut workspace: Workspace,
    force_signing_refresh: bool,
    progress: F,
) -> Result<BuildResult, DevSyncError>
where
    F: Fn(BuildProgress),
{
    if !Path::new(&workspace.folder_path).is_dir() {
        return Err(DevSyncError::new(
            "workspace_unavailable",
            "The workspace folder is currently unavailable.",
        ));
    }
    let container = match (&workspace.xcode_container_path, &workspace.container_type) {
        (Some(path), Some(kind)) if Path::new(path).exists() => XcodeContainer {
            path: path.clone(),
            container_type: kind.clone(),
        },
        _ => {
            return Err(DevSyncError::new(
                "container_required",
                "Choose an Xcode project or workspace before building.",
            ));
        }
    };
    let scheme = workspace
        .selected_scheme
        .clone()
        .ok_or_else(|| DevSyncError::new("scheme_required", "Choose a scheme before building."))?;
    let configuration = workspace
        .build_configuration
        .clone()
        .unwrap_or_else(|| "Debug".into());
    let derived = paths.derived_data_path(&workspace.id);
    fs::create_dir_all(&derived).map_err(|e| {
        DevSyncError::new(
            "filesystem_error",
            format!("Unable to create DevSync DerivedData: {e}"),
        )
    })?;
    // The timestamp is captured before the recipe runs. This prevents a prior
    // product in DevSync's reusable DerivedData directory from being installed.
    let build_started_at = SystemTime::now();
    let pre_build_log = if workspace.pre_build_enabled.unwrap_or(true) {
        if let Some(command) = workspace.pre_build_command.as_deref() {
            progress(BuildProgress::PreBuilding);
            let pre_build_directory = workspace
                .pre_build_working_directory
                .as_deref()
                .unwrap_or(&workspace.folder_path);
            if !Path::new(pre_build_directory).is_dir() {
                return Err(DevSyncError::new(
                    "pre_build_working_directory_unavailable",
                    "The configured pre-build working directory is unavailable.",
                ));
            }
            let output = Command::new("/bin/zsh")
                .args(["-lc", command])
                .current_dir(pre_build_directory)
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .output()
                .await
                .map_err(|e| {
                    DevSyncError::new(
                        "pre_build_unavailable",
                        format!("Unable to launch the pre-build command: {e}"),
                    )
                })?;
            let log = format!(
                "$ {}\n\n{}{}",
                command,
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            if !output.status.success() {
                let log_path = derived.join("DevSync-build.log");
                fs::write(&log_path, &log).map_err(|e| {
                    DevSyncError::new(
                        "filesystem_error",
                        format!("Unable to save the build log: {e}"),
                    )
                })?;
                workspace.last_build_log_path = Some(log_path.to_string_lossy().to_string());
                return Ok(BuildResult {
                    workspace,
                    succeeded: false,
                    diagnostic: Some(command_diagnostic(&log)),
                    log,
                });
            }
            progress(BuildProgress::PreBuildSucceeded);
            Some(log)
        } else {
            None
        }
    } else {
        None
    };
    let previous_expiration = workspace
        .signing_status
        .as_ref()
        .and_then(|status| status.expiration_date.clone());
    let mut args = xcode::container_args(&container);
    args.extend([
        "-scheme".into(),
        scheme,
        "-configuration".into(),
        configuration.clone(),
        "-destination".into(),
        "generic/platform=iOS".into(),
        "-derivedDataPath".into(),
        derived.to_string_lossy().to_string(),
    ]);
    // These flags let Xcode refresh profiles/certificates and register the
    // selected development device using the account configured in Xcode's
    // Accounts settings.
    args.push("-allowProvisioningUpdates".into());
    args.push("-allowProvisioningDeviceRegistration".into());
    if force_signing_refresh {
        // A renewal must cause Xcode to produce a new signed product instead
        // of accepting the unchanged product in DevSync's private cache.
        args.push("clean".into());
    }
    args.push("build".into());
    progress(BuildProgress::XcodeBuilding);
    let output = Command::new("xcodebuild")
        .args(&args)
        .current_dir(&workspace.folder_path)
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
    let xcode_log = format!(
        "$ xcodebuild {}\n\n{}{}",
        args.join(" "),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let log = pre_build_log
        .map(|pre_build_log| format!("{pre_build_log}\n\n{xcode_log}"))
        .unwrap_or(xcode_log);
    let log_path = derived.join("DevSync-build.log");
    fs::write(&log_path, &log).map_err(|e| {
        DevSyncError::new(
            "filesystem_error",
            format!("Unable to save the build log: {e}"),
        )
    })?;
    workspace.last_build_log_path = Some(log_path.to_string_lossy().to_string());
    if !output.status.success() {
        let diagnostic = xcode::relevant_diagnostic(&log);
        return Ok(BuildResult {
            workspace,
            succeeded: false,
            log,
            diagnostic: Some(diagnostic),
        });
    }
    let artifact = workspace
        .bundle_identifier
        .as_deref()
        .and_then(|bundle_id| {
            if force_signing_refresh {
                newest_app_for_bundle_since(&derived, bundle_id, build_started_at)
            } else {
                select_app_artifact(&derived, bundle_id, build_started_at)
            }
        });
    let Some(artifact) = artifact else {
        return Ok(BuildResult {
            workspace,
            succeeded: false,
            log,
            diagnostic: Some(
                "xcodebuild succeeded but no newly generated .app matching this workspace Bundle ID was found."
                    .into(),
            ),
        });
    };
    workspace.last_artifact_path = Some(artifact.to_string_lossy().to_string());
    let signing_status = match signing::inspect_embedded_profile(&artifact).await {
        Ok(status) if signing::has_usable_profile(&status) => status,
        Ok(status) => {
            workspace.last_artifact_path = None;
            workspace.signing_status = Some(status);
            return Ok(BuildResult {
                workspace,
                succeeded: false,
                log,
                diagnostic: Some(
                    "The built app does not contain a usable, non-expired provisioning profile."
                        .into(),
                ),
            });
        }
        Err(error) => {
            workspace.last_artifact_path = None;
            workspace.signing_status = None;
            return Ok(BuildResult {
                workspace,
                succeeded: false,
                log,
                diagnostic: Some(error.message),
            });
        }
    };
    if force_signing_refresh
        && !signing::is_sufficiently_renewed(
            &signing_status,
            previous_expiration.as_deref(),
            DEFAULT_REFRESH_THRESHOLD_SECONDS,
        )
    {
        workspace.last_artifact_path = None;
        workspace.signing_status = Some(signing_status);
        return Ok(BuildResult {
            workspace,
            succeeded: false,
            log,
            diagnostic: Some(
                "Xcode completed the build but did not produce a renewed provisioning profile."
                    .into(),
            ),
        });
    }
    workspace.signing_status = Some(signing_status);
    workspace.last_artifact_fingerprint_path = write_artifact_fingerprint(
        &derived,
        &container,
        &workspace,
        &configuration,
        &artifact,
        build_started_at,
    )
    .await
    .ok()
    .map(|path| path.to_string_lossy().to_string());
    Ok(BuildResult {
        workspace,
        succeeded: true,
        log,
        diagnostic: None,
    })
}

/// Writes a local, inspectable record of the exact app accepted for install.
/// It is deliberately generated after artifact selection so it documents the
/// same bundle DevSync will pass to `devicectl`, not merely xcodebuild intent.
async fn write_artifact_fingerprint(
    derived_data_path: &Path,
    container: &XcodeContainer,
    workspace: &Workspace,
    configuration: &str,
    artifact: &Path,
    build_started_at: SystemTime,
) -> Result<PathBuf, DevSyncError> {
    let info_path = artifact.join("Info.plist");
    let info = Value::from_file(&info_path).map_err(|error| {
        DevSyncError::new(
            "artifact_invalid",
            format!("Unable to read the selected app metadata: {error}"),
        )
    })?;
    let metadata = info.as_dictionary().ok_or_else(|| {
        DevSyncError::new(
            "artifact_invalid",
            "The selected app metadata is not a dictionary.",
        )
    })?;
    let executable_name = metadata
        .get("CFBundleExecutable")
        .and_then(Value::as_string)
        .unwrap_or_default();
    let executable_path = artifact.join(executable_name);
    let effective_settings = xcode::inspect_container(
        container,
        workspace.selected_scheme.as_deref(),
        Some(configuration),
    )
    .await
    .ok();
    let entitlements = Command::new("/usr/bin/codesign")
        .args(["-d", "--entitlements", "-"])
        .arg(artifact)
        .output()
        .await
        .ok()
        .map(|output| String::from_utf8_lossy(&output.stderr).to_string());
    let modified_at = |path: &Path| {
        fs::metadata(path)
            .and_then(|metadata| metadata.modified())
            .ok()
            .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
            .map(|duration| duration.as_secs())
    };
    let started_at = build_started_at
        .duration_since(UNIX_EPOCH)
        .ok()
        .map(|duration| duration.as_secs());
    let report = serde_json::json!({
        "workspace": workspace.folder_path,
        "sourceRevision": workspace.source_revision,
        "xcode": {
            "container": container.path,
            "containerType": container.container_type,
            "scheme": workspace.selected_scheme,
            "configuration": configuration,
            "sdk": effective_settings.as_ref().and_then(|settings| settings.sdk_root.as_deref()).unwrap_or("iPhoneOS"),
            "destination": "generic/platform=iOS",
            "derivedDataPath": derived_data_path,
            "action": "build",
            "commandLineBuildSettingOverrides": [],
            "codeSignStyle": effective_settings.as_ref().and_then(|settings| settings.code_sign_style.as_deref()),
            "developmentTeam": effective_settings.as_ref().and_then(|settings| settings.development_team.as_deref()).or(workspace.signing_team.as_deref()),
            "productBundleIdentifier": effective_settings.as_ref().and_then(|settings| settings.bundle_identifier.as_deref()).or(workspace.bundle_identifier.as_deref()),
        },
        "artifact": {
            "path": artifact,
            "buildStartedAtUnixSeconds": started_at,
            "modifiedAtUnixSeconds": modified_at(artifact),
            "bundleIdentifier": metadata.get("CFBundleIdentifier").and_then(Value::as_string),
            "supportedPlatforms": metadata.get("CFBundleSupportedPlatforms").and_then(Value::as_array),
            "executablePath": executable_path,
            "executableModifiedAtUnixSeconds": modified_at(&executable_path),
            "executableSha256": sha256(&executable_path).await,
            "infoPlistSha256": sha256(&info_path).await,
            "infoPlist": {
                "urlSchemes": metadata.get("CFBundleURLTypes"),
                "documentTypes": metadata.get("CFBundleDocumentTypes"),
                "backgroundModes": metadata.get("UIBackgroundModes"),
                "localNetworkUsageDescription": metadata.get("NSLocalNetworkUsageDescription"),
                "fileSharingEnabled": metadata.get("UIFileSharingEnabled"),
                "openDocumentsInPlace": metadata.get("LSSupportsOpeningDocumentsInPlace"),
            },
            "effectiveEntitlements": entitlements,
            "signing": workspace.signing_status,
        },
    });
    let path = derived_data_path.join("DevSync-artifact-fingerprint.json");
    let contents = serde_json::to_vec_pretty(&report).map_err(|error| {
        DevSyncError::new(
            "fingerprint_serialization_failed",
            format!("Unable to serialize the artifact report: {error}"),
        )
    })?;
    fs::write(&path, contents).map_err(|error| {
        DevSyncError::new(
            "filesystem_error",
            format!("Unable to save the artifact report: {error}"),
        )
    })?;
    Ok(path)
}

async fn sha256(path: &Path) -> Option<String> {
    let output = Command::new("/usr/bin/shasum")
        .args(["-a", "256"])
        .arg(path)
        .output()
        .await
        .ok()?;
    output.status.success().then(|| {
        String::from_utf8_lossy(&output.stdout)
            .split_whitespace()
            .next()
            .unwrap_or_default()
            .to_string()
    })
}

fn command_diagnostic(log: &str) -> String {
    log.lines()
        .rev()
        .find(|line| !line.trim().is_empty() && !line.starts_with('$'))
        .map(str::to_string)
        .unwrap_or_else(|| "Pre-Build command failed.".into())
}

#[cfg(test)]
pub fn newest_app(root: &Path) -> Option<PathBuf> {
    let mut apps = Vec::new();
    collect_apps(root, 0, &mut apps);
    apps.into_iter().max_by_key(|path| {
        fs::metadata(path)
            .and_then(|metadata| metadata.modified())
            .ok()
    })
}

pub fn newest_app_for_bundle(root: &Path, bundle_identifier: &str) -> Option<PathBuf> {
    let mut apps = Vec::new();
    collect_apps(root, 0, &mut apps);
    apps.into_iter()
        .filter(|path| app_bundle_identifier(path).as_deref() == Some(bundle_identifier))
        .max_by_key(|path| {
            fs::metadata(path)
                .and_then(|metadata| metadata.modified())
                .ok()
        })
}

/// Prefer a product written by this build, but allow Xcode's incremental build
/// to reuse a valid product from this workspace's private DerivedData folder.
fn select_app_artifact(
    root: &Path,
    bundle_identifier: &str,
    started_at: SystemTime,
) -> Option<PathBuf> {
    newest_app_for_bundle_since(root, bundle_identifier, started_at)
        .or_else(|| newest_app_for_bundle(root, bundle_identifier))
}

/// Returns only an artifact whose bundle directory or Info.plist was written
/// during this build invocation. DevSync never searches Xcode's global cache.
pub fn newest_app_for_bundle_since(
    root: &Path,
    bundle_identifier: &str,
    started_at: SystemTime,
) -> Option<PathBuf> {
    let mut apps = Vec::new();
    collect_apps(root, 0, &mut apps);
    apps.into_iter()
        .filter(|path| app_bundle_identifier(path).as_deref() == Some(bundle_identifier))
        .filter(|path| was_written_since(path, started_at))
        .max_by_key(|path| {
            fs::metadata(path)
                .and_then(|metadata| metadata.modified())
                .ok()
        })
}

fn was_written_since(app: &Path, started_at: SystemTime) -> bool {
    let mut paths = vec![app.to_path_buf(), app.join("Info.plist")];
    if let Some(executable) = app_bundle_executable(app) {
        paths.push(app.join(executable));
    }
    paths.push(app.join("embedded.mobileprovision"));
    paths.iter().any(|path| {
        fs::metadata(path)
            .and_then(|metadata| metadata.modified())
            .is_ok_and(|modified| modified >= started_at)
    })
}

fn app_bundle_identifier(app: &Path) -> Option<String> {
    Value::from_file(app.join("Info.plist"))
        .ok()?
        .as_dictionary()?
        .get("CFBundleIdentifier")?
        .as_string()
        .map(str::to_string)
}

fn app_bundle_executable(app: &Path) -> Option<String> {
    Value::from_file(app.join("Info.plist"))
        .ok()?
        .as_dictionary()?
        .get("CFBundleExecutable")?
        .as_string()
        .map(str::to_string)
}

fn collect_apps(directory: &Path, depth: usize, apps: &mut Vec<PathBuf>) {
    if depth > 8 {
        return;
    }
    let Ok(entries) = fs::read_dir(directory) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() && path.extension().is_some_and(|extension| extension == "app") {
            apps.push(path);
        } else if path.is_dir() {
            collect_apps(&path, depth + 1, apps);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{env, fs};
    #[test]
    fn finds_an_app_artifact() {
        let root = env::temp_dir().join(format!("devsync-artifact-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("Build/Products/Debug-iphoneos/Demo.app")).unwrap();
        assert!(newest_app(&root).unwrap().ends_with("Demo.app"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn selects_the_app_matching_the_workspace_bundle_id() {
        let root = env::temp_dir().join(format!(
            "devsync-artifact-bundle-test-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        for (name, bundle) in [
            ("Demo.app", "me.example.demo"),
            ("DemoTests-Runner.app", "me.example.demo.tests"),
        ] {
            let app = root.join("Build/Products/Debug-iphoneos").join(name);
            fs::create_dir_all(&app).unwrap();
            fs::write(app.join("Info.plist"), format!("<?xml version=\"1.0\"?><plist version=\"1.0\"><dict><key>CFBundleIdentifier</key><string>{bundle}</string></dict></plist>")).unwrap();
        }
        assert!(
            newest_app_for_bundle(&root, "me.example.demo")
                .unwrap()
                .ends_with("Demo.app")
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn reuses_a_matching_incremental_build_product_when_metadata_is_unchanged() {
        let root = env::temp_dir().join(format!(
            "devsync-incremental-artifact-test-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        let app = root.join("Build/Products/Debug-iphoneos/Demo.app");
        fs::create_dir_all(&app).unwrap();
        fs::write(
            app.join("Info.plist"),
            "<?xml version=\"1.0\"?><plist version=\"1.0\"><dict><key>CFBundleIdentifier</key><string>me.example.demo</string><key>CFBundleExecutable</key><string>Demo</string></dict></plist>",
        )
        .unwrap();

        let started_at = SystemTime::now() + Duration::from_secs(60);
        assert_eq!(
            select_app_artifact(&root, "me.example.demo", started_at),
            Some(app)
        );
        fs::remove_dir_all(root).unwrap();
    }
}
