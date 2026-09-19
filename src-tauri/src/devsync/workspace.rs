use std::{
    fs,
    path::Path,
    time::{SystemTime, UNIX_EPOCH},
};

use serde::Serialize;
use serde_json::Value;
use tauri::AppHandle;

use super::{
    DevSyncError, build,
    locking::ProcessLock,
    models::{ActivityEntry, Workspace, WorkspaceInspection, WorkspaceStore, XcodeContainer},
    paths::DevSyncPaths,
    signing, xcode,
};

const STORE_KEY: &str = "workspaceState";

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BuildCacheUsage {
    pub bytes: u64,
}

pub fn devsync_data_dir(app: &AppHandle) -> Result<std::path::PathBuf, DevSyncError> {
    Ok(DevSyncPaths::from_app(app)?.data_dir)
}

#[tauri::command]
pub async fn get_devsync_build_cache_usage(
    app: AppHandle,
) -> Result<BuildCacheUsage, DevSyncError> {
    Ok(BuildCacheUsage {
        bytes: directory_size(&devsync_data_dir(&app)?.join("DerivedData")),
    })
}

/// Deletes only DevSync-owned DerivedData, never projects, global Xcode caches,
/// devices, or signing material.
#[tauri::command]
pub async fn clean_devsync_build_cache(app: AppHandle) -> Result<BuildCacheUsage, DevSyncError> {
    if build::has_active_workspace_operations() {
        return Err(DevSyncError::new(
            "operation_in_progress",
            "Wait for the active sync to finish before cleaning the build cache.",
        ));
    }
    let paths = DevSyncPaths::from_app(&app)?;
    // The background Agent is a separate process, so the in-process guard
    // above cannot see its work. Hold every workspace lock while removing the
    // shared cache to prevent a concurrent xcodebuild from being interrupted.
    let workspaces = list_workspaces_at(&paths)?;
    let mut operation_locks = Vec::with_capacity(workspaces.len());
    for workspace in &workspaces {
        let Some(lock) = ProcessLock::try_acquire(&paths.operation_lock_path(&workspace.id))?
        else {
            return Err(DevSyncError::new(
                "operation_in_progress",
                "Wait for the active sync to finish before cleaning the build cache.",
            ));
        };
        operation_locks.push(lock);
    }
    let root = paths.data_dir.join("DerivedData");
    if root.exists() {
        fs::remove_dir_all(&root).map_err(|error| {
            DevSyncError::new(
                "filesystem_error",
                format!("Unable to clean DevSync build cache: {error}"),
            )
        })?;
    }
    Ok(BuildCacheUsage { bytes: 0 })
}

fn directory_size(path: &Path) -> u64 {
    let Ok(entries) = fs::read_dir(path) else {
        return 0;
    };
    entries
        .flatten()
        .map(|entry| {
            let path = entry.path();
            match entry.metadata() {
                Ok(metadata) if metadata.is_file() => metadata.len(),
                Ok(metadata) if metadata.is_dir() => directory_size(&path),
                _ => 0,
            }
        })
        .sum()
}

pub fn list_workspaces(app: &AppHandle) -> Result<Vec<Workspace>, DevSyncError> {
    Ok(load_store(app)?.workspaces)
}

pub fn list_workspaces_at(paths: &DevSyncPaths) -> Result<Vec<Workspace>, DevSyncError> {
    Ok(load_store_at(paths)?.workspaces)
}

#[tauri::command]
pub async fn list_devsync_workspaces(app: AppHandle) -> Result<Vec<Workspace>, DevSyncError> {
    list_workspaces(&app)
}

#[tauri::command]
pub async fn add_devsync_workspace(
    app: AppHandle,
    folder_path: String,
) -> Result<WorkspaceInspection, DevSyncError> {
    let folder = Path::new(&folder_path);
    if !folder.is_dir() {
        return Err(DevSyncError::new(
            "workspace_unavailable",
            "The selected folder is not available.",
        ));
    }
    let canonical = folder.canonicalize().map_err(|e| {
        DevSyncError::new(
            "workspace_unreadable",
            format!("Unable to read selected folder: {e}"),
        )
    })?;
    let canonical_path = canonical.to_string_lossy().to_string();
    let mut store = load_store(&app)?;
    if let Some(existing) = store
        .workspaces
        .iter()
        .find(|workspace| workspace.folder_path == canonical_path)
    {
        return inspect_and_persist(&app, existing.id.clone()).await;
    }
    let now = now();
    let display_name = canonical
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("Untitled Workspace")
        .to_string();
    let workspace = Workspace {
        id: workspace_id(),
        folder_path: canonical_path,
        display_name,
        created_at: now.clone(),
        updated_at: now,
        xcode_container_path: None,
        container_type: None,
        selected_scheme: None,
        pre_build_command: None,
        pre_build_working_directory: None,
        pre_build_enabled: Some(false),
        product_name: None,
        bundle_identifier: None,
        build_configuration: Some("Debug".into()),
        signing_team: None,
        signing_status: None,
        auto_sync: false,
        changes_detected: false,
        source_revision: 0,
        deployed_revision: 0,
        background_state: Some("healthy".into()),
        deployment_state: None,
        deployment_message: None,
        activities: vec![],
        last_build_status: None,
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
    let id = workspace.id.clone();
    store.workspaces.push(workspace);
    save_store(&app, &store)?;
    inspect_and_persist(&app, id).await
}

#[tauri::command]
pub async fn refresh_devsync_workspace(
    app: AppHandle,
    workspace_id: String,
) -> Result<WorkspaceInspection, DevSyncError> {
    inspect_and_persist(&app, workspace_id).await
}

#[tauri::command]
pub async fn remove_devsync_workspace(
    app: AppHandle,
    workspace_id: String,
) -> Result<(), DevSyncError> {
    let mut store = load_store(&app)?;
    let before = store.workspaces.len();
    store
        .workspaces
        .retain(|workspace| workspace.id != workspace_id);
    if before == store.workspaces.len() {
        return Err(DevSyncError::new(
            "workspace_not_found",
            "Workspace no longer exists.",
        ));
    }
    save_store(&app, &store)
}

#[tauri::command]
pub async fn select_devsync_container(
    app: AppHandle,
    workspace_id: String,
    container: XcodeContainer,
) -> Result<WorkspaceInspection, DevSyncError> {
    let mut store = load_store(&app)?;
    let workspace = workspace_mut(&mut store, &workspace_id)?;
    let candidates = xcode::discover_containers(Path::new(&workspace.folder_path))?;
    if !candidates.iter().any(|candidate| {
        candidate.path == container.path && candidate.container_type == container.container_type
    }) {
        return Err(DevSyncError::new(
            "invalid_container",
            "The selected Xcode container is not part of this workspace.",
        ));
    }
    workspace.xcode_container_path = Some(container.path);
    workspace.container_type = Some(container.container_type);
    workspace.selected_scheme = None;
    workspace.updated_at = now();
    save_store(&app, &store)?;
    inspect_and_persist(&app, workspace_id).await
}

#[tauri::command]
pub async fn select_devsync_scheme(
    app: AppHandle,
    workspace_id: String,
    scheme: String,
) -> Result<WorkspaceInspection, DevSyncError> {
    let mut store = load_store(&app)?;
    let workspace = workspace_mut(&mut store, &workspace_id)?;
    workspace.selected_scheme = Some(scheme);
    workspace.updated_at = now();
    save_store(&app, &store)?;
    inspect_and_persist(&app, workspace_id).await
}

#[tauri::command]
pub async fn set_devsync_auto_sync(
    app: AppHandle,
    workspace_id: String,
    enabled: bool,
) -> Result<Workspace, DevSyncError> {
    let mut store = load_store(&app)?;
    let workspace = workspace_mut(&mut store, &workspace_id)?;
    workspace.auto_sync = enabled;
    workspace.updated_at = now();
    let saved = workspace.clone();
    save_store(&app, &store)?;
    Ok(saved)
}

/// Saves a per-workspace build recipe. An empty value means no pre-build step.
#[tauri::command]
pub async fn set_devsync_pre_build_command(
    app: AppHandle,
    workspace_id: String,
    command: String,
) -> Result<Workspace, DevSyncError> {
    let mut store = load_store(&app)?;
    let workspace = workspace_mut(&mut store, &workspace_id)?;
    workspace.pre_build_command = match command.trim() {
        "" => {
            workspace.pre_build_enabled = Some(false);
            None
        }
        value => {
            workspace.pre_build_enabled = Some(true);
            Some(value.to_string())
        }
    };
    workspace.updated_at = now();
    let saved = workspace.clone();
    save_store(&app, &store)?;
    Ok(saved)
}

#[tauri::command]
pub async fn clear_devsync_activity(
    app: AppHandle,
    workspace_id: String,
) -> Result<Workspace, DevSyncError> {
    let mut store = load_store(&app)?;
    let workspace = workspace_mut(&mut store, &workspace_id)?;
    workspace.activities.clear();
    workspace.updated_at = now();
    let saved = workspace.clone();
    save_store(&app, &store)?;
    Ok(saved)
}

pub fn record_deployment_update_at(
    paths: &DevSyncPaths,
    workspace_id: &str,
    state: &str,
    message: &str,
) -> Result<(), DevSyncError> {
    let mut store = load_store_at(paths)?;
    let workspace = workspace_mut(&mut store, workspace_id)?;
    workspace.deployment_state = Some(state.to_string());
    workspace.deployment_message = Some(message.to_string());
    if let Some((kind, activity)) = deployment_activity(state, message) {
        append_activity(workspace, kind, &activity);
    }
    workspace.updated_at = now();
    save_store_at(paths, &store)
}

fn append_activity(workspace: &mut Workspace, kind: &str, message: &str) {
    let timestamp = now();
    if workspace.activities.last().is_some_and(|item| {
        item.kind == kind && item.message == message && item.timestamp == timestamp
    }) {
        return;
    }
    workspace.activities.push(ActivityEntry {
        id: format!("{}-{}", timestamp, workspace.activities.len()),
        kind: kind.to_string(),
        message: message.to_string(),
        timestamp,
    });
    if workspace.activities.len() > 30 {
        let overflow = workspace.activities.len() - 30;
        workspace.activities.drain(0..overflow);
    }
}

fn deployment_activity(state: &str, message: &str) -> Option<(&'static str, String)> {
    match state {
        "preparing" => Some(("sync", "Detected changes in project".into())),
        "building" => Some(("build", "Building project…".into())),
        "buildSucceeded" => Some(("build", "Build completed".into())),
        "signing" => Some(("signing", "Signing build…".into())),
        "waitingForDevice" => Some(("device", message.into())),
        "installing" => Some(("install", "Installing on iPhone…".into())),
        "installed" => Some(("install", "Installed on iPhone".into())),
        "buildFailed" => Some(("error", "Build failed".into())),
        "installFailed" => Some(("error", "Installation failed".into())),
        _ => None,
    }
}

pub async fn inspect_and_persist(
    app: &AppHandle,
    workspace_id: String,
) -> Result<WorkspaceInspection, DevSyncError> {
    let mut store = load_store(app)?;
    let index = store
        .workspaces
        .iter()
        .position(|workspace| workspace.id == workspace_id)
        .ok_or_else(|| DevSyncError::new("workspace_not_found", "Workspace no longer exists."))?;
    let mut workspace = store.workspaces[index].clone();
    let folder = Path::new(&workspace.folder_path);
    if !folder.is_dir() {
        workspace.unavailable = true;
        workspace.metadata_error = Some("Workspace folder is currently unavailable.".into());
        workspace.updated_at = now();
        store.workspaces[index] = workspace.clone();
        save_store(app, &store)?;
        return Ok(WorkspaceInspection {
            workspace,
            containers: vec![],
            metadata: None,
        });
    }
    let containers = xcode::discover_containers(folder)?;
    workspace.unavailable = false;
    let current = workspace
        .xcode_container_path
        .as_ref()
        .and_then(|path| containers.iter().find(|candidate| candidate.path == *path))
        .cloned();
    let selected = current.or_else(|| xcode::preferred_container(&containers));
    if let Some(container) = selected {
        workspace.xcode_container_path = Some(container.path.clone());
        workspace.container_type = Some(container.container_type.clone());
        let metadata = match xcode::inspect_container(
            &container,
            workspace.selected_scheme.as_deref(),
            workspace.build_configuration.as_deref(),
        )
        .await
        {
            Ok(mut metadata) => {
                if workspace.selected_scheme.is_none() && metadata.schemes.len() == 1 {
                    workspace.selected_scheme = metadata.schemes.first().cloned();
                    metadata = xcode::inspect_container(
                        &container,
                        workspace.selected_scheme.as_deref(),
                        workspace.build_configuration.as_deref(),
                    )
                    .await
                    .unwrap_or(metadata);
                }
                workspace.product_name = metadata.product_name.clone();
                if let (Some(expected), Some(detected)) = (
                    workspace.bundle_identifier.as_deref(),
                    metadata.bundle_identifier.as_deref(),
                ) && expected != detected
                {
                    workspace.metadata_error = Some(format!(
                        "Bundle ID changed from {expected} to {detected}. Review the Xcode target before syncing."
                    ));
                } else {
                    workspace.bundle_identifier = metadata.bundle_identifier.clone();
                    workspace.metadata_error = None;
                }
                workspace.signing_team = metadata.development_team.clone();
                Some(metadata)
            }
            Err(error) => {
                workspace.metadata_error = Some(error.message);
                None
            }
        };
        workspace.updated_at = now();
        store.workspaces[index] = workspace.clone();
        save_store(app, &store)?;
        Ok(WorkspaceInspection {
            workspace,
            containers,
            metadata,
        })
    } else {
        workspace.xcode_container_path = None;
        workspace.container_type = None;
        workspace.selected_scheme = None;
        workspace.metadata_error = Some(if containers.is_empty() {
            "No Xcode project or workspace was found in this folder.".into()
        } else {
            "Multiple Xcode containers were found. Choose one to continue.".into()
        });
        workspace.updated_at = now();
        store.workspaces[index] = workspace.clone();
        save_store(app, &store)?;
        Ok(WorkspaceInspection {
            workspace,
            containers,
            metadata: None,
        })
    }
}

pub fn load_store(app: &AppHandle) -> Result<WorkspaceStore, DevSyncError> {
    load_store_at(&DevSyncPaths::from_app(app)?)
}

pub fn save_store(app: &AppHandle, state: &WorkspaceStore) -> Result<(), DevSyncError> {
    save_store_at(&DevSyncPaths::from_app(app)?, state)
}

pub fn load_store_at(paths: &DevSyncPaths) -> Result<WorkspaceStore, DevSyncError> {
    let path = paths.workspace_store_path();
    if !path.is_file() {
        return Ok(WorkspaceStore::default());
    }
    let contents = fs::read_to_string(&path).map_err(|error| {
        DevSyncError::new(
            "storage_error",
            format!("Unable to read workspace storage: {error}"),
        )
    })?;
    let document: Value = serde_json::from_str(&contents).map_err(|error| {
        DevSyncError::new(
            "storage_error",
            format!("Workspace data is invalid: {error}"),
        )
    })?;
    let state = document.get(STORE_KEY).cloned().unwrap_or(document);
    let mut state: WorkspaceStore = serde_json::from_value(state).map_err(|error| {
        DevSyncError::new(
            "storage_error",
            format!("Workspace data is invalid: {error}"),
        )
    })?;
    // `remaining_seconds` is a derived display value. Refresh it whenever the
    // store is read so the scheduler and UI never make decisions from a stale
    // build-time snapshot.
    for workspace in &mut state.workspaces {
        if let Some(status) = workspace.signing_status.as_mut() {
            signing::refresh_status(status);
        }
    }
    Ok(state)
}

pub fn save_store_at(paths: &DevSyncPaths, state: &WorkspaceStore) -> Result<(), DevSyncError> {
    let _lock = ProcessLock::try_acquire(&paths.workspace_store_lock_path())?.ok_or_else(|| {
        DevSyncError::new(
            "storage_busy",
            "Workspace storage is currently being updated by another DevSync process.",
        )
    })?;
    fs::create_dir_all(&paths.app_data_dir).map_err(|error| {
        DevSyncError::new(
            "storage_error",
            format!("Unable to create workspace storage directory: {error}"),
        )
    })?;
    let mut document = if paths.workspace_store_path().is_file() {
        let contents = fs::read_to_string(paths.workspace_store_path()).map_err(|error| {
            DevSyncError::new(
                "storage_error",
                format!("Unable to read workspace storage: {error}"),
            )
        })?;
        serde_json::from_str::<Value>(&contents).map_err(|error| {
            DevSyncError::new(
                "storage_error",
                format!("Workspace data is invalid: {error}"),
            )
        })?
    } else {
        Value::Object(serde_json::Map::new())
    };
    let object = document.as_object_mut().ok_or_else(|| {
        DevSyncError::new(
            "storage_error",
            "Workspace storage root is not a JSON object.",
        )
    })?;
    object.insert(
        STORE_KEY.into(),
        serde_json::to_value(state).map_err(|error| {
            DevSyncError::new(
                "storage_error",
                format!("Unable to serialize workspace data: {error}"),
            )
        })?,
    );
    let serialized = serde_json::to_string_pretty(&document).map_err(|error| {
        DevSyncError::new(
            "storage_error",
            format!("Unable to serialize workspace data: {error}"),
        )
    })?;
    let temp_path = paths
        .workspace_store_path()
        .with_extension(format!("json.{}.tmp", std::process::id()));
    fs::write(&temp_path, serialized).map_err(|error| {
        DevSyncError::new(
            "storage_error",
            format!("Unable to write workspace storage: {error}"),
        )
    })?;
    fs::rename(&temp_path, paths.workspace_store_path()).map_err(|error| {
        DevSyncError::new(
            "storage_error",
            format!("Unable to commit workspace storage: {error}"),
        )
    })
}

pub fn workspace_mut<'a>(
    store: &'a mut WorkspaceStore,
    id: &str,
) -> Result<&'a mut Workspace, DevSyncError> {
    store
        .workspaces
        .iter_mut()
        .find(|workspace| workspace.id == id)
        .ok_or_else(|| DevSyncError::new("workspace_not_found", "Workspace no longer exists."))
}

pub fn now() -> String {
    chrono::Utc::now().to_rfc3339()
}

fn workspace_id() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    format!("workspace-{:x}-{}", nanos, std::process::id())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn workspace_store_uses_a_versioned_shape() {
        let state = WorkspaceStore::default();
        let json = serde_json::to_value(state).unwrap();
        assert_eq!(json["schemaVersion"], 1);
        assert!(json["workspaces"].is_array());
    }
}
