//! `lorectl mem eval` as a person runs it: a policy scored against the
//! proposals they already accepted and rejected, with nothing written.
#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::io::Write;
use std::path::Path;
use std::process::{Command, Output, Stdio};

fn lorectl(home: &Path, args: &[&str], stdin: &str) -> Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_lorectl"))
        .arg("--archive")
        .arg(home.join("archive"))
        .args(args)
        .arg("--vault")
        .arg(home.join("vault"))
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("HOME", home)
        .env_remove("LORE_VAULT_DIR")
        .env_remove("LORE_ARCHIVE_DIR")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(stdin.as_bytes())
        .unwrap();
    child.wait_with_output().unwrap()
}

fn ok(o: &Output) -> String {
    assert_eq!(
        o.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&o.stderr)
    );
    String::from_utf8(o.stdout.clone()).unwrap()
}

/// File a proposal and return its id.
fn propose(home: &Path, session: &str, title: &str) -> String {
    let out = ok(&lorectl(
        home,
        &[
            "mem",
            "propose",
            "--agent",
            "codex",
            "--session",
            session,
            "--type",
            "fact",
            "--title",
            title,
        ],
        "Body.\n",
    ));
    out.split_whitespace().nth(1).unwrap().to_owned()
}

/// A vault with two accepted proposals, one rejected, and one pending.
fn decided_home() -> tempfile::TempDir {
    let home = tempfile::tempdir().unwrap();
    ok(&lorectl(home.path(), &["mem", "init"], ""));
    let a = propose(home.path(), "s-1", "First fact");
    let b = propose(home.path(), "s-2", "Second fact");
    let c = propose(home.path(), "s-3", "Wrong fact");
    propose(home.path(), "s-4", "Pending fact");
    ok(&lorectl(home.path(), &["mem", "accept", &a], ""));
    ok(&lorectl(home.path(), &["mem", "accept", &b], ""));
    ok(&lorectl(
        home.path(),
        &["mem", "reject", &c, "--reason", "wrong"],
        "",
    ));
    home
}

const NEVER_REPORT: &str = "\
Policy never, scored on 3 proposals you decided: 2 accepted, 1 rejected.
It would accept 0: 0 you accepted and 0 you rejected.
Precision: none accepted. Coverage: 0/1000.
Gate not passed:
- decided proposals: 3, needs at least 30
- rejected proposals: 1, needs at least 5
- auto-accepted: 0, needs at least 5
";

#[test]
fn eval_scores_the_never_policy_by_default_and_writes_nothing() {
    let home = decided_home();
    let vault = home.path().join("vault");
    let before = std::fs::read(vault.join(".lore/decisions.jsonl")).unwrap();

    assert_eq!(
        ok(&lorectl(home.path(), &["mem", "eval"], "")),
        NEVER_REPORT
    );
    assert_eq!(
        ok(&lorectl(
            home.path(),
            &["mem", "eval", "--policy", "never"],
            ""
        )),
        NEVER_REPORT
    );
    assert_eq!(
        std::fs::read(vault.join(".lore/decisions.jsonl")).unwrap(),
        before
    );
    assert!(!home.path().join("archive").exists());
}

#[test]
fn eval_without_an_archive_verifies_no_session_so_starting_bar_holds_all() {
    let home = decided_home();
    let out = ok(&lorectl(
        home.path(),
        &["mem", "eval", "--policy", "starting-bar", "--json"],
        "",
    ));
    let report: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(
        report,
        serde_json::json!({
            "policy": "starting-bar",
            "cases": 3,
            "accepts": 2,
            "rejects": 1,
            "auto_accepted": 0,
            "agreed": 0,
            "false_accepts": [],
            "precision_per_mille": null,
            "coverage_per_mille": 0,
            "passes": false,
            "shortfalls": [
                "decided proposals: 3, needs at least 30",
                "rejected proposals: 1, needs at least 5",
                "auto-accepted: 0, needs at least 5",
            ],
        })
    );
}

#[test]
fn eval_refuses_a_policy_it_does_not_know() {
    let home = decided_home();
    let out = lorectl(home.path(), &["mem", "eval", "--policy", "always"], "");
    assert_eq!(out.status.code(), Some(1));
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("never, starting-bar"),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn eval_with_archive_verifies_sessions_by_native_id() {
    let home = tempfile::tempdir().unwrap();
    ok(&lorectl(home.path(), &["mem", "init"], ""));

    let archive_dir = home.path().join("archive");
    std::fs::create_dir_all(&archive_dir).unwrap();
    let conn =
        lore_core::storage::open(&archive_dir.join(lore_core::paths::ARCHIVE_DB_FILENAME)).unwrap();
    conn.execute(
        "INSERT INTO agent (id, display_name) VALUES ('codex', 'Codex')",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO agent_session (id, agent_id, native_session_id, dedupe_key)
         VALUES ('s_arch_1', 'codex', 'rollout-2026-08-26-test', 'd1')",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO agent_session (id, agent_id, native_session_id, dedupe_key)
         VALUES ('s_arch_2', 'codex', 'rollout-2026-08-27-test', 'd2')",
        [],
    )
    .unwrap();

    let vault = lore_memory::vault::Vault::open(&home.path().join("vault")).unwrap();

    let p1 = lore_memory::inbox::Proposal {
        id: lore_memory::note::NoteId::parse("01JD0000000000000000000001").unwrap(),
        agent: lore_memory::note::AgentKind::Codex,
        session: lore_memory::note::SessionId::parse("s_arch_1").unwrap(),
        source_verified: false,
        note_type: lore_memory::note::NoteType::Fact,
        scope: lore_memory::note::Scope::Repo(
            lore_memory::note::RepoKey::parse("81444c90f1ff44ac").unwrap(),
        ),
        title: "Migrations are not idempotent".into(),
        body: "Run the migration twice and it fails.\n".into(),
        anchors: vec![lore_memory::note::Anchor {
            path: lore_memory::note::RelPath::parse("src/db.rs").unwrap(),
            lines: None,
            hash: Some(lore_memory::note::LineHash::parse("b3:0123456789abcdef").unwrap()),
            commit: None,
        }],
        relation: None,
        enforce: Vec::new(),
        proposed_at: time::OffsetDateTime::from_unix_timestamp(1_800_000_001).unwrap(),
    };
    let p2 = lore_memory::inbox::Proposal {
        id: lore_memory::note::NoteId::parse("01JD0000000000000000000002").unwrap(),
        agent: lore_memory::note::AgentKind::Codex,
        session: lore_memory::note::SessionId::parse("rollout-2026-08-27-test").unwrap(),
        source_verified: false,
        note_type: lore_memory::note::NoteType::Fact,
        scope: lore_memory::note::Scope::Repo(
            lore_memory::note::RepoKey::parse("81444c90f1ff44ac").unwrap(),
        ),
        title: "Migrations are not idempotent here".into(),
        body: "Run the migration twice and it fails.\n".into(),
        anchors: vec![lore_memory::note::Anchor {
            path: lore_memory::note::RelPath::parse("src/db.rs").unwrap(),
            lines: None,
            hash: Some(lore_memory::note::LineHash::parse("b3:0123456789abcdef").unwrap()),
            commit: None,
        }],
        relation: None,
        enforce: Vec::new(),
        proposed_at: time::OffsetDateTime::from_unix_timestamp(1_800_000_002).unwrap(),
    };

    lore_memory::inbox::propose(&vault, p1).unwrap();
    lore_memory::inbox::propose(&vault, p2).unwrap();
    ok(&lorectl(
        home.path(),
        &["mem", "accept", "01JD0000000000000000000001"],
        "",
    ));
    ok(&lorectl(
        home.path(),
        &["mem", "accept", "01JD0000000000000000000002"],
        "",
    ));

    let out = ok(&lorectl(
        home.path(),
        &["mem", "eval", "--policy", "starting-bar", "--json"],
        "",
    ));
    let report: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(report["auto_accepted"], 1);
    assert_eq!(report["agreed"], 1);
}
