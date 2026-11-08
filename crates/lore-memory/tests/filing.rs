//! Filing a mined lesson: the proposal it becomes, and keeping it in one step.
#![allow(clippy::expect_used, clippy::unwrap_used)]

mod common;

use lore_memory::anchor::Freshness;
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
    lesson.receipts[1].following = Some("Reverted ./src/gen.rs.".to_owned());

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

/// A lesson whose receipts come from `root`, saying `said` in the person's words.
fn lesson_in(root: &std::path::Path, said: &str) -> Lesson {
    let mut lesson = lesson();
    for receipt in &mut lesson.receipts {
        receipt.repo_path = Some(root.to_str().unwrap().to_owned());
        receipt.text = said.to_owned();
    }
    lesson
}

fn anchored(lesson: &Lesson) -> Vec<String> {
    let proposal = filing::proposal(lesson).unwrap();
    (proposal.anchors.iter())
        .map(|a| a.path.as_str().to_owned())
        .collect()
}

/// A repository at `root` that tracks `files` and ignores `.env`.
fn tracking(root: &std::path::Path, files: &[&str]) {
    common::repo(root, Some("https://github.com/example/proj.git"));
    common::commit(root, ".gitignore", ".env\n", "ignore");
    for file in files {
        common::commit(root, file, "x\n", file);
    }
}

#[test]
fn files_only_one_agent_turn_lists_are_not_anchored() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("proj");
    let listed = [
        "README.md",
        "package.json",
        "Cargo.toml",
        "docs/guide.md",
        "src/lib.rs",
        "src/main.rs",
        "src/gen.rs",
    ];
    tracking(&root, &listed);
    std::fs::write(root.join(".env"), "TOKEN=1\n").unwrap();
    let mut lesson = lesson_in(&root, "stop editing src/gen.rs, it is generated");
    lesson.receipts[0].preceding = Some(format!("Read {}, .env and README.MD.", listed.join(", ")));

    assert_eq!(anchored(&lesson), ["src/gen.rs"]);
}

#[test]
fn an_untracked_or_ignored_file_is_never_anchored() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("proj");
    tracking(&root, &["src/gen.rs"]);
    std::fs::write(root.join(".env"), "TOKEN=1\n").unwrap();
    std::fs::write(root.join("notes.txt"), "draft\n").unwrap();
    let mut lesson = lesson_in(&root, "never touch .env or notes.txt");
    lesson.receipts[0].following = Some("Left .env and notes.txt alone.".to_owned());

    assert!(anchored(&lesson).is_empty());
}

#[cfg(unix)]
#[test]
fn an_absolute_path_through_a_symlink_is_anchored_inside_the_repository() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("proj");
    tracking(&root, &["src/gen.rs"]);
    let link = dir.path().join("link");
    std::os::unix::fs::symlink(&root, &link).unwrap();
    let lesson = lesson_in(
        &root,
        &format!("stop editing {}/src/gen.rs", link.display()),
    );

    assert_eq!(anchored(&lesson), ["src/gen.rs"]);
}

#[test]
fn a_lesson_is_anchored_to_at_most_three_files() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("proj");
    let files = ["a.rs", "b.rs", "c.rs", "d.rs", "e.rs"];
    tracking(&root, &files);
    let lesson = lesson_in(&root, &format!("stop editing {}", files.join(" ")));

    assert_eq!(anchored(&lesson), ["a.rs", "b.rs", "c.rs"]);
}

#[test]
fn a_filed_anchor_tracks_the_file_not_its_content() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("proj");
    tracking(&root, &["src/lib.rs"]);
    let lesson = lesson_in(&root, "don't rename public functions in src/lib.rs");

    let proposal = filing::proposal(&lesson).unwrap();
    let [anchor] = proposal.anchors.as_slice() else {
        panic!("one anchor expected: {:?}", proposal.anchors);
    };
    assert_eq!(anchor.hash, None);
    std::fs::write(root.join("src/lib.rs"), "x\npub fn added() {}\n").unwrap();
    assert_eq!(anchor.check(&root), Freshness::Fresh);
}
