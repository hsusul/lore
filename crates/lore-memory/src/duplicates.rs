//! Notes that say the same thing twice: two active notes whose titles are
//! near-identical, so an agent can be shown one of them and a person told to
//! retire the other. Nothing here edits or retires a note.
//!
//! Two titles are near-identical when their content words, as
//! [`related_words`] picks them and with a plural or verb ending folded
//! ("filing" and "file" are one word), overlap by at least
//! [`NEAR_IDENTICAL`] (Jaccard: shared words over all words). The threshold is
//! strict on purpose: "Resolve Greptile threads you fix; reply then resolve
//! the ones you decline" and "Resolve review threads once addressed; reply,
//! then resolve, the ones you decline" share half their words and are two
//! lessons, while "List follow-up items in chat; don't file them to a team
//! tracker" and "List follow-up items in chat instead of filing them to a
//! team tracker" share seven of eight.

use std::collections::BTreeSet;

use crate::index::related_words;
use crate::note::{Note, Scope, Status};

/// The share of content words two titles must share to be one note twice.
pub const NEAR_IDENTICAL: f64 = 0.7;

/// The content words of a title with common English endings folded, so the
/// forms of one word compare equal.
fn stems(title: &str) -> BTreeSet<String> {
    related_words(title)
        .into_iter()
        .map(|word| {
            for ending in ["ing", "ed", "es", "s", "e"] {
                if let Some(stem) = word.strip_suffix(ending) {
                    if stem.chars().count() >= 3 {
                        return stem.to_owned();
                    }
                }
            }
            word
        })
        .collect()
}

/// Whether two titles are near-identical; see the module docs. A title with
/// no content words is near-identical to nothing.
#[must_use]
pub fn near_identical(a: &str, b: &str) -> bool {
    let (a, b) = (stems(a), stems(b));
    if a.is_empty() || b.is_empty() {
        return false;
    }
    let shared = a.intersection(&b).count();
    let all = a.union(&b).count();
    #[allow(clippy::cast_precision_loss)]
    let share = shared as f64 / all as f64;
    share >= NEAR_IDENTICAL
}

/// Two active notes with near-identical titles that can be in scope together.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NearDuplicate {
    /// The two note files, sorted.
    pub paths: [String; 2],
    /// Their titles, in the order of `paths`.
    pub titles: [String; 2],
}

/// Every pair of active notes in `notes`, given with their vault paths, whose
/// titles are near-identical and which one directory can hold together: the
/// same scope, or one of them global. Sorted by path.
pub fn near_duplicates<'a>(
    notes: impl IntoIterator<Item = (&'a str, &'a Note)>,
) -> Vec<NearDuplicate> {
    let mut active: Vec<(&str, &Note)> = notes
        .into_iter()
        .filter(|(_, note)| note.status == Status::Active)
        .collect();
    active.sort_by(|a, b| a.0.cmp(b.0));
    let mut pairs = Vec::new();
    for (i, (path_a, a)) in active.iter().enumerate() {
        for (path_b, b) in &active[i + 1..] {
            let together =
                a.scope == b.scope || a.scope == Scope::Global || b.scope == Scope::Global;
            if together && near_identical(&a.title, &b.title) {
                pairs.push(NearDuplicate {
                    paths: [(*path_a).to_owned(), (*path_b).to_owned()],
                    titles: [a.title.clone(), b.title.clone()],
                });
            }
        }
    }
    pairs
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_same_lesson_reworded_is_near_identical() {
        assert!(near_identical(
            "List follow-up items in chat; don't file them to a team tracker",
            "List follow-up items in chat instead of filing them to a team tracker",
        ));
        assert!(near_identical("Run the tests", "Run tests"));
    }

    #[test]
    fn related_but_distinct_lessons_are_not() {
        assert!(!near_identical(
            "Resolve Greptile threads you fix; reply then resolve the ones you decline",
            "Resolve review threads once addressed; reply, then resolve, the ones you decline",
        ));
        assert!(!near_identical(
            "Always ask users for consent before collecting analytics data such as PostHog",
            "Ask users before collecting PostHog data; otherwise it is an App Store violation",
        ));
        assert!(!near_identical("Note 01", "Note 02"));
        assert!(!near_identical("the", "the"));
    }
}
