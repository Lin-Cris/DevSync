use std::{
    env, fs,
    path::{Path, PathBuf},
};

use plist::Value;
use tauri::{AppHandle, Manager};

use super::DevSyncError;

#[derive(Debug, Clone)]
pub struct DevSyncPaths {
    pub app_data_dir: PathBuf,
    pub data_dir: PathBuf,
}

impl DevSyncPaths {
    pub fn from_app(app: &AppHandle) -> Result<Self, DevSyncError> {
        let app_data_dir = app.path().app_data_dir().map_err(|error| {
            DevSyncError::new(
                "storage_error",
                format!("Unable to locate DevSync app data: {error}"),
            )
        })?;
        Ok(Self::from_app_data_dir(app_data_dir))
    }

    pub fn from_agent() -> Result<Self, DevSyncError> {
        let app_data_dir = env::var_os("DEVSYNC_APP_DATA_DIR")
            .map(PathBuf::from)
            .or_else(default_agent_app_data_dir)
            .ok_or_else(|| {
                DevSyncError::new(
                    "storage_error",
                    "Unable to determine the DevSync application data directory.",
                )
            })?;
        let data_dir = env::var_os("DEVSYNC_DATA_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| data_dir_for(&app_data_dir));
        Ok(Self {
            app_data_dir,
            data_dir,
        })
    }

    pub fn from_app_data_dir(app_data_dir: PathBuf) -> Self {
        let data_dir = data_dir_for(&app_data_dir);
        Self {
            app_data_dir,
            data_dir,
        }
    }

    #[cfg(test)]
    pub fn for_test(root: &Path) -> Self {
        Self {
            app_data_dir: root.join("app-data"),
            data_dir: root.join("devsync"),
        }
    }

    pub fn workspace_store_path(&self) -> PathBuf {
        self.app_data_dir.join("devsync-workspaces.json")
    }

    pub fn workspace_store_lock_path(&self) -> PathBuf {
        self.data_dir.join("locks/workspaces.lock")
    }

    pub fn device_store_path(&self) -> PathBuf {
        self.app_data_dir.join("devsync-devices.json")
    }

    pub fn device_store_lock_path(&self) -> PathBuf {
        self.data_dir.join("locks/devices.lock")
    }

    pub fn derived_data_path(&self, workspace_id: &str) -> PathBuf {
        self.data_dir.join("DerivedData").join(workspace_id)
    }

    pub fn operation_lock_path(&self, workspace_id: &str) -> PathBuf {
        self.data_dir
            .join("locks/workspaces")
            .join(format!("{workspace_id}.lock"))
    }

    pub fn suppression_path(&self, workspace_id: &str) -> PathBuf {
        self.data_dir
            .join("locks/suppression")
            .join(format!("{workspace_id}.until"))
    }

    pub fn agent_lock_path(&self) -> PathBuf {
        self.data_dir.join("agent.lock")
    }

    pub fn agent_state_path(&self) -> PathBuf {
        self.data_dir.join("agent-state.json")
    }

    pub fn agent_log_path(&self) -> PathBuf {
        self.data_dir.join("logs/devsync-agent.log")
    }
}

fn data_dir_for(app_data_dir: &Path) -> PathBuf {
    app_data_dir
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| app_data_dir.to_path_buf())
        .join("DevSync")
}

fn default_agent_app_data_dir() -> Option<PathBuf> {
    let home = env::var_os("HOME").map(PathBuf::from)?;
    let identifier = current_bundle_identifier().unwrap_or_else(|| "me.nabdev.iloader".into());
    #[cfg(target_os = "macos")]
    {
        Some(home.join("Library/Application Support").join(identifier))
    }
    #[cfg(not(target_os = "macos"))]
    {
        Some(home.join(".local/share").join(identifier))
    }
}

fn current_bundle_identifier() -> Option<String> {
    let executable = env::current_exe().ok()?;
    let bundle = executable
        .parent()?
        .parent()?
        .parent()?
        .join("Contents/Info.plist");
    let value = Value::from_file(bundle).ok()?;
    value
        .as_dictionary()?
        .get("CFBundleIdentifier")?
        .as_string()
        .map(str::to_string)
}

pub fn ensure_directory(path: &Path) -> Result<(), DevSyncError> {
    fs::create_dir_all(path).map_err(|error| {
        DevSyncError::new(
            "filesystem_error",
            format!("Unable to create {}: {error}", path.display()),
        )
    })
}
