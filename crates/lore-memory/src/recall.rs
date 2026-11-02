//! Recall: the notes that apply where someone is working, each labeled with
//! how well its anchors still match the code.
//!
//! The scope comes from the directory: global notes always, plus the notes of
//! the repository that contains it. Ranking is the index's BM25, then notes
//! whose anchors changed sink below the rest and notes whose files are gone
//! sink to the bottom. A query with no words recalls every note in scope, in
//! path order before that sort. Recall goes through [`Vault::index`], so
//! notes a person edited, renamed, or deleted by hand are picked up first; it
//! never writes a note file or the working tree. A file that no longer parses
//! cannot be searched, so recall names it in `skipped` instead of dropping it
//! unseen.

use std::fs;
use std::io;
use std::path::Path;

use crate::anchor::{Freshness, Located, WorkTree};
use crate::index::has_words;
use crate::note::{Note, NoteError};
use crate::repo::RepoContext;
use crate::vault::{Vault, VaultError};

/// One recalled note.
#[derive(Debug, Clone, PartialEq)]
pub struct Recalled {
    /// The note file, relative to the vault.
    pub path: String,
    /// The file name without `.md`.
    pub slug: String,
    pub note: Note,
    /// A one-line extract of the body around the match, or its start when
    /// the query has no words.
    pub excerpt: String,
    /// FTS5 BM25. Lower is a better match. 0 when the query has no words.
    pub score: f64,
    /// The worst of the anchors, `Unanchored` when the note has none, or
    /// `Unchecked` when it has some but there is no working tree.
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
    /// Markdown files in the vault that are not valid notes, with the reason,
    /// as `Vault::reindex` reports them. Recall cannot search them, so the
    /// answer may be missing what they hold.
    pub skipped: Vec<(String, NoteError)>,
}

/// Notes matching `query` that apply in `cwd`, best first, at most `limit`.
/// When `query` has no words, every active note that applies in `cwd`.
pub fn recall(vault: &Vault, query: &str, cwd: &Path, limit: usize) -> Result<Recall, VaultError> {
    let context = RepoContext::discover(cwd);
    let repo_scope = context.key().map(|key| format!("repo:{}", key.as_str()));
    let index = vault.index()?;
    let hits = if has_words(query) {
        index.search_active(query, repo_scope.as_deref(), limit)?
    } else {
        index.list_active(repo_scope.as_deref(), limit)?
    };
    let mut skipped = Vec::new();
    for rel in index.unparsed()? {
        if let Some(Err(error)) = parse_file(vault, &rel)? {
            skipped.push((rel, error));
        }
    }
    let tree = context.root().map(WorkTree::new);
    let mut notes = Vec::new();
    for hit in hits {
        // The file changed after the index read it: gone, or no longer a note.
        let note = match parse_file(vault, &hit.path)? {
            Some(Ok(note)) => note,
            Some(Err(error)) => {
                skipped.push((hit.path, error));
                continue;
            }
            None => continue,
        };
        let (anchors, freshness) = match &tree {
            Some(tree) => {
                let anchors: Vec<Located> = note.anchors.iter().map(|a| tree.locate(a)).collect();
                let freshness = Freshness::of_note(&anchors);
                (anchors, freshness)
            }
            None if note.anchors.is_empty() => (Vec::new(), Freshness::Unanchored),
            None => (Vec::new(), Freshness::Unchecked),
        };
        notes.push(Recalled {
            freshness,
            anchors,
            path: hit.path,
            slug: hit.slug,
            note,
            excerpt: hit.excerpt,
            score: hit.score,
        });
    }
    notes.sort_by_key(|recalled| tier(recalled.freshness));
    Ok(Recall {
        context,
        notes,
        skipped,
    })
}

/// Read and parse one vault file the way `Vault::reindex` does: a file that
/// is not UTF-8 is not a note. `None` when the file is gone.
fn parse_file(vault: &Vault, rel: &str) -> Result<Option<Result<Note, NoteError>>, VaultError> {
    match fs::read_to_string(vault.root().join(rel)) {
        Ok(text) => Ok(Some(Note::parse(&text))),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) if error.kind() == io::ErrorKind::InvalidData => Ok(Some(Err(
            NoteError::NotANote("the file is not valid UTF-8"),
        ))),
        Err(error) => Err(VaultError::Io(format!("read {rel}"), error)),
    }
}

fn tier(freshness: Freshness) -> u8 {
    match freshness {
        Freshness::Fresh
        | Freshness::Moved { .. }
        | Freshness::Unanchored
        | Freshness::Unchecked => 0,
        Freshness::Changed => 1,
        Freshness::Gone => 2,
    }
}
