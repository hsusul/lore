//! Did the work an agent did reach a commit?
//!
//! ## The claim, and its limits
//!
//! Lore addresses recorded file content by its **Git blob object id** — the same
//! address Git itself would assign. Git hashes content, not commits, so an oid
//! recorded at ingest still matches after a rebase, squash, cherry-pick or
//! amend. That is what makes a landing claim survive ordinary history rewriting
//! without any commit tracking.
//!
//! The ladder is discrete, and each rung is something Lore can point at:
//!
//! - [`Landing::Committed`] — the exact content is reachable from a ref. Proven.
//! - [`Landing::Staged`] — the object exists in the repository but no ref reaches
//!   it. That is work sitting in an index, or on a commit that was rewritten
//!   away. Actionable, and distinct from both of its neighbours.
//! - [`Landing::NoObservedLanding`] — the content is not in the object database.
//!   An **absence of observation**, not a finding: the file may have been edited
//!   again before committing, or committed in a repository Lore cannot see.
//! - [`Landing::NotAssessed`] — Lore never knew the resulting content, so it has
//!   no question to ask. `edit` events are here (see below), as is anything
//!   recorded before this became possible.
//!
//! Nothing here may be rendered as *lost*, *abandoned*, *uncommitted*, or
//! *never landed*. Those assert something about the developer's work; these
//! variants only ever assert something about what Lore looked for and found.
//!
//! ## Why `edit` is not assessed
//!
//! A `create` records the whole new file, so its post-image is known exactly. An
//! `edit` does not: Codex records a unified diff, and Claude Code records an
//! `old_string`/`new_string` fragment. Neither yields the resulting file without
//! the pre-image, so no honest oid can be computed and the answer is
//! `NotAssessed` rather than a guess. Reading the file from disk at ingest would
//! produce an oid, but it would be the oid of whatever the file happens to
//! contain *now* — attributing later human edits to the agent.
//!
//! ## Where the lookup happens
//!
//! Against the repository's object store, reached through **any** worktree Lore
//! knows for it — not the worktree the agent used. Agents routinely work in
//! throwaway worktrees that are deleted when the task ends (one repository in
//! the author's own archive has 37 recorded worktrees, 36 of them gone). Linked
//! worktrees share one object database, so the commits survive in whichever
//! worktree still exists; looking only at the recorded path would report
//! `NoObservedLanding` for every one of them.

use std::path::{Path, PathBuf};

use rusqlite::{Connection, OptionalExtension};

use crate::storage::Result;

/// Git's blob object id for `content`, as `git hash-object` would compute it.
///
/// The header is part of the hashed input — Git hashes `"blob <len>\0"` followed
/// by the bytes — so this is directly comparable with the ids in a repository's
/// object database rather than merely being "a hash of the file".
#[must_use]
pub fn blob_oid(content: &[u8]) -> String {
    // Infallible in practice for sha1 over an in-memory slice; a hasher error
    // would mean the algorithm itself failed, so it degrades to an empty id
    // rather than panicking in an archive path.
    gix::objs::compute_hash(gix::hash::Kind::Sha1, gix::objs::Kind::Blob, content)
        .map(|oid| oid.to_string())
        .unwrap_or_default()
}

/// The algorithm [`blob_oid`] used, recorded alongside the value because an oid
/// is only comparable against a repository using the same object format.
pub const CONTENT_OID_ALGO: &str = "git-sha1";

/// What Lore can say about whether recorded content reached a commit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Landing {
    /// The exact content is reachable from a ref.
    Committed,
    /// The content is in the **index**: staged and not yet committed. A
    /// `git reset --hard` or `git checkout` discards it, so this is the one
    /// rung that may say so.
    Staged,
    /// The object exists but is neither in the index nor reachable from a ref —
    /// typically a commit that was rewritten away.
    ///
    /// Split from [`Landing::Staged`] because the obvious advice is wrong here:
    /// a reset does **not** discard it (it is already unreferenced, and the
    /// reflog may still hold the commit), so telling a user it would is the
    /// forbidden inference wearing friendlier words.
    Unreferenced,
    /// Not present in the object database. An absence of observation.
    NoObservedLanding,
    /// Lore never knew the resulting content, so it asked nothing.
    NotAssessed,
}

impl Landing {
    /// Wording safe to show a user. Deliberately drab: every phrase describes
    /// what Lore looked for, never what the developer did or failed to do.
    #[must_use]
    pub fn describe(self) -> &'static str {
        match self {
            Landing::Committed => "in a commit",
            Landing::Staged => "staged, not yet committed",
            Landing::Unreferenced => "in the repository but on no branch",
            Landing::NoObservedLanding => "no observed landing",
            Landing::NotAssessed => "not assessed",
        }
    }
}

/// A repository's worktrees, newest-known first, filtered to those that still
/// exist on disk.
///
/// Returns every live candidate rather than one: linked worktrees share an
/// object database, so any of them can answer the question, and the one the
/// agent used is frequently gone.
pub fn live_worktrees(conn: &Connection, repository_id: &str) -> Result<Vec<PathBuf>> {
    let mut stmt = conn.prepare(
        "SELECT path FROM worktree WHERE repository_id = ?1 ORDER BY is_primary DESC, path",
    )?;
    let rows = stmt
        .query_map([repository_id], |row| row.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows
        .into_iter()
        .map(PathBuf::from)
        .filter(|p| p.join(".git").exists())
        .collect())
}

/// Every blob reachable from any ref in a repository, built once and queried
/// many times.
///
/// Landing means "was ever committed", not "is in the current tip", so this
/// walks **commit history**, not just the tips' trees: work that landed and was
/// later changed still landed. Trees are memoized by object id, which is what
/// makes the walk affordable — successive commits share almost all of their
/// tree structure.
pub struct LandingIndex {
    blobs: std::collections::HashSet<gix::ObjectId>,
}

impl LandingIndex {
    /// Build the index for the repository containing `worktree`.
    ///
    /// Returns `None` when the repository cannot be opened — which is "cannot
    /// answer", not "the content is absent".
    #[must_use]
    pub fn build(worktree: &Path) -> Option<Self> {
        let repo = gix::discover(worktree).ok()?;
        let mut blobs = std::collections::HashSet::new();
        let mut seen_trees = std::collections::HashSet::new();

        // `Reference::id()` PANICS on a symbolic target (HEAD, and any ref that
        // points at another ref), and a real repository is full of them. Peel
        // instead: it resolves symbolic targets and unwraps annotated tags, and
        // returns an error rather than panicking on anything it cannot follow.
        let tips: Vec<gix::ObjectId> = repo
            .references()
            .ok()?
            .all()
            .ok()?
            .filter_map(std::result::Result::ok)
            .filter_map(|r| r.into_fully_peeled_id().ok())
            .map(gix::Id::detach)
            .filter(|id| {
                repo.find_object(*id)
                    .map(|o| o.kind == gix::object::Kind::Commit)
                    .unwrap_or(false)
            })
            .collect();

        // Ancestry from every ref, so history is covered rather than only tips.
        if let Ok(walk) = repo.rev_walk(tips).all() {
            for info in walk.flatten() {
                let Ok(object) = repo.find_object(info.id) else {
                    continue;
                };
                let Ok(commit) = object.try_into_commit() else {
                    continue;
                };
                let Ok(tree) = commit.tree_id() else { continue };
                collect_tree(&repo, tree.detach(), &mut blobs, &mut seen_trees);
            }
        }
        Some(Self { blobs })
    }

    /// Whether this repository's history contains `oid`.
    #[must_use]
    pub fn contains(&self, oid: &str) -> bool {
        gix::ObjectId::from_hex(oid.as_bytes())
            .map(|id| self.blobs.contains(&id))
            .unwrap_or(false)
    }

    /// How many blobs the index holds. Useful for asserting a build was not
    /// silently empty.
    #[must_use]
    pub fn len(&self) -> usize {
        self.blobs.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.blobs.is_empty()
    }
}

/// Add every blob in `tree` (recursively) to `out`, skipping trees already seen.
fn collect_tree(
    repo: &gix::Repository,
    tree: gix::ObjectId,
    out: &mut std::collections::HashSet<gix::ObjectId>,
    seen: &mut std::collections::HashSet<gix::ObjectId>,
) {
    if !seen.insert(tree) {
        return;
    }
    let Ok(obj) = repo.find_object(tree) else {
        return;
    };
    let Ok(tree) = obj.try_into_tree() else {
        return;
    };
    let Ok(entries) = tree.iter().collect::<std::result::Result<Vec<_>, _>>() else {
        return;
    };
    for entry in entries {
        let id = entry.oid().to_owned();
        if entry.mode().is_tree() {
            collect_tree(repo, id, out, seen);
        } else {
            out.insert(id);
        }
    }
}

/// Ask one repository whether `oid` is committed, merely present, or absent.
///
/// Convenience over [`LandingIndex`] for a single lookup; classify many oids by
/// building the index once instead.
#[must_use]
pub fn lookup_in_worktree(worktree: &Path, oid: &str) -> Option<Landing> {
    let index = LandingIndex::build(worktree)?;
    Some(classify(worktree, &index, oid))
}

/// Classify `oid` against an already-built index.
#[must_use]
pub fn classify(worktree: &Path, index: &LandingIndex, oid: &str) -> Landing {
    if index.contains(oid) {
        return Landing::Committed;
    }
    // Not in history. Three outcomes, not two: in the index (a reset discards
    // it), merely present (a reset does not), or absent. Collapsing the first
    // two produces advice that is wrong half the time.
    let Ok(repo) = gix::discover(worktree) else {
        return Landing::NoObservedLanding;
    };
    let Ok(id) = gix::ObjectId::from_hex(oid.as_bytes()) else {
        return Landing::NoObservedLanding;
    };
    if repo.find_object(id).is_err() {
        return Landing::NoObservedLanding;
    }
    if index_contains(&repo, id) {
        Landing::Staged
    } else {
        Landing::Unreferenced
    }
}

/// Whether the repository's index holds `blob`.
///
/// A missing or unreadable index is treated as "not staged": the object is
/// known to exist, and claiming it is staged on a guess is exactly the
/// overclaim this split exists to prevent.
fn index_contains(repo: &gix::Repository, blob: gix::ObjectId) -> bool {
    repo.index()
        .map(|index| index.entries().iter().any(|e| e.id == blob))
        .unwrap_or(false)
}

/// Resolve landing for one recorded content oid, trying every live worktree of
/// the repository until one can answer.
pub fn resolve(conn: &Connection, repository_id: &str, oid: Option<&str>) -> Result<Landing> {
    let Some(oid) = oid else {
        return Ok(Landing::NotAssessed);
    };
    let mut answer = Landing::NoObservedLanding;
    for worktree in live_worktrees(conn, repository_id)? {
        let Some(index) = LandingIndex::build(&worktree) else {
            continue; // could not answer; not evidence of absence
        };
        match classify(&worktree, &index, oid) {
            // A positive finding anywhere settles it: the content is in that
            // repository's history, whichever worktree answered.
            Landing::Committed => return Ok(Landing::Committed),
            Landing::Staged => answer = Landing::Staged,
            Landing::Unreferenced if answer != Landing::Staged => {
                answer = Landing::Unreferenced;
            }
            Landing::Unreferenced | Landing::NoObservedLanding | Landing::NotAssessed => {}
        }
    }
    Ok(answer)
}

/// The repository a file event belongs to, via its segment.
pub fn event_repository(conn: &Connection, file_event_id: &str) -> Result<Option<String>> {
    Ok(conn
        .query_row(
            "SELECT sg.repository_id FROM file_event f
             JOIN session_segment sg ON sg.id = f.segment_id
             WHERE f.id = ?1 AND sg.repository_id IS NOT NULL",
            [file_event_id],
            |row| row.get::<_, String>(0),
        )
        .optional()?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blob_oid_matches_git_hash_object() {
        // The canonical example: an empty blob's id is a fixed, well-known
        // constant in every Git repository on earth. If the header framing were
        // wrong, this would not match.
        assert_eq!(blob_oid(b""), "e69de29bb2d1d6434b8b29ae775ad8c2e48c5391");
        // `git hash-object` of "hello\n".
        assert_eq!(
            blob_oid(b"hello\n"),
            "ce013625030ba8dba906f756967f9e9ca394464a"
        );
    }

    #[test]
    fn identical_content_addresses_identically() {
        // The property landing depends on: the same content produces the same
        // id, which is why a rebase cannot break a landing claim.
        assert_eq!(blob_oid(b"fn main() {}\n"), blob_oid(b"fn main() {}\n"));
        assert_ne!(blob_oid(b"a\n"), blob_oid(b"b\n"));
    }

    #[test]
    fn a_missing_oid_is_not_assessed_rather_than_absent() {
        // The distinction the whole module exists to protect: "Lore did not
        // look" must never render as "the work did not land".
        let conn = crate::storage::open_in_memory().unwrap();
        assert_eq!(resolve(&conn, "repo", None).unwrap(), Landing::NotAssessed);
    }

    #[test]
    fn every_rung_reads_as_an_observation_not_a_verdict() {
        for landing in [
            Landing::Committed,
            Landing::Staged,
            Landing::Unreferenced,
            Landing::NoObservedLanding,
            Landing::NotAssessed,
        ] {
            let text = landing.describe();
            for forbidden in ["lost", "abandoned", "uncommitted", "unfinished", "never"] {
                assert!(
                    !text.contains(forbidden),
                    "`{text}` asserts `{forbidden}`, which Lore cannot prove"
                );
            }
        }
    }

    #[test]
    fn the_rungs_are_distinguishable() {
        // Collapsing any two would lose the difference between "we looked and
        // it is not there" and "we never looked".
        let all = [
            Landing::Committed,
            Landing::Staged,
            Landing::Unreferenced,
            Landing::NoObservedLanding,
            Landing::NotAssessed,
        ];
        let mut texts: Vec<&str> = all.iter().map(|l| l.describe()).collect();
        texts.sort_unstable();
        texts.dedup();
        assert_eq!(texts.len(), all.len());
    }

    #[test]
    fn a_repository_with_no_live_worktree_reports_no_observation() {
        // Not an error and not a negative finding: Lore had nowhere to look.
        let conn = crate::storage::open_in_memory().unwrap();
        assert_eq!(
            resolve(&conn, "no-such-repo", Some(&blob_oid(b"x"))).unwrap(),
            Landing::NoObservedLanding
        );
    }
}
