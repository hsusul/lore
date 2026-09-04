//! Exit-code taxonomy, and the errors that map onto it.
//!
//! A CLI's exit code is its most-consumed output: shell scripts, agent hooks,
//! and CI branch on it long before anyone reads the message. So the taxonomy is
//! fixed here, once, and every error the binary can produce is mapped onto it by
//! a total function with no wildcard arm — adding a
//! [`StorageError`](lore_core::storage::StorageError) variant without deciding
//! what it means for a caller is a compile error, not a silent `1`.
//!
//! The codes deliberately separate *no archive* (2) from *archive unreadable*
//! (4). They lead to different next actions: the first is fixed by scanning, the
//! second by upgrading Lore or investigating a damaged file. Collapsing them
//! would make the exit code useless for exactly the automation that reads it.

use lore_core::paths::PathError;
use lore_core::storage::StorageError;

/// The command did what it was asked to do.
pub const OK: u8 = 0;
/// The invocation was wrong: unknown flag or command, a relative `--archive`,
/// or a command this build does not implement.
pub const USAGE: u8 = 1;
/// No archive exists at the resolved location. Recoverable by scanning.
pub const NO_ARCHIVE: u8 = 2;
/// The working directory is not inside a Git repository.
///
/// Reserved: nothing in this build resolves a repository. It is defined now so
/// the repository-aware commands cannot renumber the codes below it later.
pub const NOT_A_REPO: u8 = 3;
/// An archive exists but this build cannot read it: not a Lore archive, a
/// schema from a newer or older build, an inconsistent migration ledger, or an
/// I/O failure while opening.
pub const ARCHIVE_UNREADABLE: u8 = 4;

/// Everything `lorectl` can fail with.
///
/// Kept content-free in the same sense as [`StorageError`]: the `Display` text
/// describes a condition, never archive contents. The one thing a message can
/// echo is the user's own argv, which they just typed.
#[derive(Debug, thiserror::Error)]
pub enum CliError {
    /// The command line could not be understood.
    #[error("{0}")]
    Usage(String),
    /// A command name this build recognizes but does not yet implement. Named
    /// so the parser already knows every command we will grow into, and PR-sized
    /// slices can replace one without touching argument parsing.
    #[error("`lorectl {0}` is not implemented in this build")]
    NotImplemented(&'static str),
    /// The archive location could not be resolved.
    #[error(transparent)]
    Path(#[from] PathError),
    /// The archive could not be opened or is not one.
    #[error(transparent)]
    Storage(#[from] StorageError),
}

impl CliError {
    /// Advice printed after the error, when there is a concrete next action.
    ///
    /// Named here rather than at the failure site so the same condition always
    /// suggests the same fix.
    #[must_use]
    pub fn advice(&self) -> Option<&'static str> {
        match self {
            CliError::Storage(StorageError::ArchiveMissing) => {
                Some("run `lorectl scan` to build one")
            }
            CliError::Usage(_) | CliError::NotImplemented(_) => Some("run `lorectl --help`"),
            _ => None,
        }
    }
}

/// The process exit code for an error. Total, and exhaustive by construction.
#[must_use]
pub fn exit_code(error: &CliError) -> u8 {
    match error {
        // A relative `--archive` is a mistyped flag, not a broken archive: the
        // user asked for a location that cannot mean one thing, so it is usage.
        CliError::Usage(_) | CliError::NotImplemented(_) | CliError::Path(_) => USAGE,
        CliError::Storage(error) => storage_exit_code(error),
    }
}

/// Map an archive-layer failure onto the taxonomy.
///
/// Listed variant by variant with no `_` arm on purpose: a new `StorageError`
/// must be triaged here deliberately. `Migration`, `Sqlite`, and `Io` all reach
/// a caller only as "this archive did not open", which is [`ARCHIVE_UNREADABLE`]
/// — the distinction between them lives in the message, not the code.
fn storage_exit_code(error: &StorageError) -> u8 {
    match error {
        StorageError::ArchiveMissing => NO_ARCHIVE,
        StorageError::NotAnArchive
        | StorageError::SchemaTooNew { .. }
        | StorageError::SchemaNeedsUpgrade { .. }
        | StorageError::SchemaInconsistent(_)
        | StorageError::Migration(_)
        | StorageError::Sqlite(_)
        | StorageError::Io => ARCHIVE_UNREADABLE,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_missing_archive_is_distinct_from_an_unreadable_one() {
        // The whole point of the taxonomy: "you have no archive yet" and "your
        // archive is damaged" need different next actions from the caller.
        assert_eq!(
            exit_code(&CliError::Storage(StorageError::ArchiveMissing)),
            NO_ARCHIVE
        );
        assert_eq!(
            exit_code(&CliError::Storage(StorageError::NotAnArchive)),
            ARCHIVE_UNREADABLE
        );
        assert_ne!(NO_ARCHIVE, ARCHIVE_UNREADABLE);
    }

    #[test]
    fn every_archive_compatibility_failure_is_unreadable() {
        for error in [
            StorageError::NotAnArchive,
            StorageError::SchemaTooNew {
                archive: 99,
                supported: 5,
            },
            StorageError::SchemaNeedsUpgrade {
                archive: 1,
                supported: 5,
            },
            StorageError::SchemaInconsistent("gap in versions"),
            StorageError::Migration("boom".into()),
            StorageError::Io,
        ] {
            assert_eq!(
                exit_code(&CliError::Storage(error)),
                ARCHIVE_UNREADABLE,
                "every non-missing archive failure shares one code"
            );
        }
    }

    #[test]
    fn compatibility_failures_still_read_differently_from_each_other() {
        // Same exit code, distinct messages: automation branches on the code, a
        // person needs to know whether to upgrade Lore or to run a scan.
        let too_new = StorageError::SchemaTooNew {
            archive: 9,
            supported: 5,
        }
        .to_string();
        let needs_upgrade = StorageError::SchemaNeedsUpgrade {
            archive: 1,
            supported: 5,
        }
        .to_string();
        assert_ne!(too_new, needs_upgrade);
        assert_ne!(too_new, StorageError::NotAnArchive.to_string());
    }

    #[test]
    fn bad_input_of_every_shape_is_a_usage_error() {
        assert_eq!(exit_code(&CliError::Usage("nope".into())), USAGE);
        assert_eq!(exit_code(&CliError::NotImplemented("scan")), USAGE);
        assert_eq!(
            exit_code(&CliError::Path(PathError::RelativeOverride)),
            USAGE
        );
        assert_eq!(
            exit_code(&CliError::Path(PathError::RelativeEnvOverride)),
            USAGE
        );
        assert_eq!(exit_code(&CliError::Path(PathError::UnknownDataDir)), USAGE);
    }

    #[test]
    fn a_missing_archive_names_the_command_that_creates_one() {
        let advice = CliError::Storage(StorageError::ArchiveMissing)
            .advice()
            .expect("a missing archive has a fix");
        assert!(advice.contains("lorectl scan"), "{advice}");
    }

    #[test]
    fn an_unreadable_archive_offers_no_glib_fix() {
        // Scanning would not repair a corrupt or too-new archive, so suggesting
        // it would be worse than saying nothing.
        assert!(CliError::Storage(StorageError::NotAnArchive)
            .advice()
            .is_none());
    }

    #[test]
    fn the_reserved_codes_are_disjoint_and_leave_room() {
        let assigned = [OK, USAGE, NO_ARCHIVE, NOT_A_REPO, ARCHIVE_UNREADABLE];
        let mut seen = assigned.to_vec();
        seen.sort_unstable();
        seen.dedup();
        assert_eq!(seen.len(), assigned.len(), "codes must not collide");
        assert_eq!(seen, [0, 1, 2, 3, 4], "5-9 stay free for later commands");
    }
}
