//! Errors a replay reports instead of panicking.

use std::path::{Path, PathBuf};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("could not {action} {}: {source}", path.display())]
    Io {
        action: &'static str,
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("{} is not valid JSON: {source}", path.display())]
    Json {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
    #[error(transparent)]
    Paths(#[from] lore_core::paths::PathError),
    #[error(transparent)]
    Storage(#[from] lore_core::storage::StorageError),
    #[error("archive query failed: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("no case `{0}` in the cases file")]
    CaseNotFound(String),
    #[error("the arms file has no `{arm}` text for case `{case}`")]
    ArmTextMissing { case: String, arm: String },
    #[error("`git {args}` failed: {stderr}")]
    Git { args: String, stderr: String },
    #[error("could not start `{program}`: {source}")]
    Spawn {
        program: String,
        #[source]
        source: std::io::Error,
    },
    #[error(
        "{} exists and is not a symlink; refusing to replace a real Claude Code project folder",
        .0.display()
    )]
    ProjectDirOccupied(PathBuf),
    #[error("the home directory is unknown (HOME is unset)")]
    NoHome,
    #[error("the model judge gave no answer: {0}")]
    Judge(String),
    #[error("case `{0}` has no label; sweep screens each case with its label's grader")]
    Unlabeled(String),
    #[error("the grades mix {0}; pass --harness N or --settings KEY=VALUE,... (harness, model, trim, max_turns) to report one group")]
    MixedSettings(String),
}

impl Error {
    /// An I/O failure, with what was being done and to which path.
    pub fn io(action: &'static str, path: &Path, source: std::io::Error) -> Self {
        Self::Io {
            action,
            path: path.to_owned(),
            source,
        }
    }
}

pub type Result<T> = std::result::Result<T, Error>;
