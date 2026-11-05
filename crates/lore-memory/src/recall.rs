//! Recall: the notes that apply where someone is working, each labeled with
//! how well its anchors still match the code.
//!
//! The scope comes from the directory: global notes always, plus the notes of
//! the repository that contains it. Ranking is the index's BM25, then notes
//! whose anchors changed sink below the rest and notes whose files are gone
//! sink to the bottom. A query with no words recalls every note in scope:
//! the repository's own notes before global ones, then notes with more
//! sources, then newer, then by path, before that sort. The limit applies after that sort, to a
//! bounded set of candidates, so a stale note never takes a fresher note's
//! slot among those candidates. Recall goes through [`Vault::index`], so
//! notes a person edited, renamed, or deleted by hand are picked up first; it
//! never writes a note file or the working tree. A file that no longer parses
//! cannot be searched, so recall names it in `skipped` instead of dropping it
//! unseen.
//!
//! [`anchored_to`] narrows recall to the notes anchored to one file, so an
//! agent about to change that file can be shown them.

use std::fs;
use std::io;
use std::path::Path;

use crate::anchor::{Freshness, Located, WorkTree};
use crate::index::has_words;
use crate::note::{Note, NoteError, RelPath, Scope};
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

/// How many index hits recall checks per note it returns. Each candidate costs
/// a file read and an anchor check, so the set is bounded rather than the
/// whole scope.
const CANDIDATES_PER_NOTE: usize = 5;
/// The most candidates recall checks, unless `limit` alone asks for more.
const MAX_CANDIDATES: usize = 200;

/// Notes matching `query` that apply in `cwd`, best first, at most `limit`.
/// When `query` has no words, every active note that applies in `cwd`.
pub fn recall(vault: &Vault, query: &str, cwd: &Path, limit: usize) -> Result<Recall, VaultError> {
    let context = RepoContext::discover(cwd);
    let repo_scope = context.key().map(|key| format!("repo:{}", key.as_str()));
    let index = vault.index()?;
    let candidates = limit
        .saturating_mul(CANDIDATES_PER_NOTE)
        .min(MAX_CANDIDATES)
        .max(limit);
    let ranked = has_words(query);
    // With no words there is nothing to rank by in the index, so every note in
    // scope is read and ordered here; a path order would hand the slots to
    // whichever notes sort first by name.
    let hits = if ranked {
        index.search_active(query, repo_scope.as_deref(), candidates)?
    } else {
        index.list_active(repo_scope.as_deref(), usize::MAX)?
    };
    let mut skipped = Vec::new();
    for rel in index.unparsed()? {
        if let Some(Err(error)) = parse_file(vault, &rel)? {
            skipped.push((rel, error));
        }
    }
    let mut parsed = Vec::new();
    for hit in hits {
        // The file changed after the index read it: gone, or no longer a note.
        match parse_file(vault, &hit.path)? {
            Some(Ok(note)) => parsed.push((hit, note)),
            Some(Err(error)) => skipped.push((hit.path, error)),
            None => {}
        }
    }
    if !ranked {
        parsed.sort_by(|(a, x), (b, y)| {
            matches!(y.scope, Scope::Repo(_))
                .cmp(&matches!(x.scope, Scope::Repo(_)))
                .then(y.sources.len().cmp(&x.sources.len()))
                .then(y.created.cmp(&x.created))
                .then(a.path.cmp(&b.path))
        });
        parsed.truncate(candidates);
    }
    let tree = context.root().map(WorkTree::new);
    let mut notes = Vec::new();
    for (hit, note) in parsed {
        let (anchors, freshness) = check_note(&note, &context, tree.as_ref());
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
    // Stable, so BM25 order (or path order) holds within a tier.
    notes.sort_by_key(|recalled| tier(recalled.freshness));
    notes.truncate(limit);
    Ok(Recall {
        context,
        notes,
        skipped,
    })
}

/// The active notes that apply in `cwd` with an anchor on `path`, a path
/// relative to the root of the repository that holds `cwd`, best first, at
/// most `limit`. An anchor whose file is gone from the working tree is on the
/// file it was renamed to, if any, not on its old path. Changed notes sink below the rest, as
/// in [`recall`]. Outside a repository, nothing is anchored.
pub fn anchored_to(
    vault: &Vault,
    cwd: &Path,
    path: &RelPath,
    limit: usize,
) -> Result<Vec<Recalled>, VaultError> {
    let context = RepoContext::discover(cwd);
    let Some(root) = context.root() else {
        return Ok(Vec::new());
    };
    let tree = WorkTree::new(root);
    let repo_scope = context.key().map(|key| format!("repo:{}", key.as_str()));
    let hits = vault
        .index()?
        .list_active(repo_scope.as_deref(), usize::MAX)?;
    let mut notes = Vec::new();
    for hit in hits {
        let Some(Ok(note)) = parse_file(vault, &hit.path)? else {
            continue;
        };
        // Where each anchor's file is now: its own path while that file
        // exists, else the file it was renamed to.
        let on_path = note.anchors.iter().any(|anchor| {
            if root.join(anchor.path.as_str()).exists() {
                anchor.path == *path
            } else {
                tree.locate(anchor).path == *path
            }
        });
        if !on_path {
            continue;
        }
        let (anchors, freshness) = check_note(&note, &context, Some(&tree));
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
    notes.truncate(limit);
    Ok(notes)
}

/// Each of `note`'s anchors checked against `tree`, the working tree of
/// `context`, and the worst of them. The tree is used only when the note
/// applies in `context`, so a note from another repository is `Unchecked`.
/// Recall and the MCP `read_note` tool both label notes with this.
pub fn check_note(
    note: &Note,
    context: &RepoContext,
    tree: Option<&WorkTree>,
) -> (Vec<Located>, Freshness) {
    let applies = match &note.scope {
        Scope::Global => true,
        Scope::Repo(key) => context.key() == Some(key),
    };
    match tree.filter(|_| applies) {
        Some(tree) => {
            let anchors: Vec<Located> = note.anchors.iter().map(|a| tree.locate(a)).collect();
            let freshness = Freshness::of_note(&anchors);
            (anchors, freshness)
        }
        None if note.anchors.is_empty() => (Vec::new(), Freshness::Unanchored),
        None => (Vec::new(), Freshness::Unchecked),
    }
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
