//! Lore desktop shell (Tauri 2): runs coding agents in parallel, each in its
//! own git worktree, via `lore-orchestrator` (ADR-0007).

pub mod commands;
pub mod state;

pub use commands::*;
pub use state::{init_state, AppState};

use tauri::{Manager, RunEvent};

/// Build and run the desktop application.
pub fn run() {
    let app = tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            let state = init_state(app.handle())?;
            app.manage(state);
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
            merge_task
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
