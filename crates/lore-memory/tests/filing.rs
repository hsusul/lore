//! Filing a mined lesson: the proposal it becomes, and keeping it in one step.
#![allow(clippy::expect_used, clippy::unwrap_used)]

mod common;

use lore_memory::filing;
use lore_memory::inbox;
use lore_memory::mine::{Candidate, Lesson, Signal};
use lore_memory::note::{NoteType, Scope};
use lore_memory::vault::Vault;

fn lesson() -> Lesson {
    let receipt = |session: &str, at_ms: i64, text: &str| Candidate {
        session: session.to_owned(),
        agent: "claude-code".to_owned(),
        repo: None,
        repo_path: None,
        at_ms: Some(at_ms),
        text: text.to_owned(),
        preceding: Some("Ran npm install.".to_owned()),
        following: None,
        signal: Signal::Correction,
    };
    Lesson {
        id: "0123456789".to_owned(),
        text: "use pnpm, not npm".to_owned(),
        conversations: 2,
        score: 4,
        latest_ms: Some(1_793_000_000_000),
        receipts: vec![
            receipt("sess-b", 1_793_000_000_000, "use pnpm, not npm"),
            receipt("sess-a", 1_792_000_000_000, "no, pnpm"),
        ],
        conflicts: Vec::new(),
    }
}

#[test]
fn a_lesson_becomes_a_verified_convention_from_its_newest_receipt() {
    let proposal = filing::proposal(&lesson()).unwrap();
    assert_eq!(proposal.title, "use pnpm, not npm");
    assert_eq!(proposal.agent.as_str(), "claude-code");
    assert_eq!(proposal.session.as_str(), "sess-b");
    assert!(proposal.source_verified);
    assert_eq!(proposal.note_type, NoteType::Convention);
    assert_eq!(proposal.scope, Scope::Global);
    assert_eq!(
        proposal.body,
        "use pnpm, not npm\n\n\
         Mined by `lorectl mem mine` (lesson 0123456789) from corrections in 2 conversations:\n\
         - 2026-10-26 claude-code sess-b: \"use pnpm, not npm\", after \"Ran npm install.\"\n\
         - 2026-10-14 claude-code sess-a: \"no, pnpm\", after \"Ran npm install.\"\n"
    );
}

#[test]
fn keeping_a_lesson_files_and_accepts_it_once() {
    std::env::set_var("GIT_CONFIG_GLOBAL", "/dev/null");
    std::env::set_var("GIT_CONFIG_NOSYSTEM", "1");
    let dir = tempfile::tempdir().unwrap();
    let (vault, _) = Vault::init(dir.path()).unwrap();
    let lesson = lesson();

    let rel = filing::keep(&vault, &lesson, "Use pnpm").unwrap();
    assert!(dir.path().join(&rel).is_file(), "{rel} was not written");

    let (filed, _) = inbox::proposals(&vault).unwrap();
    assert!(filing::is_filed(&filed, &lesson));
    assert_eq!(
        filed
            .iter()
            .filter(|p| filing::came_from(p, &lesson))
            .count(),
        1
    );
}

#[test]
fn a_lesson_from_one_repository_is_anchored_to_the_files_its_receipts_name() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("proj");
    common::repo(&root, Some("https://github.com/example/proj.git"));
    common::commit(&root, "src/gen.rs", "pub fn generated() {}\n", "init");
    let head = common::git(&root, &["rev-parse", "--short=12", "HEAD"]);
    let mut lesson = lesson();
    for receipt in &mut lesson.receipts {
        receipt.repo_path = Some(root.to_str().unwrap().to_owned());
        receipt.text = "stop editing the generated files".to_owned();
    }
    lesson.receipts[0].preceding = Some(format!(
        "I'll edit `src/gen.rs:1`, {}/src/gen.rs and docs/missing.md.",
        root.display()
    ));

    let proposal = filing::proposal(&lesson).unwrap();
    assert!(matches!(proposal.scope, Scope::Repo(_)));
    let anchors: Vec<_> = (proposal.anchors.iter())
        .map(|a| {
            (
                a.path.as_str(),
                a.lines,
                a.commit.as_ref().map(|c| c.as_str()),
            )
        })
        .collect();
    assert_eq!(anchors, [("src/gen.rs", None, Some(head.as_str()))]);
}
