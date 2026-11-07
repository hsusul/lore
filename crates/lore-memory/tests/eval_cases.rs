//! Eval cases come from the vault itself: every proposal a person accepted or
//! rejected, labeled with that decision, and nothing still pending.
#![allow(clippy::expect_used, clippy::unwrap_used)]

use lore_memory::eval::{self, Label};
use lore_memory::inbox::{self, Proposal};
use lore_memory::note::{AgentKind, NoteId, NoteType, Scope, SessionId};
use lore_memory::review;
use lore_memory::vault::Vault;
use time::OffsetDateTime;

fn proposal(id: &str, title: &str, at: i64) -> Proposal {
    Proposal {
        id: NoteId::parse(id).unwrap(),
        agent: AgentKind::Codex,
        session: SessionId::parse("s-1").unwrap(),
        source_verified: false,
        note_type: NoteType::Fact,
        scope: Scope::Global,
        title: title.into(),
        body: "Body.\n".into(),
        anchors: Vec::new(),
        relation: None,
        enforce: Vec::new(),
        proposed_at: OffsetDateTime::from_unix_timestamp(at).unwrap(),
    }
}

#[test]
fn decided_proposals_become_labeled_cases_and_pending_ones_do_not() {
    std::env::set_var("GIT_CONFIG_GLOBAL", "/dev/null");
    std::env::set_var("GIT_CONFIG_NOSYSTEM", "1");
    let dir = tempfile::tempdir().unwrap();
    let (vault, _) = Vault::init(dir.path()).unwrap();
    assert_eq!(eval::cases(&vault).unwrap(), Vec::new());

    for (id, title, at) in [
        ("01JD0000000000000000000001", "Accepted one", 1_800_000_001),
        ("01JD0000000000000000000002", "Rejected one", 1_800_000_002),
        ("01JD0000000000000000000003", "Pending one", 1_800_000_003),
    ] {
        inbox::propose(&vault, proposal(id, title, at)).unwrap();
    }
    std::fs::write(dir.path().join("_inbox/codex-s-1.jsonl.bad"), "x").unwrap();
    review::accept(&vault, "01JD0000000000000000000001", None).unwrap();
    review::reject(&vault, "01JD0000000000000000000002", "not true").unwrap();

    let cases = eval::cases(&vault).unwrap();
    let labeled: Vec<(&str, &str, Label)> = cases
        .iter()
        .map(|c| (c.proposal.id.as_str(), c.proposal.title.as_str(), c.label))
        .collect();
    assert_eq!(
        labeled,
        [
            ("01JD0000000000000000000001", "Accepted one", Label::Accept),
            ("01JD0000000000000000000002", "Rejected one", Label::Reject),
        ]
    );
    assert_eq!(
        cases[0].proposal,
        proposal("01JD0000000000000000000001", "Accepted one", 1_800_000_001)
    );
}
