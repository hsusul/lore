//! The notes an agent is shown when a session starts: the accepted notes that
//! apply where it runs, each with its freshness label, bounded so they never
//! crowd the agent's context. `lorectl hook session-start` prints this, and
//! the desktop app shows the same text as "What agents will read".

use std::path::Path;

use crate::index::Index;
use crate::vault::Vault;
use crate::{inbox, recall};

/// At most this many notes at session start. Experiment Q3 saw agents follow
/// them at 5, 10, and 25; the cap is set by what the context costs.
pub const NOTE_CAP: usize = 10;
/// The notes section stops adding note lines past this many bytes.
pub const CHAR_CAP: usize = 2_000;

/// The notes section for `cwd`, or `None` when there is nothing to add. Recall
/// may refresh the vault's derived index under `.lore/`.
#[must_use]
pub fn notes(vault: &Vault, cwd: &Path) -> Option<String> {
    let found = recall::recall(vault, "", cwd, NOTE_CAP).ok()?;
    if found.notes.is_empty() {
        return None;
    }
    // Recall left the index fresh, so counting what it capped is one query.
    let scope = found.context.key().map(|k| format!("repo:{}", k.as_str()));
    let total = Index::open(&vault.lore_dir())
        .ok()
        .flatten()
        .and_then(|index| index.list_active(scope.as_deref(), usize::MAX).ok())
        .map_or(0, |all| all.len());
    let lines = found.notes.iter().map(|recalled| {
        format!(
            "- [{}] {} ({})",
            recalled.freshness.label(),
            title(&recalled.note.title),
            recalled.note.id.as_str()
        )
    });
    Some(render(lines, total.max(found.notes.len())))
}

/// A note title fit for an agent's context: secrets masked, on one line.
fn title(text: &str) -> String {
    inbox::redact(text)
        .unwrap_or_else(|_| "(title withheld)".to_owned())
        .replace(char::is_control, " ")
}

/// A heading, one line per note while the section stays under [`CHAR_CAP`],
/// then a line saying how many of `total` were left out and where they are.
fn render(lines: impl IntoIterator<Item = String>, total: usize) -> String {
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
    out
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
        assert_eq!(out, expected);
        assert_eq!(
            render(["- [gone] a (b)".to_owned()], 1),
            "Lore memory for this repo:\n- [gone] a (b)\n"
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
