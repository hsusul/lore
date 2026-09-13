//! Lore desktop shell (Tauri 2): runs coding agents in parallel, each in its
//! own git worktree, via `lore-orchestrator` (ADR-0007).

pub mod commands;
pub mod state;

pub use commands::*;
pub use state::{init_state, AppState};

use std::time::Duration;

use lore_ipc::TasksChangedEvent;
use tauri::{AppHandle, Emitter, Manager, RunEvent};

/// Poll cheap change signals (state and log size) on a background thread and
/// emit `tasks_changed` so the UI refreshes only what moved, instead of asking
/// for every task's git status twice a second. Also performs automatic handoffs
/// when an agent stops on a usage limit.
fn watch_tasks(app: AppHandle) {
    std::thread::spawn(move || {
        let mut previous: Vec<(String, lore_ipc::TaskState, u64)> = Vec::new();
        loop {
            std::thread::sleep(Duration::from_millis(900));
            let Some(state) = app.try_state::<AppState>() else {
                return;
            };
            let current = state.orchestrator.change_signatures();
            let mut ids: Vec<String> = current
                .iter()
                .filter(|(id, s, len)| {
                    !previous
                        .iter()
                        .any(|(pid, ps, plen)| pid == id && ps == s && plen == len)
                })
                .map(|(id, _, _)| id.clone())
                .collect();
            // A task that disappeared also changed the list.
            if current.len() != previous.len() {
                ids.push(String::new());
            }
            previous = current;
            if !ids.is_empty() {
                let _ = app.emit("tasks_changed", TasksChangedEvent { ids });
            }
            for id in state.orchestrator.run_auto_handoffs() {
                let _ = app.emit("tasks_changed", TasksChangedEvent { ids: vec![id] });
            }
        }
    });
}

/// Build and run the desktop application.
pub fn run() {
    let app = tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            let state = init_state(app.handle())?;
            app.manage(state);
            watch_tasks(app.handle().clone());
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            list_tasks,
            create_task,
            stop_task,
            discard_task,
            open_task_worktree,
            task_load_warning,
            list_workspace_dir,
            read_workspace_file,
            open_workspace,
            task_diff,
            task_activity,
            continue_task,
            commit_task,
            merge_task,
            get_task,
            list_decisions
        ])
        .build(tauri::generate_context!());
    let app = match app {
        Ok(app) => app,
        Err(error) => {
            eprintln!("fatal: error while running Lore: {error}");
            std::process::exit(1);
        }
    };

    app.run(|app_handle, event| {
        // Agents launched by Lore stop with it (ORCHESTRATOR_PLAN.md step 1).
        if let RunEvent::Exit = event {
            if let Some(state) = app_handle.try_state::<AppState>() {
                state.orchestrator.shutdown();
            }
        }
    });
}
