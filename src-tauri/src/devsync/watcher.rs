use std::{
    collections::HashMap,
    path::Path,
    sync::{Arc, Mutex},
    time::Duration,
};

use notify::{Config, Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use tokio::{runtime::Handle, task::AbortHandle};

use super::{DevSyncError, agent, build, paths::DevSyncPaths, scheduler, workspace};

pub const DEBOUNCE_WINDOW: Duration = Duration::from_secs(5);

pub struct WorkspaceWatcher {
    watchers: Mutex<HashMap<String, RecommendedWatcher>>,
    debounce_tokens: Arc<Mutex<HashMap<String, u64>>>,
    debounce_tasks: Arc<Mutex<HashMap<String, AbortHandle>>>,
    runtime: Handle,
}

impl WorkspaceWatcher {
    pub fn new(runtime: Handle) -> Self {
        Self {
            watchers: Mutex::new(HashMap::new()),
            debounce_tokens: Arc::new(Mutex::new(HashMap::new())),
            debounce_tasks: Arc::new(Mutex::new(HashMap::new())),
            runtime,
        }
    }

    pub fn sync(&self, paths: &DevSyncPaths) -> Result<(), DevSyncError> {
        let workspaces = workspace::list_workspaces_at(paths)?;
        let workspace_ids = workspaces
            .iter()
            .map(|workspace| workspace.id.as_str())
            .collect::<std::collections::HashSet<_>>();
        let mut watchers = self
            .watchers
            .lock()
            .map_err(|_| DevSyncError::new("watcher_error", "Workspace watcher lock failed."))?;
        watchers.retain(|id, _| workspace_ids.contains(id.as_str()));
        if let Ok(mut tokens) = self.debounce_tokens.lock() {
            tokens.retain(|id, _| workspace_ids.contains(id.as_str()));
        }
        if let Ok(mut tasks) = self.debounce_tasks.lock() {
            tasks.retain(|id, task| {
                if workspace_ids.contains(id.as_str()) {
                    true
                } else {
                    task.abort();
                    false
                }
            });
        }
        for workspace_record in workspaces {
            if watchers.contains_key(&workspace_record.id)
                || !Path::new(&workspace_record.folder_path).is_dir()
            {
                continue;
            }
            let paths = paths.clone();
            let workspace_id = workspace_record.id.clone();
            let callback_tokens = Arc::clone(&self.debounce_tokens);
            let callback_tasks = Arc::clone(&self.debounce_tasks);
            let runtime = self.runtime.clone();
            let mut watcher = RecommendedWatcher::new(
                move |result: notify::Result<Event>| match result {
                    Ok(event)
                        if !matches!(event.kind, EventKind::Access(_))
                            && !build::is_workspace_watcher_suppressed(&paths, &workspace_id)
                            && event.paths.iter().any(|path| is_meaningful_path(path)) =>
                    {
                        schedule_change(
                            paths.clone(),
                            workspace_id.clone(),
                            Arc::clone(&callback_tokens),
                            Arc::clone(&callback_tasks),
                            runtime.clone(),
                        );
                    }
                    Err(error) => {
                        agent::record_error(
                            &paths,
                            &format!("workspace watcher event failed: {error}"),
                        );
                        agent::log(&paths, &format!("workspace watcher event failed: {error}"));
                    }
                    _ => {}
                },
                Config::default(),
            )
            .map_err(|error| {
                DevSyncError::new(
                    "watcher_error",
                    format!("Unable to create workspace watcher: {error}"),
                )
            })?;
            watcher
                .watch(
                    Path::new(&workspace_record.folder_path),
                    RecursiveMode::Recursive,
                )
                .map_err(|error| {
                    DevSyncError::new(
                        "watcher_error",
                        format!("Unable to watch workspace: {error}"),
                    )
                })?;
            watchers.insert(workspace_record.id, watcher);
        }
        Ok(())
    }
}

fn schedule_change(
    paths: DevSyncPaths,
    workspace_id: String,
    tokens: Arc<Mutex<HashMap<String, u64>>>,
    tasks: Arc<Mutex<HashMap<String, AbortHandle>>>,
    runtime: Handle,
) {
    let token = match tokens.lock() {
        Ok(mut values) => {
            let next = values
                .get(&workspace_id)
                .copied()
                .unwrap_or(0)
                .saturating_add(1);
            values.insert(workspace_id.clone(), next);
            next
        }
        Err(_) => return,
    };
    let task_workspace_id = workspace_id.clone();
    let task = runtime.spawn(async move {
        tokio::time::sleep(DEBOUNCE_WINDOW).await;
        if tokens
            .lock()
            .ok()
            .and_then(|values| values.get(&task_workspace_id).copied())
            != Some(token)
        {
            return;
        }
        match mark_changed_with_retry(&paths, &task_workspace_id).await {
            Ok(should_reconcile) => {
                agent::log(
                    &paths,
                    &format!("workspace change detected: {task_workspace_id}"),
                );
                if should_reconcile {
                    if let Err(error) = scheduler::reconcile_agent(&paths).await {
                        agent::record_error(&paths, &error.message);
                        agent::log(
                            &paths,
                            &format!("workspace reconcile failed: {}", error.message),
                        );
                    }
                }
            }
            Err(error) => {
                agent::record_error(&paths, &error.message);
                agent::log(
                    &paths,
                    &format!("workspace change could not be recorded: {}", error.message),
                );
            }
        }
    });
    if let Ok(mut pending) = tasks.lock() {
        if let Some(previous) = pending.insert(workspace_id, task.abort_handle()) {
            previous.abort();
        }
    }
}

async fn mark_changed_with_retry(
    paths: &DevSyncPaths,
    workspace_id: &str,
) -> Result<bool, DevSyncError> {
    let mut last_error = None;
    for attempt in 0..3 {
        match mark_changed(paths, workspace_id) {
            Ok(should_reconcile) => return Ok(should_reconcile),
            Err(error) => {
                last_error = Some(error);
                if attempt < 2 {
                    tokio::time::sleep(Duration::from_millis(200)).await;
                }
            }
        }
    }
    Err(last_error.unwrap_or_else(|| {
        DevSyncError::new("watcher_error", "Unable to record workspace change.")
    }))
}

fn mark_changed(paths: &DevSyncPaths, workspace_id: &str) -> Result<bool, DevSyncError> {
    let mut store = workspace::load_store_at(paths)?;
    let workspace_record = workspace::workspace_mut(&mut store, workspace_id)?;
    workspace_record.source_revision = workspace_record.source_revision.saturating_add(1);
    workspace_record.changes_detected =
        workspace_record.source_revision != workspace_record.deployed_revision;
    let should_reconcile = workspace_record.auto_sync && workspace_record.changes_detected;
    workspace_record.background_state = Some("changesDetected".into());
    workspace_record.updated_at = workspace::now();
    workspace::save_store_at(paths, &store)?;
    Ok(should_reconcile)
}

pub fn is_meaningful_path(path: &Path) -> bool {
    let normalized = path.to_string_lossy().to_lowercase();
    if [
        "/.git/",
        "/deriveddata/",
        "/.build/",
        "/build/",
        "/node_modules/",
        "/target/",
        "/dist/",
        "/logs/",
        "/pods/build/",
        "/carthage/build/",
    ]
    .iter()
    .any(|part| normalized.contains(part))
    {
        return false;
    }
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("");
    if name.starts_with('.')
        || name.ends_with('~')
        || name.ends_with(".swp")
        || name.ends_with(".tmp")
    {
        return false;
    }
    if name == "Package.swift" || name == "Package.resolved" || name == "project.pbxproj" {
        return true;
    }
    matches!(
        path.extension()
            .and_then(|extension| extension.to_str())
            .unwrap_or("")
            .to_lowercase()
            .as_str(),
        "swift"
            | "m"
            | "mm"
            | "h"
            | "hpp"
            | "c"
            | "cpp"
            | "metal"
            | "storyboard"
            | "xib"
            | "plist"
            | "ts"
            | "tsx"
            | "js"
            | "jsx"
            | "mjs"
            | "css"
            | "html"
            | "entitlements"
            | "strings"
            | "json"
            | "yaml"
            | "yml"
            | "xcassets"
            | "xcodeproj"
            | "xcworkspace"
            | "png"
            | "jpg"
            | "jpeg"
            | "heic"
            | "svg"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn ignores_generated_and_editor_paths() {
        assert!(!is_meaningful_path(Path::new(
            "/project/DerivedData/App.swift"
        )));
        assert!(!is_meaningful_path(Path::new("/project/.git/index")));
        assert!(!is_meaningful_path(Path::new("/project/App.swift.swp")));
        assert!(is_meaningful_path(Path::new("/project/App.swift")));
        assert!(is_meaningful_path(Path::new(
            "/project/App.xcodeproj/project.pbxproj"
        )));
        assert!(is_meaningful_path(Path::new("/project/web/App.tsx")));
        assert!(is_meaningful_path(Path::new(
            "/project/Assets.xcassets/Icon.png"
        )));
        assert!(is_meaningful_path(Path::new("/project/App.xcworkspace")));
        assert!(!is_meaningful_path(Path::new(
            "/project/node_modules/react/index.js"
        )));
    }

    #[test]
    fn suppresses_events_while_a_workspace_operation_is_active() {
        let root =
            std::env::temp_dir().join(format!("devsync-watcher-test-{}", std::process::id()));
        let paths = DevSyncPaths::for_test(&root);
        let workspace_id = format!("watcher-test-{}", std::process::id());
        let guard = build::acquire_workspace_operation(&paths, &workspace_id).unwrap();
        assert!(build::is_workspace_watcher_suppressed(
            &paths,
            &workspace_id
        ));
        drop(guard);
        assert!(build::is_workspace_watcher_suppressed(
            &paths,
            &workspace_id
        ));
        let _ = std::fs::remove_dir_all(root);
    }
}
