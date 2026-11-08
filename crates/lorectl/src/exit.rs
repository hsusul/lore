//! Exit-code taxonomy, and the errors that map onto it.
//!
//! A CLI's exit code is its most-consumed output: shell scripts, agent hooks,
//! and CI branch on it long before anyone reads the message. So the taxonomy is
//! fixed here, once, and every error the binary can produce is mapped onto it by
//! a total function with no wildcard arm — adding a
//! [`StorageError`](lore_core::storage::StorageError) variant without deciding
//! what it means for a caller is a compile error, not a silent `1`.
//!
//! The codes deliberately separate conditions that lead to different next
//! actions, because that is the only thing an exit code is good for. *No
//! archive* (2) is fixed by scanning; *archive unreadable* (4) by upgrading Lore
//! or investigating a damaged file; *archive busy* (5) by waiting and retrying,
//! which is the one case where retrying the identical command is the right move;
//! *scan failed* (6) by looking at why. Collapsing any of these would make the
//! code useless to exactly the automation that reads it.

use lore_core::jobs::JobQueueError;
use lore_core::lock::LockError;
use lore_core::paths::PathError;
use lore_core::source_roots::SourceRootError;
use lore_core::storage::StorageError;
use lore_memory::inbox::InboxError;
use lore_memory::review::ReviewError;
use lore_memory::vault::VaultError;

/// The command did what it was asked to do.
pub const OK: u8 = 0;
/// The invocation was wrong: unknown flag or command, a relative `--archive`,
/// a command this build does not implement, or a command only a person may
/// run, run without a terminal.
pub const USAGE: u8 = 1;
/// No archive exists at the resolved location. Recoverable by scanning.
pub const NO_ARCHIVE: u8 = 2;
/// The working directory is not inside a Git repository.
///
/// Distinct from every archive code: the archive may be perfectly healthy and
/// simply not the problem. A caller that retried against a different archive
/// would be fixing the wrong thing.
pub const NOT_A_REPO: u8 = 3;
/// An archive exists but this build cannot read it: not a Lore archive, a
/// schema from a newer or older build, an inconsistent migration ledger, or an
/// I/O failure while opening.
pub const ARCHIVE_UNREADABLE: u8 = 4;
/// Another process holds the archive's writer lock.
///
/// The only code for which retrying the same command unchanged is the correct
/// response, which is why it is not folded into any other.
pub const ARCHIVE_BUSY: u8 = 5;
/// The archive opened and was locked, but the scan itself did not complete.
/// Distinct from [`ARCHIVE_UNREADABLE`]: nothing is wrong with the archive's
/// shape, so "upgrade Lore" would be the wrong advice.
pub const SCAN_FAILED: u8 = 6;
/// `mem` refused a note, a path, a date, or an id another note already has,
/// and wrote nothing; or `mem reindex` found one id in two files.
///
/// Its own code because its next action is its own: fix the note. A missing
/// vault stays [`NO_ARCHIVE`] (create one), a busy vault [`ARCHIVE_BUSY`]
/// (retry).
pub const NOTE_REFUSED: u8 = 7;

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
    /// The archive's writer lock could not be taken.
    #[error(transparent)]
    Lock(#[from] LockError),
    /// A scan started but did not finish. `JobQueueError` is already content-free
    /// (it names queue conditions, never rows), so it is surfaced as-is.
    #[error("scan did not complete: {0}")]
    Scan(#[from] JobQueueError),
    /// The archive's persisted source roots could not be read.
    #[error(transparent)]
    SourceRoots(#[from] SourceRootError),
    /// The command needs a Git repository and the working directory is not in
    /// one.
    #[error("not inside a Git repository")]
    NotARepository,
    /// A command only a person may run had no terminal on its stdin, as under
    /// an agent's shell tool.
    #[error("`lorectl {0}` needs a person at a terminal")]
    NotATerminal(&'static str),
    /// A `mem` command failed.
    #[error(transparent)]
    Memory(#[from] VaultError),
    /// A `mem` proposal or review command failed.
    #[error(transparent)]
    Review(#[from] ReviewError),
}

impl CliError {
    /// Advice printed after the error, when there is a concrete next action.
    ///
    /// Named here rather than at the failure site so the same condition always
    /// suggests the same fix.
    #[must_use]
    pub fn advice(&self) -> Option<&'static str> {
        match self {
            CliError::Storage(StorageError::ArchiveMissing)
            | CliError::SourceRoots(SourceRootError::Storage(StorageError::ArchiveMissing)) => Some(
                "run `lorectl init` to set Lore up, or `lorectl scan` to build the archive alone",
            ),
            CliError::Usage(_) | CliError::NotImplemented(_) => Some("run `lorectl --help`"),
            CliError::Path(PathError::RelativeOverride) => {
                Some("give an absolute path starting with `/`")
            }
            CliError::Path(PathError::RelativeEnvOverride) => {
                Some("set LORE_ARCHIVE_DIR to an absolute path starting with `/`")
            }
            CliError::Lock(LockError::Held) => {
                Some("wait for the other process to finish, then run it again")
            }
            CliError::NotARepository => Some("run it from inside a Git repository"),
            CliError::NotATerminal(_) => Some(
                "agents propose notes and a person decides; ask the person to run it in their own terminal",
            ),
            CliError::Memory(error) => vault_advice(error),
            CliError::Review(ReviewError::Dirty(_)) => {
                Some("commit or discard your edit to that note, then run it again")
            }
            CliError::Review(ReviewError::Vault(VaultError::Conflict(_))) => {
                Some("accept it under another name with `--as <slug>`")
            }
            CliError::Review(ReviewError::NoProposal(_))
            | CliError::Review(ReviewError::Decided(..))
            | CliError::Review(ReviewError::DuplicateProposal(_)) => {
                Some("run `lorectl mem inbox` to see what is waiting")
            }
            CliError::Review(ReviewError::BadSlug(_)) => {
                Some("give a slug of lowercase letters, digits, and single `-`")
            }
            CliError::Review(ReviewError::Vault(error))
            | CliError::Review(ReviewError::Inbox(InboxError::Vault(error))) => vault_advice(error),
            _ => None,
        }
    }
}

fn vault_advice(error: &VaultError) -> Option<&'static str> {
    match error {
        VaultError::Missing(_) => Some("run `lorectl mem init` to create one"),
        VaultError::Busy(_) => Some("wait for the other process to finish, then run it again"),
        VaultError::RelativeDir(_) => Some("give an absolute path starting with `/`"),
        VaultError::NoHome => {
            Some("set LORE_VAULT_DIR to an absolute path starting with `/`")
        }
        VaultError::BadPath(..) => {
            Some("give a note path inside the vault, like `global/<slug>.md`")
        }
        VaultError::Conflict(_) => {
            Some("edit that file by hand, or give the new note another title")
        }
        VaultError::DuplicateId { .. } => {
            Some("leave out `id:` to get a new one, or edit the note that has it")
        }
        VaultError::NotFound(_) => Some("run `lorectl mem search <QUERY>` to find notes"),
        VaultError::Ambiguous(..) => {
            Some("give the note's id or its full path in the vault")
        }
        VaultError::Git(..) => Some(
            "git failed in the vault; run `git status` there to see why (Lore skips hooks and signing)",
        ),
        _ => None,
    }
}

/// The process exit code for an error. Total, and exhaustive by construction.
#[must_use]
pub fn exit_code(error: &CliError) -> u8 {
    match error {
        // A relative `--archive` is a mistyped flag, not a broken archive: the
        // user asked for a location that cannot mean one thing, so it is usage.
        CliError::Usage(_)
        | CliError::NotImplemented(_)
        | CliError::Path(_)
        | CliError::NotATerminal(_) => USAGE,
        CliError::Storage(error) => storage_exit_code(error),
        CliError::Lock(error) => lock_exit_code(error),
        CliError::Scan(_) => SCAN_FAILED,
        CliError::NotARepository => NOT_A_REPO,
        // Reading the persisted roots is an archive read. Delegate the wrapped
        // storage failure so a too-new schema still reports as one, rather than
        // being flattened into a generic "unreadable".
        CliError::SourceRoots(SourceRootError::Storage(error)) => storage_exit_code(error),
        CliError::SourceRoots(_) => ARCHIVE_UNREADABLE,
        CliError::Memory(error) => memory_exit_code(error),
        CliError::Review(error) => review_exit_code(error),
    }
}

/// Map a proposal or review failure onto the taxonomy. Every refusal writes
/// nothing, so it shares the note-refused code.
fn review_exit_code(error: &ReviewError) -> u8 {
    match error {
        ReviewError::NoProposal(_) | ReviewError::BadSlug(_) => USAGE,
        ReviewError::Decided(..)
        | ReviewError::Refused { .. }
        | ReviewError::DuplicateProposal(_)
        | ReviewError::Dirty(_) => NOTE_REFUSED,
        ReviewError::Inbox(InboxError::Field { .. } | InboxError::ScanFailed) => NOTE_REFUSED,
        ReviewError::Inbox(InboxError::Io(..)) => ARCHIVE_UNREADABLE,
        ReviewError::Inbox(InboxError::Vault(error)) | ReviewError::Vault(error) => {
            memory_exit_code(error)
        }
    }
}

/// Map a `mem` failure onto the taxonomy. Exhaustive, like the others.
fn memory_exit_code(error: &VaultError) -> u8 {
    match error {
        VaultError::Note(_)
        | VaultError::BadPath(..)
        | VaultError::Conflict(_)
        | VaultError::DuplicateId { .. }
        | VaultError::Pin(_) => NOTE_REFUSED,
        VaultError::Missing(_) => NO_ARCHIVE,
        VaultError::Busy(_) => ARCHIVE_BUSY,
        VaultError::RelativeDir(_)
        | VaultError::NoHome
        | VaultError::NotFound(_)
        | VaultError::Ambiguous(..) => USAGE,
        VaultError::Io(..) | VaultError::Git(..) | VaultError::Index(_) => ARCHIVE_UNREADABLE,
    }
}

/// Map a lock failure onto the taxonomy. Exhaustive for the same reason as
/// [`storage_exit_code`].
fn lock_exit_code(error: &LockError) -> u8 {
    match error {
        LockError::Held => ARCHIVE_BUSY,
        // Neither is "busy": retrying would fail identically. They are archive
        // failures the caller cannot fix by waiting.
        LockError::Io | LockError::Unsupported => ARCHIVE_UNREADABLE,
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
        let assigned = [
            OK,
            USAGE,
            NO_ARCHIVE,
            NOT_A_REPO,
            ARCHIVE_UNREADABLE,
            ARCHIVE_BUSY,
            SCAN_FAILED,
            NOTE_REFUSED,
        ];
        let mut seen = assigned.to_vec();
        seen.sort_unstable();
        seen.dedup();
        assert_eq!(seen.len(), assigned.len(), "codes must not collide");
        assert_eq!(
            seen,
            [0, 1, 2, 3, 4, 5, 6, 7],
            "8-9 stay free for later work"
        );
    }

    #[test]
    fn not_a_repository_is_its_own_code_and_not_an_archive_failure() {
        // Reserved by the first CLI slice, claimed here. It must never collapse
        // into an archive code: the archive may be fine and simply not the
        // problem, and a caller retrying against another archive would be
        // fixing the wrong thing.
        assert_eq!(exit_code(&CliError::NotARepository), NOT_A_REPO);
        assert_ne!(NOT_A_REPO, NO_ARCHIVE);
        assert_ne!(NOT_A_REPO, ARCHIVE_UNREADABLE);
        let advice = CliError::NotARepository
            .advice()
            .expect("a non-repository has a next action");
        assert!(advice.contains("Git repository"), "{advice}");
    }

    #[test]
    fn a_busy_archive_is_the_one_retryable_failure() {
        // Every other code means retrying the identical command fails
        // identically, so only this one may tell the caller to try again.
        assert_eq!(exit_code(&CliError::Lock(LockError::Held)), ARCHIVE_BUSY);
        let advice = CliError::Lock(LockError::Held)
            .advice()
            .expect("a busy archive has a next action");
        assert!(advice.contains("again"), "{advice}");
    }

    #[test]
    fn a_lock_that_cannot_be_taken_at_all_is_not_reported_as_busy() {
        // Waiting would never help, so these must not share BUSY's "retry" story.
        for error in [LockError::Io, LockError::Unsupported] {
            assert_eq!(exit_code(&CliError::Lock(error)), ARCHIVE_UNREADABLE);
            assert!(CliError::Lock(error).advice().is_none());
        }
    }

    #[test]
    fn a_storage_failure_keeps_its_code_through_the_source_roots_wrapper() {
        // Otherwise "upgrade Lore to open it" would be reported as a generic
        // unreadable archive, losing the only actionable part.
        let inner = StorageError::SchemaTooNew {
            archive: 9,
            supported: 5,
        };
        let wrapped = CliError::SourceRoots(SourceRootError::Storage(inner));
        assert_eq!(exit_code(&wrapped), ARCHIVE_UNREADABLE);
        let missing = CliError::SourceRoots(SourceRootError::Storage(StorageError::ArchiveMissing));
        assert_eq!(
            exit_code(&missing),
            NO_ARCHIVE,
            "a missing archive must not be flattened by the wrapper"
        );
    }

    #[test]
    fn a_refused_note_a_missing_vault_and_a_busy_vault_each_have_one_code() {
        use lore_memory::note::NoteError;
        let refused = [
            VaultError::Note(NoteError::Missing("title")),
            VaultError::BadPath("../x".into(), "`..` is not allowed".into()),
            VaultError::Conflict("global/x.md".into()),
            VaultError::DuplicateId {
                id: "01JC9V6Q4M8K2T7X3N5R0B1H6D".into(),
                existing: "global/x.md".into(),
            },
        ];
        for error in refused {
            let error = CliError::Memory(error);
            assert_eq!(exit_code(&error), NOTE_REFUSED);
            assert!(error.advice().is_none_or(|a| !a.contains("mem init")));
        }
        assert_ne!(NOTE_REFUSED, NO_ARCHIVE);
        let missing = CliError::Memory(VaultError::Missing("/tmp/v".into()));
        assert_eq!(exit_code(&missing), NO_ARCHIVE);
        assert!(missing.advice().is_some_and(|a| a.contains("mem init")));
        let busy = CliError::Memory(VaultError::Busy("/tmp/v".into()));
        assert_eq!(exit_code(&busy), ARCHIVE_BUSY);
        assert!(busy.advice().is_some_and(|a| a.contains("again")));
    }

    #[test]
    fn a_git_failure_says_to_look_at_git_not_to_upgrade_lore() {
        let git = CliError::Memory(VaultError::Git("commit".into(), "boom".into()));
        assert_eq!(exit_code(&git), ARCHIVE_UNREADABLE);
        let advice = git.advice().expect("a git failure has a next action");
        assert!(advice.contains("git status"), "{advice}");
        assert!(!advice.contains("upgrade"), "{advice}");
    }

    #[test]
    fn a_failed_scan_is_not_an_unreadable_archive() {
        // The archive opened and locked fine; telling the user to upgrade Lore
        // or suspect corruption would send them somewhere useless.
        let error = CliError::Scan(JobQueueError::Full { limit: 10 });
        assert_eq!(exit_code(&error), SCAN_FAILED);
        assert_ne!(exit_code(&error), ARCHIVE_UNREADABLE);
    }

    #[test]
    fn scan_failures_stay_content_free() {
        // Surfaced verbatim, so the queue's own wording carries the contract.
        for error in [
            JobQueueError::Full { limit: 7 },
            JobQueueError::InvalidState,
            JobQueueError::NotRunning,
        ] {
            let text = CliError::Scan(error).to_string();
            assert!(!text.contains('/'), "scan error leaked a path: {text}");
        }
    }

    #[test]
    fn a_wrapped_vault_error_shares_the_vault_advice() {
        let missing = CliError::Review(ReviewError::Vault(VaultError::Missing("/tmp/v".into())));
        assert_eq!(
            missing.advice(),
            Some("run `lorectl mem init` to create one")
        );

        let busy = CliError::Review(ReviewError::Vault(VaultError::Busy("/tmp/v".into())));
        assert_eq!(
            busy.advice(),
            Some("wait for the other process to finish, then run it again")
        );

        let git = CliError::Review(ReviewError::Vault(VaultError::Git(
            "commit".into(),
            "boom".into(),
        )));
        assert_eq!(
            git.advice(),
            Some(
                "git failed in the vault; run `git status` there to see why (Lore skips hooks and signing)"
            )
        );

        let inbox_missing = CliError::Review(ReviewError::Inbox(InboxError::Vault(
            VaultError::Missing("/tmp/v".into()),
        )));
        assert_eq!(
            inbox_missing.advice(),
            Some("run `lorectl mem init` to create one")
        );
    }

    #[test]
    fn a_source_roots_missing_archive_shares_the_missing_archive_advice() {
        let missing = CliError::SourceRoots(SourceRootError::Storage(StorageError::ArchiveMissing));
        assert_eq!(
            missing.advice(),
            Some("run `lorectl init` to set Lore up, or `lorectl scan` to build the archive alone")
        );
    }

    #[test]
    fn relative_path_errors_give_clear_advice() {
        let override_err = CliError::Path(PathError::RelativeOverride);
        assert_eq!(
            override_err.advice(),
            Some("give an absolute path starting with `/`")
        );

        let env_err = CliError::Path(PathError::RelativeEnvOverride);
        assert_eq!(
            env_err.advice(),
            Some("set LORE_ARCHIVE_DIR to an absolute path starting with `/`")
        );

        let vault_rel = CliError::Memory(VaultError::RelativeDir("rel".into()));
        assert_eq!(
            vault_rel.advice(),
            Some("give an absolute path starting with `/`")
        );

        let no_home = CliError::Memory(VaultError::NoHome);
        assert_eq!(
            no_home.advice(),
            Some("set LORE_VAULT_DIR to an absolute path starting with `/`")
        );
    }

    #[test]
    fn bad_note_path_gives_actionable_advice() {
        let dotdot = CliError::Memory(VaultError::BadPath(
            "../note".into(),
            "`..` is not allowed".into(),
        ));
        assert_eq!(
            dotdot.advice(),
            Some("give a note path inside the vault, like `global/<slug>.md`")
        );

        let non_ascii_abs = CliError::Memory(VaultError::BadPath(
            "/etc/日本語".into(),
            "absolute paths outside the vault are not allowed".into(),
        ));
        assert_eq!(
            non_ascii_abs.advice(),
            Some("give a note path inside the vault, like `global/<slug>.md`")
        );

        let not_md = CliError::Memory(VaultError::BadPath(
            "global/note.txt".into(),
            "expected a `.md` file".into(),
        ));
        assert_eq!(
            not_md.advice(),
            Some("give a note path inside the vault, like `global/<slug>.md`")
        );
    }

    #[test]
    fn vault_not_found_and_ambiguous_give_actionable_advice() {
        let not_found = CliError::Memory(VaultError::NotFound("test-note".into()));
        assert_eq!(
            not_found.advice(),
            Some("run `lorectl mem search <QUERY>` to find notes")
        );

        let ambiguous =
            CliError::Memory(VaultError::Ambiguous("prefix".into(), "a.md, b.md".into()));
        assert_eq!(
            ambiguous.advice(),
            Some("give the note's id or its full path in the vault")
        );
    }

    #[test]
    fn review_errors_give_inbox_and_slug_advice() {
        let no_prop =
            CliError::Review(ReviewError::NoProposal("01JC0000000000000000000000".into()));
        assert_eq!(
            no_prop.advice(),
            Some("run `lorectl mem inbox` to see what is waiting")
        );

        let decided = CliError::Review(ReviewError::Decided(
            "01JC0000000000000000000000".into(),
            "accepted",
        ));
        assert_eq!(
            decided.advice(),
            Some("run `lorectl mem inbox` to see what is waiting")
        );

        let dup = CliError::Review(ReviewError::DuplicateProposal(
            "01JC0000000000000000000000".into(),
        ));
        assert_eq!(
            dup.advice(),
            Some("run `lorectl mem inbox` to see what is waiting")
        );

        let bad_slug = CliError::Review(ReviewError::BadSlug("Bad/Slug".into()));
        assert_eq!(
            bad_slug.advice(),
            Some("give a slug of lowercase letters, digits, and single `-`")
        );
    }
}
