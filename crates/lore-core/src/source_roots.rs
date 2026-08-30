//! User-selected agent source folders.
//!
//! Custom roots are stored as Lore-owned settings, added to each adapter's
//! documented defaults, and only ever used for read-only discovery. Keeping the
//! policy in the core makes the Tauri shell a thin folder-picker/IPC boundary.

use std::path::{Path, PathBuf};

use lore_ipc::DetectedAgent;
use rusqlite::Connection;

use crate::adapters::{AdapterRegistry, DiscoveryRoots};
use crate::discovery::DiscoveryConfig;
use crate::storage::StorageError;

const SETTING_PREFIX: &str = "agent_roots.";
const MAX_CUSTOM_ROOTS: usize = 16;
const MAX_PATH_CHARS: usize = 4_096;

#[derive(Debug, thiserror::Error)]
pub enum SourceRootError {
    #[error(transparent)]
    Storage(#[from] StorageError),
    #[error("unknown agent adapter")]
    UnknownAgent,
    #[error("selected folder path is invalid")]
    InvalidPath,
    #[error("selected folder does not exist or is not a directory")]
    NotDirectory,
    #[error("select a folder below the filesystem root")]
    FilesystemRoot,
    #[error("too many custom folders for this agent")]
    TooManyRoots,
}

pub type Result<T> = std::result::Result<T, SourceRootError>;

fn setting_key(agent_id: &str) -> String {
    format!("{SETTING_PREFIX}{agent_id}")
}

/// User-selected roots for one adapter. Malformed stored JSON safely degrades
/// to no custom roots; it can be replaced the next time a folder is selected.
pub fn custom_roots(conn: &Connection, agent_id: &str) -> Result<Vec<PathBuf>> {
    let mut roots: Vec<PathBuf> = crate::settings::get(conn, &setting_key(agent_id))?
        .and_then(|raw| serde_json::from_str::<Vec<String>>(&raw).ok())
        .unwrap_or_default()
        .into_iter()
        .filter(|path| {
            !path.is_empty()
                && path.chars().count() <= MAX_PATH_CHARS
                && !path
                    .chars()
                    .any(|c| c.is_control() || crate::is_zero_width(c))
                && Path::new(path).is_absolute()
                && Path::new(path).parent().is_some()
        })
        .map(PathBuf::from)
        .collect();
    roots.sort();
    roots.dedup();
    roots.truncate(MAX_CUSTOM_ROOTS);
    Ok(roots)
}

fn store_custom_roots(conn: &Connection, agent_id: &str, roots: &[PathBuf]) -> Result<()> {
    let values: Vec<&str> = roots
        .iter()
        .map(|root| root.to_str().ok_or(SourceRootError::InvalidPath))
        .collect::<Result<_>>()?;
    let value_json = serde_json::to_string(&values).map_err(|_| SourceRootError::InvalidPath)?;
    crate::settings::set(conn, &setting_key(agent_id), &value_json)?;
    Ok(())
}

fn normalize_selected_path(path: &str) -> Result<PathBuf> {
    if path.is_empty()
        || path.chars().count() > MAX_PATH_CHARS
        || path
            .chars()
            .any(|c| c.is_control() || crate::is_zero_width(c))
    {
        return Err(SourceRootError::InvalidPath);
    }
    let selected = Path::new(path);
    if !selected.is_absolute() {
        return Err(SourceRootError::InvalidPath);
    }
    if !selected.is_dir() {
        return Err(SourceRootError::NotDirectory);
    }
    let canonical = std::fs::canonicalize(selected).map_err(|_| SourceRootError::NotDirectory)?;
    if canonical.parent().is_none() {
        return Err(SourceRootError::FilesystemRoot);
    }
    if canonical.to_str().is_none() {
        return Err(SourceRootError::InvalidPath);
    }
    Ok(canonical)
}

/// Persist a validated, canonical folder for one registered adapter. Returns
/// the canonical path (also the value exposed back to the UI).
pub fn add_custom_root(
    conn: &Connection,
    registry: &AdapterRegistry,
    agent_id: &str,
    path: &str,
) -> Result<PathBuf> {
    if registry.get(agent_id).is_none() {
        return Err(SourceRootError::UnknownAgent);
    }
    let selected = normalize_selected_path(path)?;
    let mut roots = custom_roots(conn, agent_id)?;
    if !roots.contains(&selected) {
        if roots.len() >= MAX_CUSTOM_ROOTS {
            return Err(SourceRootError::TooManyRoots);
        }
        roots.push(selected.clone());
        roots.sort();
        store_custom_roots(conn, agent_id, &roots)?;
    }
    Ok(selected)
}

/// Remove a user-selected root. Documented adapter defaults are never stored
/// here and therefore cannot be removed through this function.
pub fn remove_custom_root(
    conn: &Connection,
    registry: &AdapterRegistry,
    agent_id: &str,
    path: &str,
) -> Result<()> {
    if registry.get(agent_id).is_none() {
        return Err(SourceRootError::UnknownAgent);
    }
    let trimmed = path.trim_end_matches(std::path::is_separator);
    let selected = PathBuf::from(trimmed);
    let canonical = std::fs::canonicalize(Path::new(trimmed)).ok();
    let mut roots = custom_roots(conn, agent_id)?;
    let before = roots.len();
    roots.retain(|root| {
        let root_str = root.to_string_lossy();
        let root_trimmed = root_str.trim_end_matches(std::path::is_separator);
        root != &selected
            && root != Path::new(path)
            && root_trimmed != trimmed
            && canonical.as_ref() != Some(root)
    });
    if roots.len() != before {
        store_custom_roots(conn, agent_id, &roots)?;
    }
    Ok(())
}

/// Build the effective configuration: documented defaults plus persisted
/// custom folders for every registered adapter.
pub fn discovery_config(conn: &Connection, registry: &AdapterRegistry) -> Result<DiscoveryConfig> {
    let mut config = DiscoveryConfig::new();
    for adapter in registry.iter() {
        let mut roots = adapter.roots(&DiscoveryRoots::default());
        roots.extend(custom_roots(conn, adapter.id().0)?);
        roots.sort();
        roots.dedup();
        config.set_roots(adapter.id().0, DiscoveryRoots::new(roots));
    }
    Ok(config)
}

/// Live, cheap adapter probes plus archived-session counts for the UI. This is
/// truthful on a fresh database because it does not depend on prior ingestion.
pub fn detected_agents(
    conn: &Connection,
    registry: &AdapterRegistry,
    config: &DiscoveryConfig,
) -> Result<Vec<DetectedAgent>> {
    let mut agents = Vec::with_capacity(registry.len());
    for adapter in registry.iter() {
        let configured = config.roots_for(adapter.id().0);
        let detection = adapter.detect_installation(&configured);
        agents.push(DetectedAgent {
            id: adapter.id().0.to_string(),
            display_name: adapter.metadata().display_name.to_string(),
            installed: detection.installed,
            version: detection.version,
            session_count: crate::query::agent_session_count(conn, adapter.id().0)?,
            roots: adapter
                .roots(&configured)
                .into_iter()
                .map(|root| root.to_string_lossy().into_owned())
                .collect(),
            custom_roots: custom_roots(conn, adapter.id().0)?
                .into_iter()
                .map(|root| root.to_string_lossy().into_owned())
                .collect(),
        });
    }
    agents.sort_by(|left, right| left.id.cmp(&right.id));
    Ok(agents)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn custom_root_is_persisted_deduplicated_and_added_to_defaults() {
        let conn = crate::storage::open_in_memory().unwrap();
        let registry = AdapterRegistry::v0();
        let selected = tempfile::tempdir().unwrap();
        let path = selected.path().to_str().unwrap();

        let canonical = add_custom_root(&conn, &registry, "codex", path).unwrap();
        add_custom_root(&conn, &registry, "codex", path).unwrap();
        assert_eq!(
            custom_roots(&conn, "codex").unwrap(),
            vec![canonical.clone()]
        );

        let config = discovery_config(&conn, &registry).unwrap();
        let adapter = registry.get("codex").unwrap();
        let mut defaults = adapter.roots(&DiscoveryRoots::default());
        defaults.sort();
        defaults.dedup();
        let effective = adapter.roots(&config.roots_for("codex"));
        assert!(effective.contains(&canonical));
        assert_eq!(effective.len(), defaults.len() + 1);
        assert!(defaults.iter().all(|root| effective.contains(root)));

        remove_custom_root(&conn, &registry, "codex", canonical.to_str().unwrap()).unwrap();
        assert!(custom_roots(&conn, "codex").unwrap().is_empty());
        let reset = discovery_config(&conn, &registry).unwrap();
        assert_eq!(adapter.roots(&reset.roots_for("codex")), defaults);
    }

    #[test]
    fn remove_custom_root_handles_trailing_slashes() {
        let conn = crate::storage::open_in_memory().unwrap();
        let registry = AdapterRegistry::v0();
        let selected = tempfile::tempdir().unwrap();
        let path = selected.path().to_str().unwrap();

        let canonical = add_custom_root(&conn, &registry, "codex", path).unwrap();
        assert_eq!(custom_roots(&conn, "codex").unwrap(), vec![canonical]);

        // Remove using a path string with a trailing slash
        let path_with_slash = format!("{path}/");
        remove_custom_root(&conn, &registry, "codex", &path_with_slash).unwrap();
        assert!(custom_roots(&conn, "codex").unwrap().is_empty());
    }

    #[test]
    fn selected_root_must_be_registered_absolute_existing_and_below_root() {
        let conn = crate::storage::open_in_memory().unwrap();
        let registry = AdapterRegistry::v0();

        assert!(matches!(
            add_custom_root(&conn, &registry, "other", "/tmp"),
            Err(SourceRootError::UnknownAgent)
        ));
        assert!(matches!(
            add_custom_root(&conn, &registry, "codex", "relative"),
            Err(SourceRootError::InvalidPath)
        ));
        assert!(matches!(
            add_custom_root(&conn, &registry, "codex", "/definitely/missing/lore-root"),
            Err(SourceRootError::NotDirectory)
        ));
        assert!(matches!(
            add_custom_root(&conn, &registry, "codex", "/"),
            Err(SourceRootError::FilesystemRoot)
        ));
    }

    #[test]
    fn unsafe_persisted_paths_are_ignored_before_discovery() {
        let conn = crate::storage::open_in_memory().unwrap();
        crate::settings::set(
            &conn,
            &setting_key("codex"),
            r#"["relative","/","/poison\u0000path","/zero\u200bwidth","/definitely/missing-but-safe"]"#,
        )
        .unwrap();

        assert_eq!(
            custom_roots(&conn, "codex").unwrap(),
            vec![PathBuf::from("/definitely/missing-but-safe")]
        );
    }

    #[test]
    fn live_detection_lists_registered_adapters_before_any_ingest() {
        let conn = crate::storage::open_in_memory().unwrap();
        let registry = AdapterRegistry::v0();
        let claude = tempfile::tempdir().unwrap();
        let codex = tempfile::tempdir().unwrap();
        add_custom_root(
            &conn,
            &registry,
            "claude-code",
            claude.path().to_str().unwrap(),
        )
        .unwrap();
        add_custom_root(&conn, &registry, "codex", codex.path().to_str().unwrap()).unwrap();
        let config = discovery_config(&conn, &registry).unwrap();

        let agents = detected_agents(&conn, &registry, &config).unwrap();
        assert_eq!(agents.len(), 2);
        assert!(agents.iter().all(|agent| agent.session_count == 0));
        assert!(agents.iter().all(|agent| agent.installed));
        assert!(agents.iter().all(|agent| !agent.roots.is_empty()));
    }

    #[test]
    fn source_root_errors_format_cleanly() {
        assert_eq!(
            SourceRootError::UnknownAgent.to_string(),
            "unknown agent adapter"
        );
        assert_eq!(
            SourceRootError::InvalidPath.to_string(),
            "selected folder path is invalid"
        );
        assert_eq!(
            SourceRootError::NotDirectory.to_string(),
            "selected folder does not exist or is not a directory"
        );
        assert_eq!(
            SourceRootError::FilesystemRoot.to_string(),
            "select a folder below the filesystem root"
        );
        assert_eq!(
            SourceRootError::TooManyRoots.to_string(),
            "too many custom folders for this agent"
        );
    }

    #[test]
    #[cfg(unix)]
    fn add_and_remove_custom_root_with_symlink() {
        let dir = tempfile::tempdir().unwrap();
        let target_dir = dir.path().join("real_folder");
        std::fs::create_dir_all(&target_dir).unwrap();
        let symlink_dir = dir.path().join("symlink_folder");
        std::os::unix::fs::symlink(&target_dir, &symlink_dir).unwrap();

        let conn = crate::storage::open_in_memory().unwrap();
        let registry = AdapterRegistry::v0();

        // Adding via symlink canonicalizes to real_folder
        let canonical = add_custom_root(
            &conn,
            &registry,
            "claude-code",
            symlink_dir.to_str().unwrap(),
        )
        .unwrap();
        assert_eq!(canonical, std::fs::canonicalize(&target_dir).unwrap());

        // Removing by symlink path removes the canonical root
        remove_custom_root(
            &conn,
            &registry,
            "claude-code",
            symlink_dir.to_str().unwrap(),
        )
        .unwrap();
        assert!(custom_roots(&conn, "claude-code").unwrap().is_empty());
    }

    #[test]
    fn add_custom_root_rejects_nonexistent_and_file_and_root_paths() {
        let conn = crate::storage::open_in_memory().unwrap();
        let registry = AdapterRegistry::v0();
        let dir = tempfile::tempdir().unwrap();

        // Nonexistent path -> NotDirectory
        let err1 = add_custom_root(
            &conn,
            &registry,
            "claude-code",
            dir.path().join("does_not_exist").to_str().unwrap(),
        )
        .unwrap_err();
        assert!(matches!(err1, SourceRootError::NotDirectory));

        // File path (not directory) -> NotDirectory
        let file_path = dir.path().join("a_file.txt");
        std::fs::write(&file_path, b"hello").unwrap();
        let err2 = add_custom_root(&conn, &registry, "claude-code", file_path.to_str().unwrap())
            .unwrap_err();
        assert!(matches!(err2, SourceRootError::NotDirectory));

        // Relative path -> InvalidPath
        let err3 = add_custom_root(&conn, &registry, "claude-code", "relative/path").unwrap_err();
        assert!(matches!(err3, SourceRootError::InvalidPath));

        // Filesystem root -> FilesystemRoot
        #[cfg(unix)]
        {
            let err4 = add_custom_root(&conn, &registry, "claude-code", "/").unwrap_err();
            assert!(matches!(err4, SourceRootError::FilesystemRoot));
        }
    }

    #[test]
    fn remove_custom_root_with_trailing_slashes_and_canonical_matching() {
        let conn = crate::storage::open_in_memory().unwrap();
        let registry = AdapterRegistry::v0();
        let dir = tempfile::tempdir().unwrap();

        let added = add_custom_root(
            &conn,
            &registry,
            "claude-code",
            dir.path().to_str().unwrap(),
        )
        .unwrap();
        assert_eq!(custom_roots(&conn, "claude-code").unwrap().len(), 1);

        // Remove using path with trailing separator
        let with_trailing = format!("{}/", added.to_string_lossy());
        remove_custom_root(&conn, &registry, "claude-code", &with_trailing).unwrap();
        assert!(custom_roots(&conn, "claude-code").unwrap().is_empty());
    }

    #[test]
    fn add_custom_root_deduplicates_and_sorts_multiple_directories() {
        let conn = crate::storage::open_in_memory().unwrap();
        let registry = AdapterRegistry::v0();
        let dir = tempfile::tempdir().unwrap();

        let sub_b = dir.path().join("b_dir");
        let sub_a = dir.path().join("a_dir");
        std::fs::create_dir_all(&sub_b).unwrap();
        std::fs::create_dir_all(&sub_a).unwrap();

        // Add in reverse order
        add_custom_root(&conn, &registry, "claude-code", sub_b.to_str().unwrap()).unwrap();
        add_custom_root(&conn, &registry, "claude-code", sub_a.to_str().unwrap()).unwrap();

        // Add duplicate
        add_custom_root(&conn, &registry, "claude-code", sub_b.to_str().unwrap()).unwrap();

        let roots = custom_roots(&conn, "claude-code").unwrap();
        assert_eq!(roots.len(), 2);
        assert_eq!(roots[0], std::fs::canonicalize(&sub_a).unwrap());
        assert_eq!(roots[1], std::fs::canonicalize(&sub_b).unwrap());
    }

    #[test]
    fn add_custom_root_enforces_max_custom_roots_limit() {
        let conn = crate::storage::open_in_memory().unwrap();
        let registry = AdapterRegistry::v0();
        let dir = tempfile::tempdir().unwrap();

        for i in 0..MAX_CUSTOM_ROOTS {
            let sub = dir.path().join(format!("root_{i}"));
            std::fs::create_dir_all(&sub).unwrap();
            add_custom_root(&conn, &registry, "claude-code", sub.to_str().unwrap()).unwrap();
        }

        assert_eq!(
            custom_roots(&conn, "claude-code").unwrap().len(),
            MAX_CUSTOM_ROOTS
        );

        let extra = dir.path().join("extra_root");
        std::fs::create_dir_all(&extra).unwrap();
        let err =
            add_custom_root(&conn, &registry, "claude-code", extra.to_str().unwrap()).unwrap_err();
        assert!(matches!(err, SourceRootError::TooManyRoots));
    }

    #[test]
    fn normalize_selected_path_rejects_empty_relative_and_root_paths() {
        assert!(matches!(
            normalize_selected_path(""),
            Err(SourceRootError::InvalidPath)
        ));
        assert!(matches!(
            normalize_selected_path("relative/path"),
            Err(SourceRootError::InvalidPath)
        ));
        assert!(matches!(
            normalize_selected_path("/"),
            Err(SourceRootError::FilesystemRoot)
        ));

        // Pointing to a file instead of a directory -> NotDirectory
        let temp = tempfile::tempdir().unwrap();
        let file_path = temp.path().join("file.txt");
        std::fs::write(&file_path, b"content").unwrap();
        assert!(matches!(
            normalize_selected_path(file_path.to_str().unwrap()),
            Err(SourceRootError::NotDirectory)
        ));
    }
}
