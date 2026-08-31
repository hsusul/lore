//! Where the Lore archive lives on disk.
//!
//! The archive directory is resolved in exactly one place so every surface —
//! the Tauri shell today, a CLI tomorrow — opens the *same* files. Before this
//! module the shell computed the path inline from `app.path().app_data_dir()`,
//! which is unreachable from a non-Tauri binary; duplicating that computation
//! would risk the two surfaces silently drifting onto different archives.
//!
//! ## Resolution order
//!
//! 1. an explicit path passed by the caller (a future `--archive` flag);
//! 2. the [`ARCHIVE_DIR_ENV`] environment variable;
//! 3. the platform data directory joined with [`APP_IDENTIFIER`].
//!
//! Step 3 reproduces Tauri's `app_data_dir()` **by construction**: Tauri 2
//! defines it as `dirs::data_dir().join(identifier)`, so this module calls the
//! same `dirs` function with the same identifier rather than reimplementing the
//! platform rules. Reimplementing them would be a latent divergence — the rules
//! differ per platform (macOS ignores `XDG_DATA_HOME`; Linux ignores a
//! *relative* `XDG_DATA_HOME`; `$HOME` falls back to the passwd database when
//! unset) and could change in a future `dirs` release.
//!
//! Resolution never creates, moves, copies, or renames anything. Callers that
//! need the directory to exist create it themselves.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};

/// Bundle identifier of the Lore desktop app, and the final path component of
/// the default archive directory.
///
/// **Must equal `identifier` in `src-tauri/tauri.conf.json`.** A test in the
/// shell crate asserts that, because changing one without the other would point
/// the app and the CLI at different archives.
pub const APP_IDENTIFIER: &str = "dev.lore.app";

/// Filename of the archive database inside the archive directory.
pub const ARCHIVE_DB_FILENAME: &str = "lore.db";

/// Environment variable that overrides the archive directory.
pub const ARCHIVE_DIR_ENV: &str = "LORE_ARCHIVE_DIR";

/// Errors from archive path resolution. Content-free: never embeds a user path.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum PathError {
    /// An explicit override was given but is not absolute. Relative archive
    /// paths are rejected rather than resolved against the process working
    /// directory, where the answer would depend on where the binary was run.
    #[error("archive path override must be absolute")]
    RelativeOverride,
    /// `LORE_ARCHIVE_DIR` is set to a relative path.
    #[error("LORE_ARCHIVE_DIR must be an absolute path")]
    RelativeEnvOverride,
    /// The platform data directory could not be determined (no home directory
    /// and no passwd entry). Returned, never panicked on.
    #[error("could not determine the user data directory")]
    UnknownDataDir,
}

/// Convenience result alias for path resolution.
pub type Result<T> = std::result::Result<T, PathError>;

/// The Lore archive directory.
///
/// Pass `Some(path)` to force a location (a `--archive` flag); pass `None` to
/// use [`ARCHIVE_DIR_ENV`] or the platform default. The directory is *not*
/// created.
pub fn archive_dir(explicit: Option<&Path>) -> Result<PathBuf> {
    let env_override = std::env::var_os(ARCHIVE_DIR_ENV);
    resolve(explicit, env_override.as_deref(), dirs::data_dir())
}

/// Full path to the archive database, i.e. [`archive_dir`] joined with
/// [`ARCHIVE_DB_FILENAME`].
pub fn archive_db_path(explicit: Option<&Path>) -> Result<PathBuf> {
    Ok(archive_dir(explicit)?.join(ARCHIVE_DB_FILENAME))
}

/// The pure resolution rule, with the environment and the platform lookup
/// passed in.
///
/// Splitting this out keeps the decision testable without mutating process-wide
/// environment variables, which is racy under the parallel test harness (and
/// `unsafe` from edition 2024 onward).
fn resolve(
    explicit: Option<&Path>,
    env_override: Option<&OsStr>,
    data_dir: Option<PathBuf>,
) -> Result<PathBuf> {
    if let Some(path) = explicit {
        return if path.is_absolute() {
            Ok(path.to_path_buf())
        } else {
            Err(PathError::RelativeOverride)
        };
    }

    // An empty value is treated as unset, matching how `dirs` treats an empty
    // `$HOME`, so `LORE_ARCHIVE_DIR= lore …` clears the override for one command
    // instead of failing.
    if let Some(value) = env_override.filter(|v| !v.is_empty()) {
        let path = PathBuf::from(value);
        return if path.is_absolute() {
            Ok(path)
        } else {
            Err(PathError::RelativeEnvOverride)
        };
    }

    data_dir
        .map(|dir| dir.join(APP_IDENTIFIER))
        .ok_or(PathError::UnknownDataDir)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn data() -> Option<PathBuf> {
        Some(PathBuf::from("/data"))
    }

    #[test]
    fn explicit_override_wins_over_env_and_platform() {
        let got = resolve(
            Some(Path::new("/explicit")),
            Some(OsStr::new("/from-env")),
            data(),
        )
        .unwrap();
        assert_eq!(got, PathBuf::from("/explicit"));
    }

    #[test]
    fn env_override_wins_over_platform_default() {
        let got = resolve(None, Some(OsStr::new("/from-env")), data()).unwrap();
        assert_eq!(got, PathBuf::from("/from-env"));
    }

    #[test]
    fn platform_default_joins_the_app_identifier() {
        let got = resolve(None, None, data()).unwrap();
        assert_eq!(got, PathBuf::from("/data").join(APP_IDENTIFIER));
    }

    #[test]
    fn relative_explicit_override_is_rejected() {
        let err = resolve(Some(Path::new("relative/archive")), None, data()).unwrap_err();
        assert_eq!(err, PathError::RelativeOverride);
    }

    #[test]
    fn relative_env_override_is_rejected() {
        let err = resolve(None, Some(OsStr::new("relative/archive")), data()).unwrap_err();
        assert_eq!(err, PathError::RelativeEnvOverride);
    }

    #[test]
    fn a_relative_env_override_is_an_error_not_a_silent_fallback() {
        // `dirs` silently ignores a relative XDG_DATA_HOME. Lore does not: an
        // override the user set deliberately must never resolve somewhere else,
        // because that would read or write the wrong archive without saying so.
        assert!(resolve(None, Some(OsStr::new("./archive")), data()).is_err());
    }

    #[test]
    fn empty_env_override_is_treated_as_unset() {
        let got = resolve(None, Some(OsStr::new("")), data()).unwrap();
        assert_eq!(got, PathBuf::from("/data").join(APP_IDENTIFIER));
    }

    #[test]
    fn missing_data_dir_is_an_error_not_a_panic() {
        let err = resolve(None, None, None).unwrap_err();
        assert_eq!(err, PathError::UnknownDataDir);
        assert_eq!(
            err.to_string(),
            "could not determine the user data directory"
        );
    }

    #[test]
    fn an_explicit_override_still_wins_when_no_data_dir_exists() {
        let got = resolve(Some(Path::new("/explicit")), None, None).unwrap();
        assert_eq!(got, PathBuf::from("/explicit"));
    }

    #[test]
    fn db_path_is_the_directory_joined_with_the_database_filename() {
        let dir = resolve(Some(Path::new("/explicit")), None, None).unwrap();
        assert_eq!(
            dir.join(ARCHIVE_DB_FILENAME),
            PathBuf::from("/explicit/lore.db")
        );
    }

    #[test]
    fn public_helpers_agree_and_use_the_explicit_override() {
        let dir = archive_dir(Some(Path::new("/explicit"))).unwrap();
        let db = archive_db_path(Some(Path::new("/explicit"))).unwrap();
        assert_eq!(dir, PathBuf::from("/explicit"));
        assert_eq!(db, dir.join(ARCHIVE_DB_FILENAME));
    }

    #[test]
    fn the_database_filename_and_identifier_are_unchanged() {
        // Guards against an accidental rename: existing archives live at these
        // exact names and are never migrated.
        assert_eq!(ARCHIVE_DB_FILENAME, "lore.db");
        assert_eq!(APP_IDENTIFIER, "dev.lore.app");
    }

    #[test]
    fn the_platform_default_ends_with_the_identifier_on_this_machine() {
        // Skips rather than fails where no home directory is resolvable (some
        // sandboxes), since that path is covered by `missing_data_dir_*`.
        if dirs::data_dir().is_none() {
            return;
        }
        let dir = archive_dir(None).unwrap();
        // Only meaningful when the environment override is not set.
        if std::env::var_os(ARCHIVE_DIR_ENV).is_none() {
            assert!(dir.is_absolute());
            assert_eq!(dir.file_name(), Some(OsStr::new(APP_IDENTIFIER)));
        }
    }
}
