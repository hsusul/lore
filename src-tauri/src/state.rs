use std::sync::atomic::AtomicBool;

use tauri::{AppHandle, Manager};

/// Process-wide application state: the agent orchestrator (ADR-0007).
///
/// The session archive (background scanning, SQLite index, search) is no
/// longer part of the desktop app, so launching Lore does no disk-heavy work.
pub struct AppState {
    /// Parallel agent tasks. Agent binaries are resolved lazily on the first
    /// launch so startup does not pay for a login-shell lookup.
    pub orchestrator: lore_orchestrator::Orchestrator,
    pub agents_resolved: AtomicBool,
}

pub fn init_state(app: &AppHandle) -> Result<AppState, Box<dyn std::error::Error>> {
    let data_dir = app.path().app_data_dir()?;
    std::fs::create_dir_all(&data_dir)?;
    Ok(AppState {
        orchestrator: lore_orchestrator::Orchestrator::open(
            data_dir.join("orchestrator"),
            lore_orchestrator::AgentPrograms::default(),
        )?,
        agents_resolved: AtomicBool::new(false),
    })
}
