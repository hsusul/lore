//! Filing a mined lesson: the proposal it becomes, and keeping it in one step.
#![allow(clippy::expect_used, clippy::unwrap_used)]

mod common;

use lore_memory::filing;
use lore_memory::inbox;
use lore_memory::mine::{Candidate, Lesson, Signal};
use lore_memory::note::{NoteType, RepoKey, Scope};
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
fn filing_a_lesson_across_subdirectories_or_worktrees_keeps_repo_scope() {
    let dir = tempfile::tempdir().unwrap();
    let repo_dir = dir.path().join("repo");
    common::repo(&repo_dir, Some("https://github.com/hsusul/lore"));
    common::commit(&repo_dir, "crates/core/a.txt", "a\n", "init");

    let sub_a = repo_dir.join("crates/core");
    let sub_b = repo_dir.join("crates/cli");
    std::fs::create_dir_all(&sub_b).unwrap();

    let worktree_dir = dir.path().join("wt");
    common::git(
        &repo_dir,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "feature",
            worktree_dir.to_str().unwrap(),
        ],
    );

    let mut l = lesson();
    l.receipts[0].repo_path = Some(sub_a.to_str().unwrap().to_owned());
    l.receipts[1].repo_path = Some(sub_b.to_str().unwrap().to_owned());

    let expected = Scope::Repo(RepoKey::parse("81444c90f1ff44ac").unwrap());
    let proposal = filing::proposal(&l).unwrap();
    assert_eq!(proposal.scope, expected);

    l.receipts[1].repo_path = Some(worktree_dir.to_str().unwrap().to_owned());
    let proposal_wt = filing::proposal(&l).unwrap();
    assert_eq!(proposal_wt.scope, expected);

    let other_dir = dir.path().join("other");
    common::repo(&other_dir, Some("https://github.com/hsusul/other"));
    l.receipts[1].repo_path = Some(other_dir.to_str().unwrap().to_owned());
    assert_eq!(filing::proposal(&l).unwrap().scope, Scope::Global);

    l.receipts[1].repo_path = None;
    assert_eq!(filing::proposal(&l).unwrap().scope, Scope::Global);

    l.receipts[0].repo_path = None;
    l.receipts[1].repo_path = Some(sub_a.to_str().unwrap().to_owned());
    assert_eq!(filing::proposal(&l).unwrap().scope, Scope::Global);
}
