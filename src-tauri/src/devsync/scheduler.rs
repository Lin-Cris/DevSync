use std::time::Duration;

use tauri::{AppHandle, Manager};
use tokio::time::interval;

use super::{
    DevSyncError, agent, deployment,
    deployment::{DeploymentReporter, PersistedDeploymentReporter},
    device,
    paths::DevSyncPaths,
    refresh_policy::{RefreshDecision, RefreshPolicy},
    workspace,
};

const SCHEDULER_INTERVAL: Duration = Duration::from_secs(30 * 60);

/// Evaluates persisted workspaces on a conservative cadence. Deployments reuse
/// the coordinator's per-workspace operation guard, so manual and background
/// refreshes cannot race one another.
pub fn start_agent(paths: DevSyncPaths) {
    tokio::spawn(async move {
        let mut timer = interval(SCHEDULER_INTERVAL);
        timer.tick().await;
        loop {
            if let Err(error) = reconcile_agent(&paths).await {
                agent::record_error(&paths, &error.message);
                agent::log(&paths, &format!("reconcile failed: {}", error.message));
            }
            timer.tick().await;
        }
    });
}

pub async fn reconcile(app: &AppHandle) -> Result<(), DevSyncError> {
    let paths = DevSyncPaths::from_app(app)?;
    let Some(window) = app.get_webview_window("main") else {
        return Ok(());
    };
    let reporter = deployment::TauriDeploymentReporter::new(&window, &paths);
    reconcile_with_reporter(&paths, &reporter, false).await
}

pub async fn reconcile_agent(paths: &DevSyncPaths) -> Result<(), DevSyncError> {
    agent::log(paths, "signing renewal check");
    let reporter = PersistedDeploymentReporter::new(&paths);
    reconcile_with_reporter(paths, &reporter, true).await
}

async fn reconcile_with_reporter(
    paths: &DevSyncPaths,
    reporter: &dyn DeploymentReporter,
    is_agent: bool,
) -> Result<(), DevSyncError> {
    let policy = RefreshPolicy::default();
    let store = workspace::load_store_at(paths)?;
    let active_workspace_id = workspace::active_workspace_id(&store);
    let workspaces = store.workspaces;
    // Signing renewal is a build operation. Device discovery is only needed
    // before a source sync that must be installed immediately; doing it here
    // for every workspace used to make a CoreDevice timeout prevent Xcode
    // from refreshing an otherwise valid provisioning profile.
    let requires_device_preflight = workspaces.iter().any(|item| {
        if active_workspace_id.as_deref() != Some(item.id.as_str()) {
            return false;
        }
        if item.unavailable || item.metadata_error.is_some() || item.selected_scheme.is_none() {
            return false;
        }
        let signing_pending =
            signing_refresh_pending(policy.evaluate(item.signing_status.as_ref()));
        requires_device_preflight(item.auto_sync && item.changes_detected, signing_pending)
    });
    let selected_available = if requires_device_preflight {
        let selection = device::load_selection_at(paths)?;
        let devices = match device::list_devices().await {
            Ok(devices) => devices,
            Err(error) => {
                if is_agent {
                    agent::record_error(paths, &error.message);
                    agent::log(
                        paths,
                        &format!("device discovery failed: {}", error.message),
                    );
                }
                return Err(error);
            }
        };
        Some(if let Some(id) = selection.selected_device_id.as_deref() {
            if let Some(candidate) = devices.iter().find(|candidate| candidate.id == id) {
                device::is_connected(candidate)
                    || (candidate.connection_state == "Unknown"
                        && device::probe_device(candidate).await.is_ok())
            } else {
                false
            }
        } else {
            false
        })
    } else {
        None
    };

    for item in workspaces {
        let is_active = active_workspace_id.as_deref() == Some(item.id.as_str());
        if item.unavailable || item.metadata_error.is_some() || item.selected_scheme.is_none() {
            if !is_active {
                continue;
            }
            set_state(paths, &item.id, "needsAttention")?;
            continue;
        }
        let signing_decision = policy.evaluate(item.signing_status.as_ref());
        // Source Auto Sync is scoped to the active project. Signing renewal
        // remains per-project so saved projects keep their signing metadata
        // healthy while the user works on another project.
        let source_pending = source_sync_pending(is_active, item.auto_sync, item.changes_detected);
        let signing_pending = signing_refresh_pending(signing_decision);
        if !source_pending && !signing_pending {
            if !is_active {
                continue;
            }
            set_state(
                paths,
                &item.id,
                if item.changes_detected {
                    "changesDetected"
                } else if signing_decision == RefreshDecision::InspectRequired {
                    "needsAttention"
                } else {
                    "healthy"
                },
            )?;
            continue;
        }
        if source_pending && !signing_pending && selected_available != Some(true) {
            set_state(paths, &item.id, "waitingForDevice")?;
            continue;
        }
        set_state(paths, &item.id, "queued")?;
        if is_agent {
            agent::set_activity(paths, Some("deployment started"));
            agent::log(paths, &format!("deployment started: {}", item.id));
        }
        let result = if signing_pending {
            deployment::deploy_workspace_for_signing_refresh(paths, &item.id, reporter).await
        } else {
            deployment::deploy_workspace(paths, &item.id, reporter).await
        };
        match result {
            Ok(result) => {
                let state = if result.state == super::models::DeploymentState::Installed {
                    if is_agent {
                        agent::set_activity(paths, None);
                        agent::clear_error(paths);
                        agent::set_last_successful_sync(paths);
                        agent::log(paths, &format!("deployment succeeded: {}", item.id));
                    }
                    "refreshed"
                } else if result.state == super::models::DeploymentState::WaitingForDevice {
                    if is_agent {
                        agent::set_activity(paths, None);
                        agent::log(
                            paths,
                            &format!("deployment waiting for device: {}", item.id),
                        );
                    }
                    "waitingForDevice"
                } else {
                    if is_agent {
                        agent::set_activity(paths, None);
                        agent::record_error(
                            paths,
                            result.diagnostic.as_deref().unwrap_or("deployment failed"),
                        );
                        agent::log(paths, &format!("deployment failed: {}", item.id));
                    }
                    "failed"
                };
                set_state(paths, &item.id, state)?;
            }
            Err(error) => {
                if is_agent && error.code == "operation_in_progress" {
                    agent::log(
                        paths,
                        &format!(
                            "deployment already in progress, keeping queued: {}",
                            item.id
                        ),
                    );
                    set_state(paths, &item.id, "queued")?;
                    continue;
                }
                if is_agent {
                    agent::set_activity(paths, None);
                    agent::record_error(paths, &error.message);
                    agent::log(
                        paths,
                        &format!("deployment failed: {} ({})", item.id, error),
                    );
                }
                set_state(paths, &item.id, "failed")?;
            }
        }
    }
    Ok(())
}

fn set_state(paths: &DevSyncPaths, workspace_id: &str, state: &str) -> Result<(), DevSyncError> {
    let mut store = workspace::load_store_at(paths)?;
    let workspace = workspace::workspace_mut(&mut store, workspace_id)?;
    workspace.background_state = Some(state.into());
    workspace.updated_at = workspace::now();
    workspace::save_store_at(paths, &store)
}

fn requires_device_preflight(source_pending: bool, signing_pending: bool) -> bool {
    source_pending && !signing_pending
}

fn source_sync_pending(is_active: bool, auto_sync: bool, changes_detected: bool) -> bool {
    is_active && auto_sync && changes_detected
}

fn signing_refresh_pending(decision: RefreshDecision) -> bool {
    matches!(
        decision,
        RefreshDecision::RefreshNeeded
            | RefreshDecision::RefreshUrgently
            | RefreshDecision::InspectRequired
    )
}

#[cfg(test)]
mod tests {
    use super::{
        SCHEDULER_INTERVAL, requires_device_preflight, signing_refresh_pending, source_sync_pending,
    };
    use crate::devsync::refresh_policy::RefreshDecision;

    #[test]
    fn uses_a_conservative_cadence() {
        assert!(SCHEDULER_INTERVAL >= std::time::Duration::from_secs(15 * 60));
    }

    #[test]
    fn signing_refresh_is_not_blocked_by_device_preflight() {
        assert!(!requires_device_preflight(true, true));
        assert!(requires_device_preflight(true, false));
        assert!(!requires_device_preflight(false, true));
    }

    #[test]
    fn unknown_signing_metadata_starts_an_inspection_build() {
        assert!(signing_refresh_pending(RefreshDecision::InspectRequired));
        assert!(signing_refresh_pending(RefreshDecision::RefreshNeeded));
        assert!(signing_refresh_pending(RefreshDecision::RefreshUrgently));
        assert!(!signing_refresh_pending(RefreshDecision::NoAction));
    }

    #[test]
    fn source_auto_sync_only_runs_for_the_active_workspace() {
        assert!(source_sync_pending(true, true, true));
        assert!(!source_sync_pending(false, true, true));
        assert!(!source_sync_pending(true, false, true));
    }
}
