//! What an agent is given at session start: the notes that apply where it
//! runs, as the section `lorectl hook session-start` prints, and the same view
//! for the desktop app's "What agents read".
//!
//! The notes are recall's, in recall's order, with two changes before the
//! cap. A note whose title is near-identical to one ranked above it is left
//! out, so one lesson written twice takes one slot; recall ranks the fresher
//! note first, then the repository's, then the one with more sources, then
//! the newer, so that is the one kept (see [`crate::duplicates`];
//! `lorectl mem reindex` names the pair so a person can retire one). And
//! when the repository's notes and global notes are both in scope, the cap is
//! shared: global notes get at least [`GLOBAL_SHARE`] slots, the
//! repository's notes the rest, and either side may use slots the other
//! leaves empty, so a person's general rules still reach an agent in a repo
//! with many notes of its own. In a repository, its notes come first under
//! one heading and global notes after under another, so an agent can tell a
//! rule for this repository from one for every repository.

use std::path::Path;

use crate::duplicates::near_identical;
use crate::inbox;
use crate::index::Index;
use crate::note::Scope;
use crate::recall::{self, Recalled};
use crate::repo::RepoContext;
use crate::vault::{Vault, VaultError};

/// At most this many notes at session start. Experiment Q3 saw agents follow
/// them at 5, 10, and 25; the cap is set by what the context costs.
pub const NOTE_CAP: usize = 10;
/// The notes section stops adding note lines past this many bytes.
pub const CHAR_CAP: usize = 2_000;
/// When both kinds are in scope, global notes get at least this many of the
/// [`NOTE_CAP`] slots, if there are that many.
pub const GLOBAL_SHARE: usize = 4;
/// How many of recall's notes are considered for the slots. Recall checks
/// each one's anchors, so the set is bounded rather than the whole scope.
const POOL: usize = 200;

/// The notes section for one working directory.
#[derive(Debug)]
pub struct SessionStart {
    pub context: RepoContext,
    /// The notes chosen for the section, best first. The first `shown` fit
    /// the section.
    pub notes: Vec<Recalled>,
    pub shown: usize,
    /// Every distinct active note that applies in this directory, chosen or
    /// not: a note left out as a near-duplicate of a chosen one is not
    /// counted.
    pub total: usize,
    /// The section exactly as an agent reads it.
    pub text: String,
}

/// The notes section for `cwd`, or `None` when no note applies there.
pub fn session_start(vault: &Vault, cwd: &Path) -> Result<Option<SessionStart>, VaultError> {
    let found = recall::recall(vault, "", cwd, POOL)?;
    if found.notes.is_empty() {
        return Ok(None);
    }
    // Recall left the index fresh, so counting what it capped is one query.
    let scope = found.context.key().map(|k| format!("repo:{}", k.as_str()));
    let in_scope = Index::open(&vault.lore_dir())
        .ok()
        .flatten()
        .and_then(|index| index.count_active(scope.as_deref()).ok())
        .unwrap_or(0)
        .max(found.notes.len());
    let (distinct, collapsed) = collapse(found.notes, |r| &r.note.title);
    let total = in_scope.saturating_sub(collapsed);
    let global = |r: &Recalled| r.note.scope == Scope::Global;
    let mut notes = share(distinct, NOTE_CAP, global);
    notes.sort_by_key(global);
    let lines = notes.iter().map(|r| (global(r), line(r)));
    let (text, shown) = render(&found.context, lines, total);
    Ok(Some(SessionStart {
        context: found.context,
        notes,
        shown,
        total,
        text,
    }))
}

/// `notes` without any whose title is near-identical to one before it, and
/// how many were left out.
fn collapse<T>(notes: Vec<T>, title: impl Fn(&T) -> &str) -> (Vec<T>, usize) {
    let mut kept: Vec<T> = Vec::with_capacity(notes.len());
    let mut collapsed = 0;
    for note in notes {
        if kept.iter().any(|k| near_identical(title(k), title(&note))) {
            collapsed += 1;
        } else {
            kept.push(note);
        }
    }
    (kept, collapsed)
}

/// At most `cap` of `notes`, in their order. When both global and other
/// notes are present, global ones get at least [`GLOBAL_SHARE`] of the
/// slots, if there are that many, the others the rest; slots one side cannot
/// fill go to the other.
fn share<T>(notes: Vec<T>, cap: usize, global: impl Fn(&T) -> bool) -> Vec<T> {
    let globals = notes.iter().filter(|n| global(n)).count();
    let others = notes.len() - globals;
    let global_slots = globals.min(GLOBAL_SHARE.min(cap).max(cap - others.min(cap)));
    let other_slots = others.min(cap - global_slots);
    let (mut global_left, mut other_left) = (global_slots, other_slots);
    notes
        .into_iter()
        .filter(|n| {
            let left = if global(n) {
                &mut global_left
            } else {
                &mut other_left
            };
            let take = *left > 0;
            *left = left.saturating_sub(1);
            take
        })
        .collect()
}

/// One note as an agent reads it: its freshness label, its title, and its id.
#[must_use]
pub fn line(recalled: &Recalled) -> String {
    format!(
        "- [{}] {} ({})",
        recalled.freshness.label(),
        title(&recalled.note.title),
        recalled.note.id.as_str()
    )
}

/// A note title fit for an agent's context: secrets masked, on one line.
pub fn title(text: &str) -> String {
    inbox::redact(text)
        .unwrap_or_else(|_| "(title withheld)".to_owned())
        .replace(char::is_control, " ")
}

/// One line per note while the section stays under [`CHAR_CAP`], each kind
/// under its heading, then a line saying how many of `total` were left out
/// and where they are. `lines` pairs each line with whether its note is
/// global. Returns the section and how many note lines it holds.
fn render(
    context: &RepoContext,
    lines: impl IntoIterator<Item = (bool, String)>,
    total: usize,
) -> (String, usize) {
    let mut out = String::new();
    let mut heading = "";
    let mut shown = 0;
    for (global, line) in lines {
        let wanted = match (context, global) {
            (RepoContext::NoRepo, _) => "Lore memory:\n",
            (_, false) => "Lore memory for this repo:\n",
            (_, true) => "Lore memory for all repos:\n",
        };
        let new = if wanted == heading { "" } else { wanted };
        if out.len() + new.len() + line.len() + 1 > CHAR_CAP {
            break;
        }
        out.push_str(new);
        heading = wanted;
        out.push_str(&line);
        out.push('\n');
        shown += 1;
    }
    if out.is_empty() {
        out.push_str("Lore memory:\n");
    }
    let left = total.saturating_sub(shown);
    if left > 0 {
        out.push_str(&format!(
            "- {left} more not shown. The recall_memory tool lists them.\n"
        ));
    }
    (out, shown)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_notes_section_stops_near_two_thousand_bytes() {
        let repo = RepoContext::NoRemote {
            root: std::path::PathBuf::from("/repo"),
        };
        let line = format!("- [fresh] {} (01JD0000000000000000000001)", "t".repeat(400));
        let out = render(&repo, std::iter::repeat_n((false, line.clone()), 10), 10);
        let expected = format!(
            "Lore memory for this repo:\n{line}\n{line}\n{line}\n{line}\n\
             - 6 more not shown. The recall_memory tool lists them.\n"
        );
        assert_eq!(out, (expected, 4));
        assert_eq!(
            render(&repo, [(false, "- [gone] a (b)".to_owned())], 1),
            ("Lore memory for this repo:\n- [gone] a (b)\n".to_owned(), 1)
        );
        assert_eq!(
            render(
                &RepoContext::NoRepo,
                [(true, "- [gone] a (b)".to_owned())],
                1
            ),
            ("Lore memory:\n- [gone] a (b)\n".to_owned(), 1)
        );
    }

    #[test]
    fn global_notes_in_a_repo_are_headed_apart_from_the_repos_own() {
        let repo = RepoContext::NoRemote {
            root: std::path::PathBuf::from("/repo"),
        };
        let lines = [
            (false, "- [fresh] a (1)".to_owned()),
            (true, "- [fresh] b (2)".to_owned()),
        ];
        assert_eq!(
            render(&repo, lines, 2).0,
            "Lore memory for this repo:\n- [fresh] a (1)\n\
             Lore memory for all repos:\n- [fresh] b (2)\n"
        );
        assert_eq!(
            render(&repo, [(true, "- [fresh] b (2)".to_owned())], 1).0,
            "Lore memory for all repos:\n- [fresh] b (2)\n"
        );
    }

    /// `share` over `repo` repository notes then `global` global ones, as
    /// recall orders them; returns how many of each it kept.
    fn shared(repo: usize, global: usize) -> (usize, usize) {
        let notes: Vec<bool> = std::iter::repeat_n(false, repo)
            .chain(std::iter::repeat_n(true, global))
            .collect();
        let kept = share(notes, NOTE_CAP, |g| *g);
        let globals = kept.iter().filter(|g| **g).count();
        (kept.len() - globals, globals)
    }

    #[test]
    fn the_cap_is_shared_between_repository_and_global_notes() {
        assert_eq!(shared(12, 10), (6, 4));
        assert_eq!(shared(12, 2), (8, 2));
        assert_eq!(shared(3, 10), (3, 7));
        assert_eq!(shared(0, 12), (0, 10));
        assert_eq!(shared(12, 0), (10, 0));
        assert_eq!(shared(4, 3), (4, 3));
        assert_eq!(shared(0, 0), (0, 0));
    }

    #[test]
    fn a_near_identical_title_after_another_is_left_out() {
        let titles = vec![
            "List follow-up items in chat; don't file them to a team tracker",
            "Resolve Greptile threads you fix; reply then resolve the ones you decline",
            "List follow-up items in chat instead of filing them to a team tracker",
            "Resolve review threads once addressed; reply, then resolve, the ones you decline",
        ];
        let (kept, collapsed) = collapse(titles.clone(), |t| t);
        assert_eq!(kept, vec![titles[0], titles[1], titles[3]]);
        assert_eq!(collapsed, 1);
    }

    #[test]
    fn a_title_is_redacted_and_kept_on_one_line() {
        assert_eq!(title("two\nlines"), "two lines");
        let key = concat!("AKIA", "Z7Q4R2T6Y8U1W3E5");
        let shown = title(&format!("rotate {key}"));
        assert!(
            shown.starts_with("rotate ") && !shown.contains(key),
            "{shown}"
        );
    }
}
