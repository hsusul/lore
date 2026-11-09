use std::io::Write;
use std::path::{Path, PathBuf};

use lore_core::adapters::AdapterRegistry;
use lore_core::discovery::DiscoveryConfig;
use lore_core::is_invalid_text_token;
use tauri::{AppHandle, State};

use crate::state::AppState;

/// Export a session as Markdown. `include_secrets` defaults off (masked); passing
/// true is an explicit opt-in to full-fidelity content.
#[tauri::command]
pub fn export_session_markdown(
    state: State<'_, AppState>,
    id: String,
    include_secrets: bool,
) -> Result<Option<String>, String> {
    if is_invalid_text_token(&id, 256) {
        return Err("invalid session id".to_string());
    }
    let conn = state.db.lock().map_err(|_| "state lock poisoned")?;
    lore_core::export::export_session_markdown(&conn, &id, include_secrets)
        .map_err(|e| e.to_string())
}

pub fn validate_export_path(
    path: &Path,
    archive_dir: &Path,
    registry: &AdapterRegistry,
    config: &DiscoveryConfig,
) -> Result<PathBuf, String> {
    if !path.is_absolute() {
        return Err("export path must be absolute".to_string());
    }
    let path_str = path.to_string_lossy();
    if path_str.is_empty()
        || path_str.len() > 4096
        || path_str
            .chars()
            .any(|c| c.is_control() || lore_core::is_zero_width(c))
    {
        return Err("invalid export path".to_string());
    }

    let parent = path.parent().ok_or("invalid destination path")?;
    let canonical_parent = parent
        .canonicalize()
        .map_err(|_| "destination directory does not exist".to_string())?;
    let file_name = path.file_name().ok_or("invalid destination file name")?;
    let target = canonical_parent.join(file_name);

    let canonical_archive = archive_dir
        .canonicalize()
        .unwrap_or_else(|_| archive_dir.to_path_buf());
    if target.starts_with(&canonical_archive) {
        return Err("cannot export inside the Lore archive directory".to_string());
    }

    // Prohibit writing inside default or configured agent discovery and home roots
    let forbidden_roots = lore_core::discovery::protected_roots(registry, config);

    for forbidden in forbidden_roots {
        let canonical_forbidden = forbidden
            .canonicalize()
            .unwrap_or_else(|_| forbidden.clone());
        if target.starts_with(&canonical_forbidden) {
            return Err("cannot export inside agent session directories".to_string());
        }
    }

    Ok(target)
}

/// Write a session's redacted Markdown export to a validated destination path.
/// If `path` is provided, it is validated against strict security boundaries
/// (must be absolute, cannot be inside archive dir or agent session dirs).
/// If `path` is omitted, the native save dialog is displayed in Rust.
///
/// `async` so it runs off the main thread. The blocking dialog needs the main
/// thread's event loop, so calling it from a sync command hangs the window. The
/// config lock is taken after the dialog closes for the same reason.
#[tauri::command]
pub async fn save_session_export(
    app: AppHandle,
    state: State<'_, AppState>,
    id: String,
    path: Option<String>,
    include_secrets: bool,
) -> Result<bool, String> {
    if is_invalid_text_token(&id, 256) {
        return Err("invalid session id".to_string());
    }

    let chosen = match path {
        Some(p) => PathBuf::from(p),
        None => {
            use tauri_plugin_dialog::DialogExt;
            let file_path = app
                .dialog()
                .file()
                .add_filter("Markdown", &["md"])
                .set_file_name("session.md")
                .blocking_save_file();
            let Some(fp) = file_path else {
                return Ok(false);
            };
            fp.as_path()
                .ok_or("invalid file path from save dialog")?
                .to_path_buf()
        }
    };
    let target_path = {
        let config = state.config.lock().map_err(|_| "state lock poisoned")?;
        validate_export_path(&chosen, &state.archive_dir, &state.registry, &config)?
    };

    let conn = state.db.lock().map_err(|_| "state lock poisoned")?;
    let markdown = lore_core::export::export_session_markdown(&conn, &id, include_secrets)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "session not found".to_string())?;
    write_export(&target_path, &markdown)?;
    Ok(true)
}

/// Write `contents` to `target` through a new sibling temp file renamed over it.
/// A symlink at `target` is refused rather than written through (which could
/// redirect the export past `validate_export_path`) or replaced (which would
/// delete a link the person made). A regular file there is replaced and keeps
/// its permissions.
pub fn write_export(target: &Path, contents: &str) -> Result<(), String> {
    let existing = match std::fs::symlink_metadata(target) {
        Ok(meta) if meta.file_type().is_symlink() => {
            return Err("the destination is a symbolic link; choose another name".into())
        }
        Ok(meta) => Some(meta.permissions()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return Err(e.to_string()),
    };
    let dir = target.parent().ok_or("invalid destination path")?;
    let file_name = target.file_name().ok_or("invalid destination file name")?;
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let tmp = dir.join(format!(
        ".{}.{}.{nanos}.tmp",
        file_name.to_string_lossy(),
        std::process::id()
    ));
    // `create_new` refuses any existing entry at `tmp`, symlinks included.
    let mut file = std::fs::File::options()
        .write(true)
        .create_new(true)
        .open(&tmp)
        .map_err(|e| e.to_string())?;
    let written = file
        .write_all(contents.as_bytes())
        .and_then(|()| match existing {
            Some(permissions) => file.set_permissions(permissions),
            None => Ok(()),
        });
    drop(file);
    if let Err(e) = written.and_then(|()| std::fs::rename(&tmp, target)) {
        let _ = std::fs::remove_file(&tmp);
        return Err(e.to_string());
    }
    Ok(())
}
