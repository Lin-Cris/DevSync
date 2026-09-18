use std::{
    fs::{self, OpenOptions},
    io::Write,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use serde::{Deserialize, Serialize};

use super::{
    DevSyncError,
    locking::ProcessLock,
    paths::{DevSyncPaths, ensure_directory},
    scheduler,
    watcher::WorkspaceWatcher,
    workspace,
};

const WORKSPACE_RECONCILE_INTERVAL: Duration = Duration::from_secs(30);
const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(30);
const SHUTDOWN_POLL_INTERVAL: Duration = Duration::from_millis(200);

static SHOULD_STOP: AtomicBool = AtomicBool::new(false);

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct AgentState {
    pub pid: u32,
    pub started_at: String,
    pub heartbeat_at: String,
    pub current_activity: Option<String>,
    pub last_successful_sync: Option<String>,
    pub last_error: Option<String>,
}

pub fn run() -> Result<(), DevSyncError> {
    let paths = DevSyncPaths::from_agent()?;
    let Some(_instance_lock) = ProcessLock::try_acquire(&paths.agent_lock_path())? else {
        println!("DevSync Agent already running");
        return Ok(());
    };

    println!("DevSync Agent started");
    initialize_state(&paths);
    log(&paths, "Agent started");

    SHOULD_STOP.store(false, Ordering::Relaxed);
    register_shutdown_signals()?;

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|error| {
            DevSyncError::new(
                "agent_runtime_error",
                format!("Unable to start Agent runtime: {error}"),
            )
        })?;
    runtime.block_on(async {
        let watcher = Arc::new(WorkspaceWatcher::new(tokio::runtime::Handle::current()));
        if let Err(error) = watcher.sync(&paths) {
            record_error(&paths, &error.message);
            log(
                &paths,
                &format!("workspace watcher failed: {}", error.message),
            );
        } else {
            log(&paths, "workspace watcher started");
        }

        scheduler::start_agent(paths.clone());
        log(&paths, "scheduler started");

        let watcher_paths = paths.clone();
        tokio::spawn(async move {
            let mut timer = tokio::time::interval(WORKSPACE_RECONCILE_INTERVAL);
            timer.tick().await;
            while !SHOULD_STOP.load(Ordering::Relaxed) {
                if let Err(error) = watcher.sync(&watcher_paths) {
                    record_error(&watcher_paths, &error.message);
                    log(
                        &watcher_paths,
                        &format!("workspace watcher refresh failed: {}", error.message),
                    );
                }
                timer.tick().await;
            }
        });

        let mut heartbeat = tokio::time::interval(HEARTBEAT_INTERVAL);
        heartbeat.tick().await;
        while !SHOULD_STOP.load(Ordering::Relaxed) {
            tokio::select! {
                _ = heartbeat.tick() => {
                    if !SHOULD_STOP.load(Ordering::Relaxed) {
                        update_state(&paths, |state| state.heartbeat_at = workspace::now());
                    }
                }
                _ = tokio::time::sleep(SHUTDOWN_POLL_INTERVAL) => {}
            }
        }
    });

    log(&paths, "Agent shutting down");
    update_state(&paths, |state| {
        state.current_activity = None;
        state.heartbeat_at = workspace::now();
    });
    Ok(())
}

#[cfg(unix)]
fn register_shutdown_signals() -> Result<(), DevSyncError> {
    unsafe {
        libc::signal(
            libc::SIGTERM,
            handle_signal as *const () as libc::sighandler_t,
        );
        libc::signal(
            libc::SIGINT,
            handle_signal as *const () as libc::sighandler_t,
        );
    }
    Ok(())
}

#[cfg(not(unix))]
fn register_shutdown_signals() -> Result<(), DevSyncError> {
    Ok(())
}

#[cfg(unix)]
extern "C" fn handle_signal(_signal: libc::c_int) {
    SHOULD_STOP.store(true, Ordering::Relaxed);
}

fn initialize_state(paths: &DevSyncPaths) {
    update_state(paths, |state| {
        state.pid = std::process::id();
        state.started_at = workspace::now();
        state.heartbeat_at = state.started_at.clone();
        state.current_activity = None;
        state.last_error = None;
    });
}

pub fn read_state(paths: &DevSyncPaths) -> Option<AgentState> {
    let contents = fs::read_to_string(paths.agent_state_path()).ok()?;
    serde_json::from_str(&contents).ok()
}

pub fn update_state(paths: &DevSyncPaths, update: impl FnOnce(&mut AgentState)) {
    let mut state = read_state(paths).unwrap_or_default();
    update(&mut state);
    if state.pid == 0 {
        state.pid = std::process::id();
    }
    if state.started_at.is_empty() {
        state.started_at = workspace::now();
    }
    if state.heartbeat_at.is_empty() {
        state.heartbeat_at = workspace::now();
    }
    let Ok(serialized) = serde_json::to_string_pretty(&state) else {
        return;
    };
    if ensure_directory(&paths.data_dir).is_err() {
        return;
    }
    let temp_path = paths
        .agent_state_path()
        .with_extension(format!("json.{}.tmp", std::process::id()));
    if fs::write(&temp_path, serialized).is_ok() {
        let _ = fs::rename(temp_path, paths.agent_state_path());
    }
}

pub fn set_activity(paths: &DevSyncPaths, activity: Option<&str>) {
    update_state(paths, |state| {
        state.current_activity = activity.map(str::to_string);
    });
}

pub fn set_last_successful_sync(paths: &DevSyncPaths) {
    update_state(paths, |state| {
        state.last_successful_sync = Some(workspace::now());
    });
}

pub fn record_error(paths: &DevSyncPaths, error: &str) {
    update_state(paths, |state| {
        state.last_error = Some(error.to_string());
    });
}

pub fn clear_error(paths: &DevSyncPaths) {
    update_state(paths, |state| state.last_error = None);
}

pub fn log(paths: &DevSyncPaths, message: &str) {
    let path = paths.agent_log_path();
    let Some(parent) = path.parent() else {
        return;
    };
    if ensure_directory(parent).is_err() {
        return;
    }
    let Ok(mut file) = OpenOptions::new().create(true).append(true).open(path) else {
        return;
    };
    let _ = writeln!(file, "{} {}", workspace::now(), message);
}
