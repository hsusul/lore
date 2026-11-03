//! Offline scoring of a proposal policy against a person's own decisions.
//!
//! A policy looks at one proposal and says whether it would accept it without
//! a person, or hold it for one. Scoring runs the policy over proposals a
//! person already accepted or rejected and counts where it agreed. Scoring
//! reads nothing: not the clock, the network, or the working tree, so the same
//! cases always give the same report.
//!
//! [`cases`] reads the labeled cases from a vault: the inbox's proposals and
//! the decisions a person recorded with `lorectl mem accept` and `mem reject`.
//!
//! A policy never rejects. Holding is its only alternative to accepting, so a
//! policy can only ever take work away from a person, never overrule them.

use crate::inbox::{self, Proposal};
use crate::review::{self, Decision, ReviewError};
use crate::vault::Vault;

/// What a person decided about a proposal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Label {
    Accept,
    Reject,
}

/// A proposal and the person's decision about it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Case {
    pub proposal: Proposal,
    pub label: Label,
}

/// What a policy does with a proposal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// Accept it without a person.
    Accept,
    /// Leave it in the inbox for a person.
    Hold,
}

/// A rule for accepting proposals without a person.
pub trait Policy {
    /// The name a person picks it by.
    fn name(&self) -> &'static str;
    /// The verdict on `proposal`. `earlier` holds the proposals made before
    /// it, oldest first, without their decisions, so a policy cannot learn
    /// from a decision a person had not made yet.
    fn decide(&self, proposal: &Proposal, earlier: &[&Proposal]) -> Verdict;
}

/// Holds everything. The floor any other policy is compared with.
#[derive(Debug, Clone, Copy, Default)]
pub struct Never;

impl Policy for Never {
    fn name(&self) -> &'static str {
        "never"
    }
    fn decide(&self, _: &Proposal, _: &[&Proposal]) -> Verdict {
        Verdict::Hold
    }
}

/// How a policy did against the labeled cases.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Report {
    pub cases: usize,
    /// Cases the person accepted.
    pub accepts: usize,
    /// Cases the person rejected.
    pub rejects: usize,
    /// Cases the policy accepted.
    pub auto_accepted: usize,
    /// Cases the policy and the person both accepted.
    pub agreed: usize,
    /// Proposals the policy accepted and the person rejected, oldest first.
    pub false_accepts: Vec<Proposal>,
}

impl Report {
    /// Of the policy's accepts, the share the person also accepted. `None`
    /// when the policy accepted nothing.
    #[must_use]
    pub fn precision(&self) -> Option<f64> {
        ratio(self.agreed, self.auto_accepted)
    }

    /// Of the person's accepts, the share the policy would have taken off
    /// their hands. `None` when the person accepted nothing.
    #[must_use]
    pub fn coverage(&self) -> Option<f64> {
        ratio(self.agreed, self.accepts)
    }

    /// Each way the report falls short of `gate`, in words. Empty means it
    /// passes.
    #[must_use]
    pub fn shortfalls(&self, gate: &Gate) -> Vec<String> {
        let mut out = Vec::new();
        let mut at_least = |what: &str, have: usize, need: usize| {
            if have < need {
                out.push(format!("{what}: {have}, needs at least {need}"));
            }
        };
        at_least("decided proposals", self.cases, gate.min_cases);
        at_least("rejected proposals", self.rejects, gate.min_rejects);
        at_least("auto-accepted", self.auto_accepted, gate.min_auto_accepted);
        if self.false_accepts.len() > gate.max_false_accepts {
            out.push(format!(
                "false accepts: {}, allows at most {}",
                self.false_accepts.len(),
                gate.max_false_accepts
            ));
        }
        // Per mille, so the comparison is exact.
        let precision = per_mille(self.agreed, self.auto_accepted);
        if self.auto_accepted > 0 && precision < gate.min_precision_per_mille {
            out.push(format!(
                "precision: {precision}/1000, needs at least {}/1000",
                gate.min_precision_per_mille
            ));
        }
        out
    }
}

/// The bar a policy must clear on a person's own decisions before it may
/// accept anything for them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Gate {
    pub min_cases: usize,
    pub min_rejects: usize,
    pub min_auto_accepted: usize,
    pub max_false_accepts: usize,
    pub min_precision_per_mille: usize,
}

impl Default for Gate {
    /// Enough decisions to mean something, and not one accept the person
    /// rejected.
    fn default() -> Self {
        Self {
            min_cases: 30,
            min_rejects: 5,
            min_auto_accepted: 5,
            max_false_accepts: 0,
            min_precision_per_mille: 950,
        }
    }
}

/// Score `policy` on `cases`. Cases are taken in proposal order, ties by id,
/// whatever order they come in.
#[must_use]
pub fn score(policy: &dyn Policy, cases: &[Case]) -> Report {
    let mut ordered: Vec<&Case> = cases.iter().collect();
    ordered.sort_by(|a, b| {
        (a.proposal.proposed_at, &a.proposal.id).cmp(&(b.proposal.proposed_at, &b.proposal.id))
    });
    let mut report = Report {
        cases: ordered.len(),
        ..Report::default()
    };
    let mut earlier: Vec<&Proposal> = Vec::new();
    for case in ordered {
        let verdict = policy.decide(&case.proposal, &earlier);
        match case.label {
            Label::Accept => report.accepts += 1,
            Label::Reject => report.rejects += 1,
        }
        if verdict == Verdict::Accept {
            report.auto_accepted += 1;
            match case.label {
                Label::Accept => report.agreed += 1,
                Label::Reject => report.false_accepts.push(case.proposal.clone()),
            }
        }
        earlier.push(&case.proposal);
    }
    report
}

/// Every proposal in `vault`'s inbox a person accepted or rejected, labeled
/// with that decision. Pending proposals, and inbox lines that are not valid
/// proposals, are left out. Read only.
pub fn cases(vault: &Vault) -> Result<Vec<Case>, ReviewError> {
    let (proposals, _) = inbox::proposals(vault)?;
    let decisions = review::decisions(vault)?;
    Ok(proposals
        .into_iter()
        .filter_map(|proposal| {
            let label = match decisions.get(&proposal.id)? {
                Decision::Accepted { .. } => Label::Accept,
                Decision::Rejected { .. } => Label::Reject,
            };
            Some(Case { proposal, label })
        })
        .collect())
}

fn ratio(part: usize, whole: usize) -> Option<f64> {
    (whole > 0).then(|| part as f64 / whole as f64)
}

fn per_mille(part: usize, whole: usize) -> usize {
    part.saturating_mul(1000).checked_div(whole).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::note::{AgentKind, NoteId, NoteType, Scope, SessionId};
    use time::OffsetDateTime;

    fn proposal(n: u8, title: &str) -> Proposal {
        Proposal {
            id: NoteId::parse(&format!("01JD00000000000000000000{n:02}")).unwrap(),
            agent: AgentKind::Codex,
            session: SessionId::parse("s-1").unwrap(),
            note_type: NoteType::Fact,
            scope: Scope::Global,
            title: title.to_owned(),
            body: "Body.\n".to_owned(),
            anchors: Vec::new(),
            relation: None,
            proposed_at: OffsetDateTime::from_unix_timestamp(1_800_000_000 + i64::from(n)).unwrap(),
        }
    }

    fn case(n: u8, title: &str, label: Label) -> Case {
        Case {
            proposal: proposal(n, title),
            label,
        }
    }

    /// Accepts any proposal whose title starts with `ok`, and records what it
    /// was shown as earlier.
    struct TitlePolicy(std::cell::RefCell<Vec<(String, Vec<String>)>>);

    impl Policy for TitlePolicy {
        fn name(&self) -> &'static str {
            "title"
        }
        fn decide(&self, proposal: &Proposal, earlier: &[&Proposal]) -> Verdict {
            let seen = earlier.iter().map(|p| p.title.clone()).collect();
            self.0.borrow_mut().push((proposal.title.clone(), seen));
            if proposal.title.starts_with("ok") {
                Verdict::Accept
            } else {
                Verdict::Hold
            }
        }
    }

    #[test]
    fn never_accepts_nothing_and_has_no_precision() {
        let cases = [case(1, "a", Label::Accept), case(2, "b", Label::Reject)];
        let report = score(&Never, &cases);
        assert_eq!(
            report,
            Report {
                cases: 2,
                accepts: 1,
                rejects: 1,
                ..Report::default()
            }
        );
        assert_eq!(report.precision(), None);
        assert_eq!(report.coverage(), Some(0.0));
    }

    #[test]
    fn a_policy_is_scored_in_proposal_order_and_sees_only_earlier_proposals() {
        // Given out of order on purpose.
        let cases = [
            case(3, "ok bad", Label::Reject),
            case(1, "ok good", Label::Accept),
            case(2, "held", Label::Accept),
            case(4, "ok fine", Label::Accept),
        ];
        let policy = TitlePolicy(std::cell::RefCell::default());
        let report = score(&policy, &cases);
        assert_eq!(report.cases, 4);
        assert_eq!(report.accepts, 3);
        assert_eq!(report.rejects, 1);
        assert_eq!(report.auto_accepted, 3);
        assert_eq!(report.agreed, 2);
        assert_eq!(report.false_accepts, vec![proposal(3, "ok bad")]);
        assert_eq!(report.precision(), Some(2.0 / 3.0));
        assert_eq!(report.coverage(), Some(2.0 / 3.0));
        let shown: Vec<(String, Vec<String>)> = policy.0.into_inner();
        let s = |v: &[&str]| v.iter().map(|t| (*t).to_owned()).collect::<Vec<_>>();
        assert_eq!(
            shown,
            vec![
                ("ok good".to_owned(), s(&[])),
                ("held".to_owned(), s(&["ok good"])),
                ("ok bad".to_owned(), s(&["ok good", "held"])),
                ("ok fine".to_owned(), s(&["ok good", "held", "ok bad"])),
            ]
        );
    }

    #[test]
    fn the_default_gate_names_each_shortfall() {
        let cases = [
            case(1, "ok bad", Label::Reject),
            case(2, "ok", Label::Accept),
        ];
        let policy = TitlePolicy(std::cell::RefCell::default());
        let report = score(&policy, &cases);
        assert_eq!(
            report.shortfalls(&Gate::default()),
            vec![
                "decided proposals: 2, needs at least 30",
                "rejected proposals: 1, needs at least 5",
                "auto-accepted: 2, needs at least 5",
                "false accepts: 1, allows at most 0",
                "precision: 500/1000, needs at least 950/1000",
            ]
        );
    }

    #[test]
    fn a_gate_is_passed_with_no_false_accept_and_enough_decisions() {
        let mut cases: Vec<Case> = (1..=25).map(|n| case(n, "ok", Label::Accept)).collect();
        cases.extend((26..=30).map(|n| case(n, "no", Label::Reject)));
        let policy = TitlePolicy(std::cell::RefCell::default());
        let report = score(&policy, &cases);
        assert_eq!(report.shortfalls(&Gate::default()), Vec::<String>::new());
        assert_eq!(report.precision(), Some(1.0));
    }
}
