use tauri::{AppHandle, Emitter, WebviewWindow};

use super::{
    DevSyncError, build, device, install,
    models::{DeploymentResult, DeploymentState, InstallResult, Workspace},
    paths::DevSyncPaths,
    refresh_policy::{RefreshDecision, RefreshPolicy},
    workspace,
};

#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct DeploymentUpdate<'a> {
    workspace_id: &'a str,
    state: DeploymentState,
    message: &'a str,
}

pub trait DeploymentReporter: Send + Sync {
    fn report(&self, workspace_id: &str, state: DeploymentState, message: &str);
}

pub(crate) struct TauriDeploymentReporter<'a> {
    window: &'a WebviewWindow,
    paths: &'a DevSyncPaths,
}

impl<'a> TauriDeploymentReporter<'a> {
    pub(crate) fn new(window: &'a WebviewWindow, paths: &'a DevSyncPaths) -> Self {
        Self { window, paths }
    }
}

impl DeploymentReporter for TauriDeploymentReporter<'_> {
    fn report(&self, workspace_id: &str, state: DeploymentState, message: &str) {
        let state_name = serde_json::to_string(&state)
            .unwrap_or_else(|_| "\"unknown\"".into())
            .trim_matches('"')
            .to_string();
        let _ =
            workspace::record_deployment_update_at(self.paths, workspace_id, &state_name, message);
        let _ = self.window.emit(
            "devsync-deployment",
            DeploymentUpdate {
                workspace_id,
                state,
                message,
            },
        );
    }
}

pub(crate) struct PersistedDeploymentReporter<'a> {
    paths: &'a DevSyncPaths,
}

impl<'a> PersistedDeploymentReporter<'a> {
    pub(crate) fn new(paths: &'a DevSyncPaths) -> Self {
        Self { paths }
    }
}

impl DeploymentReporter for PersistedDeploymentReporter<'_> {
    fn report(&self, workspace_id: &str, state: DeploymentState, message: &str) {
        let state_name = serde_json::to_string(&state)
            .unwrap_or_else(|_| "\"unknown\"".into())
            .trim_matches('"')
            .to_string();
        let _ =
            workspace::record_deployment_update_at(self.paths, workspace_id, &state_name, message);
    }
}

#[tauri::command]
pub async fn deploy_devsync_workspace(
    app: AppHandle,
    window: WebviewWindow,
    workspace_id: String,
) -> Result<DeploymentResult, DevSyncError> {
    let paths = DevSyncPaths::from_app(&app)?;
    let reporter = TauriDeploymentReporter {
        window: &window,
        paths: &paths,
    };
    deploy_workspace(&paths, &workspace_id, &reporter).await
}

pub async fn deploy_workspace(
    paths: &DevSyncPaths,
    workspace_id: &str,
    reporter: &dyn DeploymentReporter,
) -> Result<DeploymentResult, DevSyncError> {
    deploy_workspace_with_options(paths, workspace_id, reporter, false).await
}

pub async fn deploy_workspace_for_signing_refresh(
    paths: &DevSyncPaths,
    workspace_id: &str,
    reporter: &dyn DeploymentReporter,
) -> Result<DeploymentResult, DevSyncError> {
    deploy_workspace_with_options(paths, workspace_id, reporter, true).await
}

async fn deploy_workspace_with_options(
    paths: &DevSyncPaths,
    workspace_id: &str,
    reporter: &dyn DeploymentReporter,
    force_signing_refresh: bool,
) -> Result<DeploymentResult, DevSyncError> {
    let _guard = build::acquire_workspace_operation(paths, workspace_id)?;
    reporter.report(
        workspace_id,
        DeploymentState::Preparing,
        "Preparing deployment",
    );
    let state = workspace::load_store_at(paths)?;
    let selected = state
        .workspaces
        .iter()
        .find(|workspace| workspace.id == workspace_id)
        .cloned()
        .ok_or_else(|| DevSyncError::new("workspace_not_found", "Workspace no longer exists."))?;
    let force_signing_refresh = force_signing_refresh
        || matches!(
            RefreshPolicy::default().evaluate(selected.signing_status.as_ref()),
            RefreshDecision::RefreshNeeded | RefreshDecision::RefreshUrgently
        );
    let built_source_revision = selected.source_revision;
    let build_result = build::persist_build_result(
        paths,
        workspace_id,
        if force_signing_refresh {
            build::build_workspace_for_signing_refresh(paths, selected, |progress| {
                let (state, message) = match progress {
                    build::BuildProgress::PreBuilding => {
                        (DeploymentState::PreBuilding, "Pre-Build")
                    }
                    build::BuildProgress::PreBuildSucceeded => {
                        (DeploymentState::PreBuildSucceeded, "Pre-Build ✓")
                    }
                    build::BuildProgress::XcodeBuilding => {
                        (DeploymentState::Building, "Xcode Build")
                    }
                };
                reporter.report(workspace_id, state, message);
            })
            .await?
        } else {
            build::build_workspace_with_progress(paths, selected, |progress| {
                let (state, message) = match progress {
                    build::BuildProgress::PreBuilding => {
                        (DeploymentState::PreBuilding, "Pre-Build")
                    }
                    build::BuildProgress::PreBuildSucceeded => {
                        (DeploymentState::PreBuildSucceeded, "Pre-Build ✓")
                    }
                    build::BuildProgress::XcodeBuilding => {
                        (DeploymentState::Building, "Xcode Build")
                    }
                };
                reporter.report(workspace_id, state, message);
            })
            .await?
        },
    )?;
    if !build_result.succeeded {
        reporter.report(workspace_id, DeploymentState::BuildFailed, "Build failed");
        return Ok(DeploymentResult {
            workspace: build_result.workspace,
            state: DeploymentState::BuildFailed,
            build_log: build_result.log,
            install_log: None,
            diagnostic: build_result.diagnostic,
        });
    }
    reporter.report(
        workspace_id,
        DeploymentState::BuildSucceeded,
        "Xcode Build ✓",
    );
    reporter.report(workspace_id, DeploymentState::Signing, "Signing verified ✓");
    reporter.report(
        workspace_id,
        DeploymentState::WaitingForDevice,
        "Finding iPhone",
    );
    let waiting_for_device = |diagnostic: String| {
        reporter.report(
            workspace_id,
            DeploymentState::WaitingForDevice,
            diagnostic.as_str(),
        );
        DeploymentResult {
            workspace: build_result.workspace.clone(),
            state: DeploymentState::WaitingForDevice,
            build_log: build_result.log.clone(),
            install_log: None,
            diagnostic: Some(diagnostic),
        }
    };
    let devices = match device::list_devices().await {
        Ok(devices) => devices,
        Err(error) => {
            return Ok(waiting_for_device(format!(
                "CoreDevice could not find a reachable iPhone: {}",
                error.message
            )));
        }
    };
    let selection = device::load_selection_at(paths)?;
    let device = if let Some(selected_id) = selection.selected_device_id.as_deref() {
        match devices.iter().find(|candidate| candidate.id == selected_id) {
            Some(candidate) if device::is_connected(candidate) => candidate.clone(),
            Some(candidate) if candidate.connection_state == "Unknown" => {
                let mut verified = candidate.clone();
                match device::probe_device(&verified).await {
                    Ok(()) => {
                        verified.connection_state = "Connected".into();
                        verified
                    }
                    Err(error) => {
                        return Ok(waiting_for_device(format!(
                            "The selected iPhone is not reachable: {}",
                            error.message
                        )));
                    }
                }
            }
            Some(candidate) => {
                return Ok(waiting_for_device(format!(
                    "The selected iPhone is not reachable ({}).",
                    candidate.connection_state
                )));
            }
            None => {
                return Ok(waiting_for_device(
                    "The selected iPhone is currently unavailable.".into(),
                ));
            }
        }
    } else {
        match devices.as_slice() {
            [candidate] if device::is_connected(candidate) => {
                device::remember_device_at(paths, candidate).map(|_| candidate.clone())?
            }
            [candidate] if candidate.connection_state == "Unknown" => {
                let mut verified = candidate.clone();
                match device::probe_device(&verified).await {
                    Ok(()) => {
                        verified.connection_state = "Connected".into();
                        device::remember_device_at(paths, &verified).map(|_| verified.clone())?
                    }
                    Err(_) => {
                        return Ok(waiting_for_device(
                            "The paired iPhone is not currently reachable.".into(),
                        ));
                    }
                }
            }
            [_] => {
                return Ok(waiting_for_device(
                    "The paired iPhone is not currently reachable.".into(),
                ));
            }
            _ => {
                return Ok(waiting_for_device(
                    "Select a paired iPhone before installing.".into(),
                ));
            }
        }
    };
    reporter.report(workspace_id, DeploymentState::Installing, "Installing");
    let mut install_result =
        install::install_latest_artifact(&build_result.workspace, &device).await?;
    let mut waiting_reason = None;
    if !install_result.succeeded
        && install::is_transient_device_error(install_result.diagnostic.as_deref())
    {
        reporter.report(
            workspace_id,
            DeploymentState::WaitingForDevice,
            "Reconnecting to iPhone",
        );
        match device::list_devices().await {
            Ok(refreshed) => {
                if let Some(refreshed_device) =
                    refreshed.iter().find(|candidate| candidate.id == device.id)
                {
                    let mut verified = refreshed_device.clone();
                    let reachable = device::is_connected(&verified)
                        || device::probe_device(&verified).await.is_ok();
                    if reachable {
                        verified.connection_state = "Connected".into();
                        reporter.report(
                            workspace_id,
                            DeploymentState::Installing,
                            "Retrying install",
                        );
                        install_result =
                            install::install_latest_artifact(&build_result.workspace, &verified)
                                .await?;
                        if !install_result.succeeded
                            && install::is_transient_device_error(
                                install_result.diagnostic.as_deref(),
                            )
                        {
                            waiting_reason = install_result.diagnostic.clone();
                        }
                    } else {
                        waiting_reason = Some(
                            "The selected iPhone is no longer reachable. Reconnect it and try again."
                                .into(),
                        );
                    }
                } else {
                    waiting_reason = Some(
                        "The selected iPhone is no longer reachable. Reconnect it and try again."
                            .into(),
                    );
                }
            }
            Err(error) => {
                waiting_reason = Some(format!(
                    "CoreDevice is unavailable while reconnecting to the iPhone: {}",
                    error.message
                ));
            }
        }
    }
    let workspace = persist_install_result(
        paths,
        workspace_id,
        &device.id,
        built_source_revision,
        install_result.clone(),
    )?;
    if install_result.succeeded {
        reporter.report(workspace_id, DeploymentState::Installed, "Completed");
        Ok(DeploymentResult {
            workspace,
            state: DeploymentState::Installed,
            build_log: build_result.log,
            install_log: Some(install_result.log),
            diagnostic: None,
        })
    } else if let Some(diagnostic) = waiting_reason {
        reporter.report(
            workspace_id,
            DeploymentState::WaitingForDevice,
            diagnostic.as_str(),
        );
        Ok(DeploymentResult {
            workspace,
            state: DeploymentState::WaitingForDevice,
            build_log: build_result.log,
            install_log: Some(install_result.log),
            diagnostic: Some(diagnostic),
        })
    } else {
        reporter.report(
            workspace_id,
            DeploymentState::InstallFailed,
            "Installation failed",
        );
        Ok(DeploymentResult {
            workspace,
            state: DeploymentState::InstallFailed,
            build_log: build_result.log,
            install_log: Some(install_result.log),
            diagnostic: install_result.diagnostic,
        })
    }
}

fn persist_install_result(
    paths: &DevSyncPaths,
    workspace_id: &str,
    device_id: &str,
    built_source_revision: u64,
    result: InstallResult,
) -> Result<Workspace, DevSyncError> {
    let mut state = workspace::load_store_at(paths)?;
    let workspace = workspace::workspace_mut(&mut state, workspace_id)?;
    workspace.last_install_status = Some(
        if result.succeeded {
            "installed"
        } else {
            "failed"
        }
        .into(),
    );
    workspace.last_install_at = result.installed_at;
    workspace.last_install_device_id = Some(device_id.into());
    workspace.last_installed_artifact_path = result.succeeded.then_some(result.artifact_path);
    workspace.last_install_log_path = Some(result.log_path);
    if result.succeeded {
        workspace.deployed_revision = built_source_revision;
        workspace.changes_detected = workspace.source_revision != built_source_revision;
    }
    workspace.updated_at = workspace::now();
    let result = workspace.clone();
    workspace::save_store_at(paths, &state)?;
    Ok(result)
}

#[allow(dead_code)] // Exercised by unit tests and available for scheduler transition checks.
pub fn valid_transition(from: &DeploymentState, to: &DeploymentState) -> bool {
    matches!(
        (from, to),
        (DeploymentState::Idle, DeploymentState::Preparing)
            | (DeploymentState::Preparing, DeploymentState::PreBuilding)
            | (
                DeploymentState::PreBuilding,
                DeploymentState::PreBuildSucceeded
            )
            | (
                DeploymentState::PreBuildSucceeded,
                DeploymentState::Building
            )
            | (DeploymentState::Preparing, DeploymentState::Building)
            | (DeploymentState::Building, DeploymentState::BuildSucceeded)
            | (DeploymentState::BuildSucceeded, DeploymentState::Signing)
            | (DeploymentState::Signing, DeploymentState::WaitingForDevice)
            | (DeploymentState::Building, DeploymentState::BuildFailed)
            | (
                DeploymentState::BuildSucceeded,
                DeploymentState::WaitingForDevice
            )
            | (
                DeploymentState::WaitingForDevice,
                DeploymentState::Installing
            )
            | (DeploymentState::Installing, DeploymentState::Installed)
            | (DeploymentState::Installing, DeploymentState::InstallFailed)
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn accepts_the_build_to_install_flow_and_rejects_skips() {
        assert!(valid_transition(
            &DeploymentState::Building,
            &DeploymentState::BuildSucceeded
        ));
        assert!(valid_transition(
            &DeploymentState::Installing,
            &DeploymentState::Installed
        ));
        assert!(!valid_transition(
            &DeploymentState::Idle,
            &DeploymentState::Installing
        ));
    }
}
