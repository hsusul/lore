//! What an agent is given at session start: the notes that apply where it
//! runs, as the section `lorectl hook session-start` prints, and the same view
//! for the desktop app's "What agents read".

use std::path::Path;

use crate::inbox;
use crate::index::Index;
use crate::recall::{self, Recalled};
use crate::repo::RepoContext;
use crate::vault::{Vault, VaultError};

/// At most this many notes at session start. Experiment Q3 saw agents follow
/// them at 5, 10, and 25; the cap is set by what the context costs.
pub const NOTE_CAP: usize = 10;
/// The notes section stops adding note lines past this many bytes.
pub const CHAR_CAP: usize = 2_000;

/// The notes section for one working directory.
#[derive(Debug)]
pub struct SessionStart {
    pub context: RepoContext,
    /// The notes recall chose, best first. The first `shown` fit the section.
    pub notes: Vec<Recalled>,
    pub shown: usize,
    /// Every active note that applies in this directory, chosen or not.
    pub total: usize,
    /// The section exactly as an agent reads it.
    pub text: String,
}

/// The notes section for `cwd`, or `None` when no note applies there.
pub fn session_start(vault: &Vault, cwd: &Path) -> Result<Option<SessionStart>, VaultError> {
    let found = recall::recall(vault, "", cwd, NOTE_CAP)?;
    if found.notes.is_empty() {
        return Ok(None);
    }
    // Recall left the index fresh, so counting what it capped is one query.
    let scope = found.context.key().map(|k| format!("repo:{}", k.as_str()));
    let total = Index::open(&vault.lore_dir())
        .ok()
        .flatten()
        .and_then(|index| index.count_active(scope.as_deref()).ok())
        .unwrap_or(0)
        .max(found.notes.len());
    let (text, shown) = render(found.notes.iter().map(line), total);
    Ok(Some(SessionStart {
        context: found.context,
        notes: found.notes,
        shown,
        total,
        text,
    }))
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

/// A heading, one line per note while the section stays under [`CHAR_CAP`],
/// then a line saying how many of `total` were left out and where they are.
/// Returns the section and how many note lines it holds.
fn render(lines: impl IntoIterator<Item = String>, total: usize) -> (String, usize) {
    let mut out = String::from("Lore memory for this repo:\n");
    let mut shown = 0;
    for line in lines {
        if out.len() + line.len() + 1 > CHAR_CAP {
            break;
        }
        out.push_str(&line);
        out.push('\n');
        shown += 1;
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
        let line = format!("- [fresh] {} (01JD0000000000000000000001)", "t".repeat(400));
        let out = render(std::iter::repeat_n(line.clone(), 10), 10);
        let expected = format!(
            "Lore memory for this repo:\n{line}\n{line}\n{line}\n{line}\n\
             - 6 more not shown. The recall_memory tool lists them.\n"
        );
        assert_eq!(out, (expected, 4));
        assert_eq!(
            render(["- [gone] a (b)".to_owned()], 1),
            ("Lore memory for this repo:\n- [gone] a (b)\n".to_owned(), 1)
        );
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
