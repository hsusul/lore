//! Policies for accepting a proposal without a person, scored by
//! [`crate::eval`]. None of them is used to accept anything yet.

use std::collections::{BTreeSet, HashSet};

use crate::eval::{Policy, Verdict};
use crate::inbox::Proposal;
use crate::note::Scope;

/// Two proposals agree when at least this share of their words, per mille,
/// is common to both (Jaccard).
const AGREEMENT_PER_MILLE: usize = 500;

/// The starting bar. A proposal is accepted only when all three hold:
///
/// 1. Its session is one the archive holds for its agent, so its source can
///    be read back.
/// 2. It is scoped to a repository and has an anchor pinned with a line hash,
///    so recall can tell when the code it is about changes.
/// 3. An earlier proposal from another verified session agrees with it: the
///    same type and scope, and at least half their words in common. One
///    session, however sure, is not enough.
#[derive(Debug, Clone, Default)]
pub struct StartingBar {
    verified: HashSet<String>,
}

impl StartingBar {
    /// `verified` holds the session ids the archive resolved for the agent
    /// that proposed under them.
    pub fn new(verified: impl IntoIterator<Item = String>) -> Self {
        Self {
            verified: verified.into_iter().collect(),
        }
    }

    fn is_verified(&self, proposal: &Proposal) -> bool {
        self.verified.contains(proposal.session.as_str())
    }
}

impl Policy for StartingBar {
    fn name(&self) -> &'static str {
        "starting-bar"
    }

    fn decide(&self, proposal: &Proposal, earlier: &[&Proposal]) -> Verdict {
        let pinned = matches!(proposal.scope, Scope::Repo(_))
            && proposal.anchors.iter().any(|a| a.hash.is_some());
        if !self.is_verified(proposal) || !pinned {
            return Verdict::Hold;
        }
        let mine = words(proposal);
        let agreed = earlier.iter().any(|other| {
            other.session != proposal.session
                && self.is_verified(other)
                && other.note_type == proposal.note_type
                && other.scope == proposal.scope
                && overlap_per_mille(&mine, &words(other)) >= AGREEMENT_PER_MILLE
        });
        if agreed {
            Verdict::Accept
        } else {
            Verdict::Hold
        }
    }
}

/// The lowercase words of a proposal's title and body.
fn words(proposal: &Proposal) -> BTreeSet<String> {
    [proposal.title.as_str(), proposal.body.as_str()]
        .iter()
        .flat_map(|text| text.split(|c: char| !c.is_alphanumeric()))
        .filter(|word| !word.is_empty())
        .map(str::to_lowercase)
        .collect()
}

/// Shared words over all words, per mille. 0 when both are empty.
pub(crate) fn overlap_per_mille(a: &BTreeSet<String>, b: &BTreeSet<String>) -> usize {
    let intersection = a.intersection(b).count();
    let union = a.len() + b.len() - intersection;
    (intersection * 1000).checked_div(union).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::eval::{score, Case, Label};
    use crate::note::{AgentKind, Anchor, LineHash, NoteId, NoteType, RelPath, RepoKey, SessionId};
    use time::OffsetDateTime;

    fn proposal(n: u8, session: &str, title: &str) -> Proposal {
        Proposal {
            id: NoteId::parse(&format!("01JD00000000000000000000{n:02}")).unwrap(),
            agent: AgentKind::ClaudeCode,
            session: SessionId::parse(session).unwrap(),
            source_verified: false,
            note_type: NoteType::Pitfall,
            scope: Scope::Repo(RepoKey::parse("81444c90f1ff44ac").unwrap()),
            title: title.to_owned(),
            body: "Run the migration twice and it fails.\n".to_owned(),
            anchors: vec![Anchor {
                path: RelPath::parse("src/db.rs").unwrap(),
                lines: None,
                hash: Some(LineHash::parse("b3:0123456789abcdef").unwrap()),
                commit: None,
            }],
            relation: None,
            enforce: Vec::new(),
            proposed_at: OffsetDateTime::from_unix_timestamp(1_800_000_000 + i64::from(n)).unwrap(),
        }
    }

    fn bar() -> StartingBar {
        StartingBar::new(["s_a".to_owned(), "s_b".to_owned()])
    }

    fn verdict(p: &Proposal, earlier: &[&Proposal]) -> Verdict {
        bar().decide(p, earlier)
    }

    #[test]
    fn a_second_verified_session_saying_the_same_thing_is_accepted() {
        let first = proposal(1, "s_a", "Migrations are not idempotent");
        let second = proposal(2, "s_b", "Migrations are not idempotent here");
        assert_eq!(verdict(&first, &[]), Verdict::Hold);
        assert_eq!(verdict(&second, &[&first]), Verdict::Accept);
    }

    #[test]
    fn each_missing_condition_holds_the_proposal() {
        let first = proposal(1, "s_a", "Migrations are not idempotent");

        let same_session = proposal(2, "s_a", "Migrations are not idempotent");
        assert_eq!(verdict(&same_session, &[&first]), Verdict::Hold);

        let unverified = proposal(2, "s_c", "Migrations are not idempotent");
        assert_eq!(verdict(&unverified, &[&first]), Verdict::Hold);
        let after_unverified = proposal(3, "s_b", "Migrations are not idempotent");
        assert_eq!(verdict(&after_unverified, &[&unverified]), Verdict::Hold);

        let mut unpinned = proposal(2, "s_b", "Migrations are not idempotent");
        unpinned.anchors[0].hash = None;
        assert_eq!(verdict(&unpinned, &[&first]), Verdict::Hold);

        let mut global = proposal(2, "s_b", "Migrations are not idempotent");
        global.scope = Scope::Global;
        assert_eq!(verdict(&global, &[&first]), Verdict::Hold);

        let mut other_type = proposal(2, "s_b", "Migrations are not idempotent");
        other_type.note_type = NoteType::Fact;
        assert_eq!(verdict(&other_type, &[&first]), Verdict::Hold);

        let mut different = proposal(2, "s_b", "Cache keys include the locale");
        different.body = "The CDN varies on Accept-Language.\n".to_owned();
        assert_eq!(verdict(&different, &[&first]), Verdict::Hold);
    }

    #[test]
    fn overlap_is_shared_words_over_all_words() {
        let set = |s: &str| s.split(' ').map(str::to_owned).collect::<BTreeSet<_>>();
        assert_eq!(overlap_per_mille(&set("a b c"), &set("a b d")), 500);
        assert_eq!(overlap_per_mille(&set("a"), &set("b")), 0);
        assert_eq!(overlap_per_mille(&BTreeSet::new(), &BTreeSet::new()), 0);
    }

    #[test]
    fn scored_on_decisions_it_accepts_only_the_corroborated_proposal() {
        let cases = [
            Case {
                proposal: proposal(1, "s_a", "Migrations are not idempotent"),
                label: Label::Accept,
            },
            Case {
                proposal: proposal(2, "s_b", "Migrations are not idempotent"),
                label: Label::Accept,
            },
            Case {
                proposal: proposal(3, "s_c", "Migrations are not idempotent"),
                label: Label::Reject,
            },
        ];
        let report = score(&bar(), &cases);
        assert_eq!(report.auto_accepted, 1);
        assert_eq!(report.agreed, 1);
        assert_eq!(report.false_accepts, Vec::new());
    }
}
