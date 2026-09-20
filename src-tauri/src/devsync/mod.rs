mod agent;
mod build;
mod deployment;
mod device;
mod install;
mod locking;
mod models;
mod paths;
mod preferences;
mod refresh_policy;
mod scheduler;
mod signing;
pub(crate) mod watcher;
mod workspace;
mod xcode;

use serde::Serialize;

pub use background_service::{
    disable_background_service, enable_background_service, get_background_service_status,
};
pub use build::build_devsync_workspace;
pub use deployment::deploy_devsync_workspace;
pub use device::{get_devsync_device_selection, list_devsync_devices, select_devsync_device};
pub use preferences::{get_devsync_launch_at_login, set_devsync_launch_at_login};
pub use scheduler::reconcile as reconcile_refresh_scheduler;
pub use workspace::{
    add_devsync_workspace, clean_devsync_build_cache, clear_devsync_activity,
    get_devsync_active_workspace, get_devsync_build_cache_usage, list_devsync_workspaces,
    refresh_devsync_workspace, remove_devsync_workspace, select_devsync_container,
    select_devsync_scheme, select_devsync_workspace, set_devsync_auto_sync,
    set_devsync_pre_build_command,
};

mod background_service;

pub fn run_agent() -> Result<(), DevSyncError> {
    agent::run()
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DevSyncError {
    pub code: String,
    pub message: String,
}

impl DevSyncError {
    pub fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
        }
    }
}

impl std::fmt::Display for DevSyncError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}", self.message)
    }
}

impl std::error::Error for DevSyncError {}
