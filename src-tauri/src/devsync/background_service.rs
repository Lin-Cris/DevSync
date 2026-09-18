use serde::Serialize;
use tauri::AppHandle;

use super::{DevSyncError, agent, paths::DevSyncPaths};

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BackgroundServiceStatus {
    pub enabled: bool,
    pub running: bool,
    pub pid: Option<u32>,
    pub agent_path: Option<String>,
    pub last_error: Option<String>,
    pub state: String,
}

#[tauri::command]
pub fn get_background_service_status(
    app: AppHandle,
) -> Result<BackgroundServiceStatus, DevSyncError> {
    #[cfg(target_os = "macos")]
    {
        return macos::status(&app);
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = app;
        Ok(unsupported_status())
    }
}

#[tauri::command]
pub fn enable_background_service(app: AppHandle) -> Result<BackgroundServiceStatus, DevSyncError> {
    #[cfg(target_os = "macos")]
    {
        return macos::enable(&app);
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = app;
        Err(DevSyncError::new(
            "unsupported_platform",
            "Background Service is currently supported only on macOS.",
        ))
    }
}

#[tauri::command]
pub fn disable_background_service(app: AppHandle) -> Result<BackgroundServiceStatus, DevSyncError> {
    #[cfg(target_os = "macos")]
    {
        return macos::disable(&app);
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = app;
        Ok(unsupported_status())
    }
}

#[cfg(not(target_os = "macos"))]
fn unsupported_status() -> BackgroundServiceStatus {
    BackgroundServiceStatus {
        enabled: false,
        running: false,
        pid: None,
        agent_path: None,
        last_error: Some("Background Service is currently supported only on macOS.".into()),
        state: "error".into(),
    }
}

#[cfg(target_os = "macos")]
mod macos {
    use std::{
        fs::{self, OpenOptions},
        io::BufWriter,
        os::unix::fs::PermissionsExt,
        path::{Path, PathBuf},
        process::{Command, Output},
    };

    use plist::Value;
    use tauri::{AppHandle, Manager};

    use super::*;

    struct ServiceInfo {
        label: String,
        domain: String,
        plist_path: PathBuf,
        agent_path: PathBuf,
        paths: DevSyncPaths,
    }

    pub fn status(app: &AppHandle) -> Result<BackgroundServiceStatus, DevSyncError> {
        let paths = DevSyncPaths::from_app(app)?;
        let label = service_label(app);
        let domain = launch_domain()?;
        let job = print_job(&domain, &label);
        let registered = job.is_some();
        let (running, pid) = job
            .map(|output| parse_running(&output))
            .unwrap_or((false, None));
        let agent_path = find_agent_path().ok();
        let last_error = agent::read_state(&paths).and_then(|state| state.last_error);
        Ok(BackgroundServiceStatus {
            enabled: registered,
            running,
            pid,
            agent_path: agent_path.map(|path| path.to_string_lossy().to_string()),
            last_error: last_error.clone(),
            state: if last_error.is_some() && !running {
                "error".into()
            } else if running {
                "running".into()
            } else if registered {
                "starting".into()
            } else {
                "stopped".into()
            },
        })
    }

    pub fn enable(app: &AppHandle) -> Result<BackgroundServiceStatus, DevSyncError> {
        let info = service_info(app)?;
        let current = status(app)?;
        write_plist(&info)?;
        if current.enabled {
            kickstart(&info)?;
        } else {
            let bootstrapped = launchctl([
                "bootstrap",
                &info.domain,
                &info.plist_path.to_string_lossy(),
            ]);
            if let Err(error) = bootstrapped {
                if print_job(&info.domain, &info.label).is_none() {
                    return Err(error);
                }
            }
        }
        status(app)
    }

    pub fn disable(app: &AppHandle) -> Result<BackgroundServiceStatus, DevSyncError> {
        let info = service_info(app)?;
        let target = format!("{}/{}", info.domain, info.label);
        let output = launchctl_output(["bootout", &target]);
        if let Err(error) = output {
            if print_job(&info.domain, &info.label).is_some() {
                return Err(error);
            }
        }
        if info.plist_path.exists() {
            fs::remove_file(&info.plist_path).map_err(|error| {
                DevSyncError::new(
                    "background_service_error",
                    format!("Unable to remove LaunchAgent plist: {error}"),
                )
            })?;
        }
        Ok(BackgroundServiceStatus {
            enabled: false,
            running: false,
            pid: None,
            agent_path: Some(info.agent_path.to_string_lossy().to_string()),
            last_error: None,
            state: "stopped".into(),
        })
    }

    fn service_info(app: &AppHandle) -> Result<ServiceInfo, DevSyncError> {
        let paths = DevSyncPaths::from_app(app)?;
        let home = app.path().home_dir().map_err(|error| {
            DevSyncError::new(
                "background_service_error",
                format!("Unable to locate the current user's home directory: {error}"),
            )
        })?;
        let label = service_label(app);
        Ok(ServiceInfo {
            domain: launch_domain()?,
            plist_path: home
                .join("Library/LaunchAgents")
                .join(format!("{label}.plist")),
            agent_path: find_agent_path()?,
            label,
            paths,
        })
    }

    fn service_label(app: &AppHandle) -> String {
        format!("{}.background-service", app.config().identifier)
    }

    fn launch_domain() -> Result<String, DevSyncError> {
        let uid = unsafe { libc::getuid() };
        Ok(format!("gui/{uid}"))
    }

    fn find_agent_path() -> Result<PathBuf, DevSyncError> {
        let executable = std::env::current_exe().map_err(|error| {
            DevSyncError::new(
                "background_service_error",
                format!("Unable to locate the DevSync executable: {error}"),
            )
        })?;
        let macos_dir = executable.parent().ok_or_else(|| {
            DevSyncError::new(
                "background_service_error",
                "Unable to locate the DevSync app bundle directory.",
            )
        })?;
        if macos_dir.file_name().and_then(|name| name.to_str()) != Some("MacOS")
            || macos_dir
                .parent()
                .and_then(Path::file_name)
                .and_then(|name| name.to_str())
                != Some("Contents")
        {
            return Err(DevSyncError::new(
                "background_service_unavailable",
                "Background Service is available only from a bundled DevSync.app.",
            ));
        }
        let exact = macos_dir.join("devsync-agent");
        if is_executable(&exact) {
            return Ok(exact);
        }
        let mut candidates = fs::read_dir(macos_dir)
            .map_err(|error| {
                DevSyncError::new(
                    "background_service_error",
                    format!("Unable to inspect the DevSync app bundle: {error}"),
                )
            })?
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| {
                path.file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.starts_with("devsync-agent-"))
                    && is_executable(path)
            })
            .collect::<Vec<_>>();
        candidates.sort();
        candidates.into_iter().next().ok_or_else(|| {
            DevSyncError::new(
                "background_service_unavailable",
                "The bundled devsync-agent executable was not found.",
            )
        })
    }

    fn is_executable(path: &Path) -> bool {
        path.is_file()
            && fs::metadata(path)
                .map(|metadata| metadata.permissions().mode() & 0o111 != 0)
                .unwrap_or(false)
    }

    fn write_plist(info: &ServiceInfo) -> Result<(), DevSyncError> {
        let parent = info.plist_path.parent().ok_or_else(|| {
            DevSyncError::new(
                "background_service_error",
                "Unable to determine LaunchAgent directory.",
            )
        })?;
        fs::create_dir_all(parent).map_err(|error| {
            DevSyncError::new(
                "background_service_error",
                format!("Unable to create LaunchAgents directory: {error}"),
            )
        })?;
        let stdout = info.paths.data_dir.join("logs/devsync-agent.stdout.log");
        let stderr = info.paths.data_dir.join("logs/devsync-agent.stderr.log");
        if let Some(parent) = stdout.parent() {
            fs::create_dir_all(parent).map_err(|error| {
                DevSyncError::new(
                    "background_service_error",
                    format!("Unable to create Agent log directory: {error}"),
                )
            })?;
        }
        let mut environment = plist::Dictionary::new();
        environment.insert(
            "DEVSYNC_APP_DATA_DIR".into(),
            Value::String(info.paths.app_data_dir.to_string_lossy().into()),
        );
        environment.insert(
            "DEVSYNC_DATA_DIR".into(),
            Value::String(info.paths.data_dir.to_string_lossy().into()),
        );
        let mut dictionary = plist::Dictionary::new();
        dictionary.insert("Label".into(), Value::String(info.label.clone()));
        dictionary.insert(
            "Program".into(),
            Value::String(info.agent_path.to_string_lossy().into()),
        );
        dictionary.insert(
            "ProgramArguments".into(),
            Value::Array(vec![Value::String(
                info.agent_path.to_string_lossy().into(),
            )]),
        );
        dictionary.insert("RunAtLoad".into(), Value::Boolean(true));
        dictionary.insert("KeepAlive".into(), Value::Boolean(true));
        dictionary.insert("ProcessType".into(), Value::String("Background".into()));
        dictionary.insert(
            "LimitLoadToSessionType".into(),
            Value::String("Aqua".into()),
        );
        dictionary.insert(
            "StandardOutPath".into(),
            Value::String(stdout.to_string_lossy().into()),
        );
        dictionary.insert(
            "StandardErrorPath".into(),
            Value::String(stderr.to_string_lossy().into()),
        );
        dictionary.insert(
            "EnvironmentVariables".into(),
            Value::Dictionary(environment),
        );
        let temp_path = info
            .plist_path
            .with_extension(format!("plist.{}.tmp", std::process::id()));
        let mut file = OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .open(&temp_path)
            .map_err(|error| {
                DevSyncError::new(
                    "background_service_error",
                    format!("Unable to write LaunchAgent plist: {error}"),
                )
            })?;
        file.set_permissions(fs::Permissions::from_mode(0o600)).ok();
        Value::Dictionary(dictionary)
            .to_writer_xml(BufWriter::new(&mut file))
            .map_err(|error| {
                DevSyncError::new(
                    "background_service_error",
                    format!("Unable to serialize LaunchAgent plist: {error}"),
                )
            })?;
        drop(file);
        fs::rename(temp_path, &info.plist_path).map_err(|error| {
            DevSyncError::new(
                "background_service_error",
                format!("Unable to install LaunchAgent plist: {error}"),
            )
        })
    }

    fn kickstart(info: &ServiceInfo) -> Result<(), DevSyncError> {
        let target = format!("{}/{}", info.domain, info.label);
        launchctl(["kickstart", "-k", &target])
    }

    fn print_job(domain: &str, label: &str) -> Option<String> {
        let target = format!("{domain}/{label}");
        let output = Command::new("/bin/launchctl")
            .args(["print", &target])
            .output()
            .ok()?;
        output
            .status
            .success()
            .then(|| String::from_utf8_lossy(&output.stdout).into())
    }

    fn parse_running(output: &str) -> (bool, Option<u32>) {
        let pid = output.lines().find_map(|line| {
            let value = line.trim().strip_prefix("pid = ")?;
            value.parse::<u32>().ok().filter(|pid| *pid > 0)
        });
        let running = pid.is_some() || output.lines().any(|line| line.trim() == "state = running");
        (running, pid)
    }

    fn launchctl<const N: usize>(args: [&str; N]) -> Result<(), DevSyncError> {
        let output = launchctl_output(args)?;
        if output.status.success() {
            Ok(())
        } else {
            Err(command_error(&output))
        }
    }

    fn launchctl_output<const N: usize>(args: [&str; N]) -> Result<Output, DevSyncError> {
        Command::new("/bin/launchctl")
            .args(args)
            .output()
            .map_err(|error| {
                DevSyncError::new(
                    "background_service_error",
                    format!("Unable to launch launchctl: {error}"),
                )
            })
    }

    fn command_error(output: &Output) -> DevSyncError {
        let details = String::from_utf8_lossy(&output.stderr).trim().to_string();
        DevSyncError::new(
            "background_service_error",
            if details.is_empty() {
                "launchctl rejected the Background Service operation.".into()
            } else {
                details
            },
        )
    }
}
