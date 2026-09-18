import { invoke } from "@tauri-apps/api/core";
import type { BackgroundServiceStatus, BuildCacheUsage, BuildResult, DeploymentResult, DeviceSelection, DevSyncDevice, Workspace, WorkspaceInspection, XcodeContainer } from "./types";

export const devsyncApi = {
  listWorkspaces: () => invoke<Workspace[]>("list_devsync_workspaces"),
  addWorkspace: (folderPath: string) => invoke<WorkspaceInspection>("add_devsync_workspace", { folderPath }),
  refreshWorkspace: (workspaceId: string) => invoke<WorkspaceInspection>("refresh_devsync_workspace", { workspaceId }),
  removeWorkspace: (workspaceId: string) => invoke<void>("remove_devsync_workspace", { workspaceId }),
  selectContainer: (workspaceId: string, container: XcodeContainer) => invoke<WorkspaceInspection>("select_devsync_container", { workspaceId, container }),
  selectScheme: (workspaceId: string, scheme: string) => invoke<WorkspaceInspection>("select_devsync_scheme", { workspaceId, scheme }),
  buildWorkspace: (workspaceId: string) => invoke<BuildResult>("build_devsync_workspace", { workspaceId }),
  listDevices: () => invoke<DevSyncDevice[]>("list_devsync_devices"),
  getDeviceSelection: () => invoke<DeviceSelection>("get_devsync_device_selection"),
  selectDevice: (device: DevSyncDevice) => invoke<DeviceSelection>("select_devsync_device", { device }),
  setAutoSync: (workspaceId: string, enabled: boolean) => invoke<Workspace>("set_devsync_auto_sync", { workspaceId, enabled }),
  setPreBuildCommand: (workspaceId: string, command: string) => invoke<Workspace>("set_devsync_pre_build_command", { workspaceId, command }),
  getLaunchAtLogin: () => invoke<boolean>("get_devsync_launch_at_login"),
  setLaunchAtLogin: (enabled: boolean) => invoke<boolean>("set_devsync_launch_at_login", { enabled }),
  deployWorkspace: (workspaceId: string) => invoke<DeploymentResult>("deploy_devsync_workspace", { workspaceId }),
  getBuildCacheUsage: () => invoke<BuildCacheUsage>("get_devsync_build_cache_usage"),
  cleanBuildCache: () => invoke<BuildCacheUsage>("clean_devsync_build_cache"),
  getBackgroundServiceStatus: () => invoke<BackgroundServiceStatus>("get_background_service_status"),
  enableBackgroundService: () => invoke<BackgroundServiceStatus>("enable_background_service"),
  disableBackgroundService: () => invoke<BackgroundServiceStatus>("disable_background_service"),
};
