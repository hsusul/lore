//! The agent write path: proposals land one line per session file, secrets
//! never reach the inbox, and accept, reject, and retire converge.
#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::fs;
use std::path::Path;
use std::process::Command;

use lore_memory::inbox::{self, Proposal};
use lore_memory::note::{
    AgentKind, Anchor, LineRange, NoteDate, NoteId, NoteType, RelPath, Relation, Scope, SessionId,
};
use lore_memory::review::{self, ReviewError};
use lore_memory::vault::{Added, Vault, VaultError};
use time::OffsetDateTime;

const P1: &str = "01JC9V6Q4M8K2T7X3N5R0B1H6D";
const P2: &str = "01JC9V6Q4M8K2T7X3N5R0B1H6E";

fn isolate_git() {
    std::env::set_var("GIT_CONFIG_GLOBAL", "/dev/null");
    std::env::set_var("GIT_CONFIG_NOSYSTEM", "1");
}

fn git(root: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn subjects(root: &Path) -> Vec<String> {
    git(root, &["log", "--format=%s"])
        .lines()
        .map(str::to_string)
        .collect()
}

fn new_vault() -> (tempfile::TempDir, Vault) {
    isolate_git();
    let dir = tempfile::tempdir().unwrap();
    let (vault, _) = Vault::init(dir.path()).unwrap();
    (dir, vault)
}

fn proposal(id: &str, agent: AgentKind, session: &str, body: &str) -> Proposal {
    Proposal {
        id: NoteId::parse(id).unwrap(),
        agent,
        session: SessionId::parse(session).unwrap(),
        note_type: NoteType::Pitfall,
        scope: Scope::Global,
        title: "Worktrees share refs".into(),
        body: body.into(),
        anchors: Vec::new(),
        relation: None,
        proposed_at: OffsetDateTime::from_unix_timestamp(1_793_534_400).unwrap(),
    }
}

const BODY: &str = "A fetch in one worktree is seen by all of them.\n";

fn today() -> String {
    NoteDate::today().to_string()
}

#[test]
fn two_sessions_proposing_at_once_write_two_files_of_one_line() {
    let (dir, vault) = new_vault();
    let root = dir.path().to_path_buf();
    let threads: Vec<_> = [
        (P1, AgentKind::ClaudeCode, "s1"),
        (P2, AgentKind::Codex, "s2"),
    ]
    .into_iter()
    .map(|(id, agent, session)| {
        let root = root.clone();
        std::thread::spawn(move || {
            let vault = Vault::open(&root).unwrap();
            inbox::propose(&vault, proposal(id, agent, session, BODY)).unwrap();
        })
    })
    .collect();
    for thread in threads {
        thread.join().unwrap();
    }
    let mut files: Vec<String> = fs::read_dir(root.join("_inbox"))
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    files.sort();
    assert_eq!(files, ["claude-code-s1.jsonl", "codex-s2.jsonl"]);
    assert_eq!(
        fs::read_to_string(root.join("_inbox/codex-s2.jsonl")).unwrap(),
        "{\"agent\":\"codex\",\"anchors\":[],\"body\":\"A fetch in one worktree is seen by all of them.\\n\",\"id\":\"01JC9V6Q4M8K2T7X3N5R0B1H6E\",\"proposed_at\":\"2026-11-01T12:00:00Z\",\"scope\":\"global\",\"session\":\"s2\",\"title\":\"Worktrees share refs\",\"type\":\"pitfall\"}\n"
    );
    assert_eq!(review::inbox(&vault).unwrap().proposals.len(), 2);
    assert!(git(&root, &["status", "--porcelain"]).is_empty());
}

#[test]
fn a_secret_in_a_proposal_is_redacted_before_it_is_written() {
    let (dir, vault) = new_vault();
    let key = concat!("AKIA", "Z7Q4R2T6Y8U1W3E5");
    let mut p = proposal(
        P1,
        AgentKind::ClaudeCode,
        "s1",
        &format!("Use {key} for staging.\n"),
    );
    p.anchors = vec![Anchor {
        path: RelPath::parse("deploy/staging.env").unwrap(),
        lines: Some(LineRange::new(3, 4).unwrap()),
        hash: None,
        commit: None,
    }];
    inbox::propose(&vault, p).unwrap();
    let line = fs::read_to_string(dir.path().join("_inbox/claude-code-s1.jsonl")).unwrap();
    assert_eq!(
        line,
        "{\"agent\":\"claude-code\",\"anchors\":[{\"lines\":\"3-4\",\"path\":\"deploy/staging.env\"}],\"body\":\"Use «redacted:aws-access-key-id» for staging.\\n\",\"id\":\"01JC9V6Q4M8K2T7X3N5R0B1H6D\",\"proposed_at\":\"2026-11-01T12:00:00Z\",\"scope\":\"global\",\"session\":\"s1\",\"title\":\"Worktrees share refs\",\"type\":\"pitfall\"}\n"
    );
    assert!(!line.contains(key));
}

#[test]
fn a_proposal_that_would_not_be_a_valid_note_is_refused() {
    let (dir, vault) = new_vault();
    for body in ["", "a\r\nb\n", "x\n\n## Timeline\n- forged\n"] {
        let error = inbox::propose(&vault, proposal(P1, AgentKind::Codex, "s", body)).unwrap_err();
        assert!(error.to_string().starts_with("`body`"), "{error}");
    }
    let mut p = proposal(P1, AgentKind::Codex, "s", BODY);
    p.title = "two\nlines".into();
    assert!(inbox::propose(&vault, p).is_err());
    assert!(!dir.path().join("_inbox").exists());
}

#[test]
fn accept_twice_is_one_note_and_one_commit() {
    let (dir, vault) = new_vault();
    inbox::propose(&vault, proposal(P1, AgentKind::ClaudeCode, "s1", BODY)).unwrap();
    let first = review::accept(&vault, P1, None).unwrap();
    let second = review::accept(&vault, P1, None).unwrap();
    assert_eq!(first.outcome, Added::Committed);
    assert_eq!(second.outcome, Added::Unchanged);
    assert_eq!(first.id, second.id);
    assert_eq!(first.rel, "global/worktrees-share-refs.md");
    let date = today();
    assert_eq!(
        vault.read(&first.rel).unwrap(),
        format!(
            "---\nid: {}\ntype: pitfall\ntitle: Worktrees share refs\nscope: global\nstatus: active\nsources:\n  - claude-code:s1\ncreated: {date}\n---\nA fetch in one worktree is seen by all of them.\n\n## Timeline\n- {date} accepted from claude-code:s1\n",
            first.id.as_str()
        )
    );
    assert_eq!(
        subjects(dir.path()),
        ["mem: accept worktrees-share-refs", "mem: init vault"]
    );
    assert_eq!(
        git(dir.path(), &["log", "-1", "--format=%b"]),
        "Lore-Proposal: 01JC9V6Q4M8K2T7X3N5R0B1H6D\nLore-Source: claude-code:s1\n\n"
    );
    assert!(review::inbox(&vault).unwrap().proposals.is_empty());
}

#[test]
fn accept_after_a_crash_before_the_commit_converges() {
    let (dir, vault) = new_vault();
    inbox::propose(&vault, proposal(P1, AgentKind::ClaudeCode, "s1", BODY)).unwrap();
    let first = review::accept(&vault, P1, None).unwrap();
    let bytes = vault.read(&first.rel).unwrap();
    git(dir.path(), &["reset", "-q", "--soft", "HEAD~1"]);
    git(dir.path(), &["reset", "-q", "--", "global"]);
    fs::remove_file(dir.path().join(".lore/decisions.jsonl")).unwrap();

    let again = review::accept(&vault, P1, None).unwrap();
    assert_eq!((again.outcome, &again.id), (Added::Committed, &first.id));
    assert_eq!(vault.read(&first.rel).unwrap(), bytes);
    fs::remove_file(dir.path().join(".lore/decisions.jsonl")).unwrap();
    let third = review::accept(&vault, P1, None).unwrap();
    assert_eq!(third.outcome, Added::Unchanged);
    assert_eq!(
        subjects(dir.path()),
        ["mem: accept worktrees-share-refs", "mem: init vault"]
    );
}

#[test]
fn accept_commits_only_its_own_path_and_refuses_a_hand_edited_target() {
    let (dir, vault) = new_vault();
    fs::write(dir.path().join("scratch.md"), "my notes\n").unwrap();
    git(dir.path(), &["add", "scratch.md"]);
    inbox::propose(&vault, proposal(P1, AgentKind::ClaudeCode, "s1", BODY)).unwrap();
    let accepted = review::accept(&vault, P1, None).unwrap();
    assert_eq!(
        git(dir.path(), &["show", "--name-only", "--format="]),
        "global/worktrees-share-refs.md\n"
    );
    assert_eq!(
        git(dir.path(), &["status", "--porcelain"]),
        "A  scratch.md\n"
    );

    let path = dir.path().join(&accepted.rel);
    let edited = vault
        .read(&accepted.rel)
        .unwrap()
        .replace("all of them", "all");
    fs::write(&path, &edited).unwrap();
    let mut update = proposal(P2, AgentKind::Codex, "s2", "Fetch once.\n");
    update.relation = Some(Relation::Updates(accepted.id.clone()));
    inbox::propose(&vault, update).unwrap();
    let error = review::accept(&vault, P2, None).unwrap_err();
    assert!(
        matches!(error, ReviewError::Dirty(ref rel) if rel == "global/worktrees-share-refs.md")
    );
    assert_eq!(fs::read_to_string(&path).unwrap(), edited);
    assert_eq!(subjects(dir.path()).len(), 2);
}

#[test]
fn an_updates_proposal_rewrites_its_target() {
    let (dir, vault) = new_vault();
    let (target, _) = vault
        .add("---\ntitle: Worktrees share refs\nmood: calm\ntype: pitfall\nid: 01JC8Z0000000000000000000A\ncreated: 2026-10-01\n---\nA fetch in one worktree is seen by all of them.\n\n## Timeline\n- 2026-10-01 written by hand\n")
        .unwrap();
    let mut update = proposal(
        P2,
        AgentKind::Codex,
        "s2",
        "Fetch once; every worktree sees it.\n",
    );
    update.title = "ignored on update".into();
    update.relation = Some(Relation::Updates(target.id.clone()));
    update.anchors = vec![Anchor {
        path: RelPath::parse("crates/lore-core/src/git.rs").unwrap(),
        lines: Some(LineRange::new(210, 224).unwrap()),
        hash: None,
        commit: None,
    }];
    inbox::propose(&vault, update).unwrap();
    let first = review::accept(&vault, P2, None).unwrap();
    let second = review::accept(&vault, P2, None).unwrap();
    assert!(first.updated);
    assert_eq!(
        (first.outcome, second.outcome),
        (Added::Committed, Added::Unchanged)
    );
    let date = today();
    assert_eq!(
        vault.read("global/worktrees-share-refs.md").unwrap(),
        format!("---\ntitle: Worktrees share refs\naliases:\n  - Worktrees share refs\nmood: calm\ntype: pitfall\nid: 01JC8Z0000000000000000000A\nscope: global\nstatus: active\nanchors:\n  - path: crates/lore-core/src/git.rs\n    lines: 210-224\nsources:\n  - codex:s2\ncreated: 2026-10-01\n---\nFetch once; every worktree sees it.\n\n## Timeline\n- 2026-10-01 written by hand\n- {date} accepted from codex:s2\n")
    );
    assert_eq!(
        subjects(dir.path()),
        [
            "mem: update worktrees-share-refs",
            "mem: add worktrees-share-refs",
            "mem: init vault"
        ]
    );
}

#[test]
fn retire_sets_the_status_once() {
    let (dir, vault) = new_vault();
    inbox::propose(&vault, proposal(P1, AgentKind::ClaudeCode, "s1", BODY)).unwrap();
    let accepted = review::accept(&vault, P1, None).unwrap();
    let first = review::retire(&vault, "worktrees-share-refs", "fixed upstream").unwrap();
    let second = review::retire(&vault, "worktrees-share-refs", "fixed upstream").unwrap();
    assert_eq!(first, (accepted.rel.clone(), Added::Committed));
    assert_eq!(second, (accepted.rel.clone(), Added::Unchanged));
    let date = today();
    assert_eq!(
        vault.read(&accepted.rel).unwrap(),
        format!(
            "---\nid: {}\ntype: pitfall\ntitle: Worktrees share refs\nscope: global\nstatus: retired\nsources:\n  - claude-code:s1\ncreated: {date}\n---\nA fetch in one worktree is seen by all of them.\n\n## Timeline\n- {date} accepted from claude-code:s1\n- {date} retired: fixed upstream\n",
            accepted.id.as_str()
        )
    );
    assert_eq!(
        subjects(dir.path()),
        [
            "mem: retire worktrees-share-refs",
            "mem: accept worktrees-share-refs",
            "mem: init vault"
        ]
    );
}

#[test]
fn reject_leaves_the_vault_untouched() {
    let (dir, vault) = new_vault();
    inbox::propose(&vault, proposal(P1, AgentKind::ClaudeCode, "s1", BODY)).unwrap();
    let head = git(dir.path(), &["rev-parse", "HEAD"]);
    assert!(review::reject(&vault, P1, "not a pitfall").unwrap());
    assert!(!review::reject(&vault, P1, "not a pitfall").unwrap());
    assert_eq!(git(dir.path(), &["rev-parse", "HEAD"]), head);
    assert!(git(dir.path(), &["status", "--porcelain", "--ignored=no"]).is_empty());
    assert!(!dir.path().join("global").exists());
    assert!(review::inbox(&vault).unwrap().proposals.is_empty());
    let error = review::accept(&vault, P1, None).unwrap_err();
    assert!(matches!(error, ReviewError::Decided(_, "rejected")));
}

#[test]
fn accept_as_names_the_note_and_retire_finds_it_by_that_name() {
    let (dir, vault) = new_vault();
    inbox::propose(&vault, proposal(P1, AgentKind::ClaudeCode, "s1", BODY)).unwrap();
    vault.reindex().unwrap();
    assert!(matches!(
        review::accept(&vault, P1, Some("Bad Slug")),
        Err(ReviewError::BadSlug(_))
    ));
    let accepted = review::accept(&vault, P1, Some("shared-refs")).unwrap();
    assert_eq!(accepted.rel, "global/shared-refs.md");
    let (rel, outcome) = review::retire(&vault, "shared-refs", "merged").unwrap();
    assert_eq!(
        (rel.as_str(), outcome),
        ("global/shared-refs.md", Added::Committed)
    );
    assert_eq!(subjects(dir.path()).len(), 3);
}

const TARGET: &str = "---\ntitle: Worktrees share refs\ntype: pitfall\n---\nA fetch in one worktree is seen by all of them.\n";

/// How many lines of the decision log name proposal `id`.
fn decision_lines(root: &Path, id: &str) -> usize {
    fs::read_to_string(root.join(".lore/decisions.jsonl"))
        .unwrap_or_default()
        .lines()
        .filter(|line| line.contains(id))
        .count()
}

fn lose_decisions(root: &Path) {
    fs::remove_file(root.join(".lore/decisions.jsonl")).unwrap();
}

fn update_of(target: &lore_memory::note::Note) -> Proposal {
    let mut update = proposal(P2, AgentKind::Codex, "s2", "Fetch once.\n");
    update.relation = Some(Relation::Updates(target.id.clone()));
    update
}

#[test]
fn accept_refuses_to_adopt_an_orphan_whose_id_another_file_has() {
    let (dir, vault) = new_vault();
    let p = proposal(P1, AgentKind::Codex, "s1", BODY);
    inbox::propose(&vault, p.clone()).unwrap();
    let note = p.note(NoteId::parse(P2).unwrap(), NoteDate::today());
    fs::create_dir_all(dir.path().join("global")).unwrap();
    fs::write(
        dir.path().join("global/worktrees-share-refs.md"),
        note.render(),
    )
    .unwrap();
    fs::write(dir.path().join("global/duplicate.md"), note.render()).unwrap();
    let head = git(dir.path(), &["rev-parse", "HEAD"]);
    let before = git(dir.path(), &["status", "--porcelain"]);
    let error = review::accept(&vault, P1, None).unwrap_err();
    assert!(
        matches!(error, ReviewError::Vault(VaultError::DuplicateId { .. })),
        "{error}"
    );
    assert_eq!(git(dir.path(), &["rev-parse", "HEAD"]), head);
    assert_eq!(git(dir.path(), &["status", "--porcelain"]), before);
    assert_eq!(
        vault.read("global/worktrees-share-refs.md").unwrap(),
        note.render()
    );
    assert_eq!(decision_lines(dir.path(), P1), 0);
}

#[test]
fn accept_update_refuses_a_target_id_two_files_carry() {
    let (dir, vault) = new_vault();
    let (target, _) = vault.add(TARGET).unwrap();
    let text = vault.read(&target.rel_path()).unwrap();
    fs::write(dir.path().join("global/copy.md"), &text).unwrap();
    inbox::propose(&vault, update_of(&target)).unwrap();
    let head = git(dir.path(), &["rev-parse", "HEAD"]);
    let error = review::accept(&vault, P2, None).unwrap_err();
    assert!(
        matches!(error, ReviewError::Vault(VaultError::Ambiguous(..))),
        "{error}"
    );
    assert_eq!(git(dir.path(), &["rev-parse", "HEAD"]), head);
    assert_eq!(vault.read(&target.rel_path()).unwrap(), text);
    assert_eq!(decision_lines(dir.path(), P2), 0);
}

#[test]
fn retire_by_path_refuses_a_duplicate_id_added_by_hand() {
    let (dir, vault) = new_vault();
    let (note, _) = vault
        .add("---\ntitle: Original\ntype: fact\n---\nKeep this.\n")
        .unwrap();
    let rel = note.rel_path();
    let text = vault.read(&rel).unwrap();
    fs::write(dir.path().join("global/duplicate.md"), &text).unwrap();
    let head = git(dir.path(), &["rev-parse", "HEAD"]);
    let error = review::retire(&vault, &rel, "superseded").unwrap_err();
    assert!(
        matches!(error, ReviewError::Vault(VaultError::DuplicateId { .. })),
        "{error}"
    );
    assert_eq!(git(dir.path(), &["rev-parse", "HEAD"]), head);
    assert_eq!(vault.read(&rel).unwrap(), text);
}

#[test]
fn accepted_and_retired_notes_reach_search_without_a_reindex() {
    let (_dir, vault) = new_vault();
    vault
        .add("---\ntitle: Other\ntype: fact\n---\nUnrelated.\n")
        .unwrap();
    inbox::propose(&vault, proposal(P1, AgentKind::ClaudeCode, "s1", BODY)).unwrap();
    let accepted = review::accept(&vault, P1, None).unwrap();
    let hits = vault.search("fetch", 10).unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(
        (hits[0].path.as_str(), hits[0].status.as_str()),
        (accepted.rel.as_str(), "active")
    );
    review::retire(&vault, &accepted.rel, "fixed upstream").unwrap();
    let hits = vault.search("fetch", 10).unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].status, "retired");
}

#[test]
fn accept_update_finds_a_target_renamed_by_hand_and_refreshes_search() {
    let (dir, vault) = new_vault();
    let (note, _) = vault
        .add("---\ntitle: Original\ntype: fact\n---\nOld statement.\n")
        .unwrap();
    git(dir.path(), &["mv", &note.rel_path(), "global/renamed.md"]);
    git(
        dir.path(),
        &[
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.test",
            "commit",
            "-qm",
            "rename",
        ],
    );
    let mut update = proposal(P1, AgentKind::Codex, "s1", "Replacement statement.\n");
    update.relation = Some(Relation::Updates(note.id));
    inbox::propose(&vault, update).unwrap();
    let accepted = review::accept(&vault, P1, None).unwrap();
    assert_eq!(accepted.rel, "global/renamed.md");
    let hits = vault.search("Replacement", 10).unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].slug, "renamed");
    assert!(vault.search("Old", 10).unwrap().is_empty());
}

#[test]
fn accept_as_converges_when_its_decision_line_is_lost() {
    let (dir, vault) = new_vault();
    inbox::propose(&vault, proposal(P1, AgentKind::ClaudeCode, "s1", BODY)).unwrap();
    let first = review::accept(&vault, P1, Some("shared-refs")).unwrap();
    let bytes = vault.read(&first.rel).unwrap();
    lose_decisions(dir.path());
    let again = review::accept(&vault, P1, Some("shared-refs")).unwrap();
    assert_eq!(
        (again.outcome, &again.id, again.rel.as_str()),
        (Added::Unchanged, &first.id, "global/shared-refs.md")
    );
    assert_eq!(vault.read(&first.rel).unwrap(), bytes);
    assert_eq!(subjects(dir.path()).len(), 2);
    assert_eq!(decision_lines(dir.path(), P1), 1);
}

#[test]
fn an_update_converges_when_its_decision_line_is_lost() {
    let (dir, vault) = new_vault();
    let (target, _) = vault.add(TARGET).unwrap();
    inbox::propose(&vault, update_of(&target)).unwrap();
    let first = review::accept(&vault, P2, None).unwrap();
    let bytes = vault.read(&first.rel).unwrap();
    lose_decisions(dir.path());
    let again = review::accept(&vault, P2, None).unwrap();
    assert_eq!((again.outcome, again.updated), (Added::Unchanged, true));
    assert_eq!(vault.read(&first.rel).unwrap(), bytes);
    assert_eq!(subjects(dir.path()).len(), 3);
    assert_eq!(decision_lines(dir.path(), P2), 1);
}

#[test]
fn an_update_is_not_applied_again_after_a_lost_decision_and_a_retire() {
    let (dir, vault) = new_vault();
    let (target, _) = vault.add(TARGET).unwrap();
    inbox::propose(&vault, update_of(&target)).unwrap();
    let first = review::accept(&vault, P2, None).unwrap();
    lose_decisions(dir.path());
    review::retire(&vault, &first.rel, "superseded").unwrap();
    let retired = vault.read(&first.rel).unwrap();
    let again = review::accept(&vault, P2, None).unwrap();
    assert_eq!((again.outcome, again.updated), (Added::Unchanged, true));
    assert_eq!(vault.read(&first.rel).unwrap(), retired);
    assert_eq!(
        subjects(dir.path()),
        [
            "mem: retire worktrees-share-refs",
            "mem: update worktrees-share-refs",
            "mem: add worktrees-share-refs",
            "mem: init vault"
        ]
    );
    assert_eq!(decision_lines(dir.path(), P2), 1);
}

#[test]
fn a_decision_after_a_torn_line_is_still_read() {
    let (dir, vault) = new_vault();
    inbox::propose(&vault, proposal(P1, AgentKind::ClaudeCode, "s1", BODY)).unwrap();
    let mut other = proposal(P2, AgentKind::Codex, "s2", BODY);
    other.title = "Another note".into();
    inbox::propose(&vault, other).unwrap();
    // A reject of P2 killed halfway through its write.
    fs::write(
        dir.path().join(".lore/decisions.jsonl"),
        format!("{{\"id\":\"{P2}\",\"decision\":\"rej"),
    )
    .unwrap();
    let first = review::accept(&vault, P1, None).unwrap();
    assert_eq!(first.outcome, Added::Committed);
    assert!(review::decisions(&vault)
        .unwrap()
        .contains_key(&NoteId::parse(P1).unwrap()));
    let pending: Vec<String> = review::inbox(&vault)
        .unwrap()
        .proposals
        .iter()
        .map(|p| p.id.as_str().to_string())
        .collect();
    assert_eq!(pending, [P2]);
    let again = review::accept(&vault, P1, None).unwrap();
    assert_eq!(again.outcome, Added::Unchanged);
    assert_eq!(decision_lines(dir.path(), P1), 1);
    assert_eq!(subjects(dir.path()).len(), 2);
}

#[test]
fn a_session_id_that_is_a_secret_is_refused() {
    let (dir, vault) = new_vault();
    let token = concat!("ghp_", "Z7Q4R2T6Y8U1W3E5Z7Q4R2T6Y8U1W3E5abcd");
    let error = inbox::propose(&vault, proposal(P1, AgentKind::Codex, token, BODY)).unwrap_err();
    assert!(error.to_string().starts_with("`session`"), "{error}");
    assert!(!dir.path().join("_inbox").exists());

    // The same line written by hand is refused by accept.
    let line = proposal(P1, AgentKind::Codex, token, BODY).to_line();
    fs::create_dir_all(dir.path().join("_inbox")).unwrap();
    fs::write(
        dir.path().join(format!("_inbox/codex-{token}.jsonl")),
        format!("{line}\n"),
    )
    .unwrap();
    let head = git(dir.path(), &["rev-parse", "HEAD"]);
    let error = review::accept(&vault, P1, None).unwrap_err();
    assert!(matches!(error, ReviewError::Refused { .. }), "{error}");
    assert_eq!(git(dir.path(), &["rev-parse", "HEAD"]), head);

    let uuid = "0199f5c2-7a4e-7c31-9b6d-2f8e1a3c5d7b";
    inbox::propose(&vault, proposal(P2, AgentKind::Codex, uuid, BODY)).unwrap();
}
