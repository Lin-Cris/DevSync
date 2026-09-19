use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct XcodeContainer {
    pub path: String,
    pub container_type: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct XcodeMetadata {
    pub schemes: Vec<String>,
    pub configurations: Vec<String>,
    pub targets: Vec<String>,
    pub product_name: Option<String>,
    pub bundle_identifier: Option<String>,
    pub sdk_root: Option<String>,
    pub supported_platforms: Option<String>,
    pub development_team: Option<String>,
    pub code_sign_style: Option<String>,
    pub product_type: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Workspace {
    pub id: String,
    pub folder_path: String,
    pub display_name: String,
    pub created_at: String,
    pub updated_at: String,
    pub xcode_container_path: Option<String>,
    pub container_type: Option<String>,
    pub selected_scheme: Option<String>,
    /// A user-supplied command executed in this workspace before every Xcode build.
    /// DevSync deliberately does not infer this from the project's source files.
    #[serde(default)]
    pub pre_build_command: Option<String>,
    #[serde(default)]
    pub pre_build_working_directory: Option<String>,
    /// `None` is a safe migration path for an existing configured command: it
    /// remains enabled until the user explicitly disables it.
    #[serde(default)]
    pub pre_build_enabled: Option<bool>,
    pub product_name: Option<String>,
    pub bundle_identifier: Option<String>,
    pub build_configuration: Option<String>,
    pub signing_team: Option<String>,
    pub signing_status: Option<SigningStatus>,
    #[serde(default)]
    pub auto_sync: bool,
    #[serde(default)]
    pub changes_detected: bool,
    #[serde(default)]
    pub source_revision: u64,
    #[serde(default)]
    pub deployed_revision: u64,
    #[serde(default)]
    pub background_state: Option<String>,
    #[serde(default)]
    pub deployment_state: Option<String>,
    #[serde(default)]
    pub deployment_message: Option<String>,
    #[serde(default)]
    pub activities: Vec<ActivityEntry>,
    pub last_build_status: Option<String>,
    pub last_build_at: Option<String>,
    pub last_artifact_path: Option<String>,
    pub last_build_log_path: Option<String>,
    #[serde(default)]
    pub last_artifact_fingerprint_path: Option<String>,
    pub last_install_status: Option<String>,
    pub last_install_at: Option<String>,
    pub last_install_device_id: Option<String>,
    pub last_installed_artifact_path: Option<String>,
    pub last_install_log_path: Option<String>,
    pub metadata_error: Option<String>,
    pub unavailable: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ActivityEntry {
    pub id: String,
    pub kind: String,
    pub message: String,
    pub timestamp: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SigningStatus {
    pub team_identifier: Option<String>,
    pub profile_uuid: Option<String>,
    pub application_identifier: Option<String>,
    pub profile_name: Option<String>,
    pub expiration_date: Option<String>,
    pub remaining_seconds: Option<i64>,
    /// Added after the first persisted workspace format. Keep old stores
    /// readable so a missing inspection timestamp cannot hide all workspaces.
    #[serde(default)]
    pub last_inspected_at: String,
    pub status: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceInspection {
    pub workspace: Workspace,
    pub containers: Vec<XcodeContainer>,
    pub metadata: Option<XcodeMetadata>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceStore {
    pub schema_version: u32,
    pub workspaces: Vec<Workspace>,
}

impl Default for WorkspaceStore {
    fn default() -> Self {
        Self {
            schema_version: 1,
            workspaces: vec![],
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BuildResult {
    pub workspace: Workspace,
    pub succeeded: bool,
    pub log: String,
    pub diagnostic: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DevSyncDeviceInfo {
    pub id: String,
    pub name: String,
    pub model: Option<String>,
    pub os_version: Option<String>,
    pub connection_state: String,
    pub connection_type: Option<String>,
    pub last_seen: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct DeviceSelection {
    pub schema_version: u32,
    pub selected_device_id: Option<String>,
    pub selected_device_name: Option<String>,
    pub updated_at: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InstallResult {
    pub succeeded: bool,
    pub log: String,
    pub diagnostic: Option<String>,
    pub installed_at: Option<String>,
    pub artifact_path: String,
    pub log_path: String,
}

#[allow(dead_code)] // Retained as the persisted/UI initial deployment state.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum DeploymentState {
    Idle,
    Preparing,
    PreBuilding,
    PreBuildSucceeded,
    Building,
    BuildFailed,
    BuildSucceeded,
    Signing,
    WaitingForDevice,
    Installing,
    InstallFailed,
    Installed,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeploymentResult {
    pub workspace: Workspace,
    pub state: DeploymentState,
    pub build_log: String,
    pub install_log: Option<String>,
    pub diagnostic: Option<String>,
}
