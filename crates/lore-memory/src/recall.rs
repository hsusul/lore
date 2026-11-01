//! Recall: the notes that apply where someone is working, each labeled with
//! how well its anchors still match the code.
//!
//! The scope comes from the directory: global notes always, plus the notes of
//! the repository that contains it. Ranking is the index's BM25, then notes
//! whose anchors changed sink below the rest and notes whose files are gone
//! sink to the bottom. Recall reads the vault and the working tree and writes
//! neither.

use std::path::Path;

use crate::anchor::{Freshness, Located, WorkTree};
use crate::note::Note;
use crate::repo::RepoContext;
use crate::vault::{Vault, VaultError};

/// One recalled note.
#[derive(Debug, Clone, PartialEq)]
pub struct Recalled {
    /// The note file, relative to the vault.
    pub path: String,
    pub note: Note,
    /// A one-line extract of the body around the match.
    pub excerpt: String,
    /// FTS5 BM25. Lower is a better match.
    pub score: f64,
    /// The worst of the anchors, or `Unanchored`.
    pub freshness: Freshness,
    /// Each anchor checked against the working tree, in the note's order.
    /// Empty when there is no working tree to check against.
    pub anchors: Vec<Located>,
}

/// What recall found, and the repository it scoped to.
#[derive(Debug, Clone, PartialEq)]
pub struct Recall {
    pub context: RepoContext,
    pub notes: Vec<Recalled>,
}

/// Notes matching `query` that apply in `cwd`, best first, at most `limit`.
pub fn recall(vault: &Vault, query: &str, cwd: &Path, limit: usize) -> Result<Recall, VaultError> {
    let context = RepoContext::discover(cwd);
    let repo_scope = context.key().map(|key| format!("repo:{}", key.as_str()));
    let hits = vault
        .index()?
        .search_active(query, repo_scope.as_deref(), limit)?;
    let tree = context.root().map(WorkTree::new);
    let mut notes: Vec<Recalled> = hits
        .into_iter()
        .filter_map(|hit| {
            let note = Note::parse(&vault.read(&hit.path).ok()?).ok()?;
            let anchors: Vec<Located> = match &tree {
                Some(tree) => note.anchors.iter().map(|a| tree.locate(a)).collect(),
                None => Vec::new(),
            };
            Some(Recalled {
                freshness: Freshness::of_note(&anchors),
                anchors,
                path: hit.path,
                note,
                excerpt: hit.excerpt,
                score: hit.score,
            })
        })
        .collect();
    notes.sort_by_key(|recalled| tier(recalled.freshness));
    Ok(Recall { context, notes })
}

fn tier(freshness: Freshness) -> u8 {
    match freshness {
        Freshness::Fresh | Freshness::Moved { .. } | Freshness::Unanchored => 0,
        Freshness::Changed => 1,
        Freshness::Gone => 2,
    }
}
