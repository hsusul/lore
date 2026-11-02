//! `lorectl mem propose|inbox|accept|reject|retire` as an agent and a person
//! run them: stdin bodies, printed lines, and exit codes.
#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::io::Write;
use std::path::Path;
use std::process::{Command, Output, Stdio};

fn lorectl(vault: &Path, args: &[&str], stdin: &str) -> Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_lorectl"))
        .arg("mem")
        .args(args)
        .arg("--vault")
        .arg(vault)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("HOME", vault.parent().unwrap())
        .env_remove("LORE_VAULT_DIR")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn lorectl");
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
    String::from_utf8_lossy(&o.stdout).into_owned()
}

fn propose(vault: &Path, session: &str, title: &str) -> String {
    let out = ok(&lorectl(
        vault,
        &[
            "propose",
            "--agent",
            "codex",
            "--session",
            session,
            "--type",
            "howto",
            "--title",
            title,
            "--scope",
            "repo:3f9a1c0b01234567",
            "--anchor",
            "src/main.rs:4-8",
        ],
        "Run `git worktree prune`.\n",
    ));
    let id = out.split_whitespace().nth(1).unwrap().to_string();
    assert_eq!(out, format!("proposed {id} _inbox/codex-{session}.jsonl\n"));
    id
}

#[test]
fn mem_propose_then_accept_writes_one_note() {
    let dir = tempfile::tempdir().unwrap();
    let vault = dir.path().join("vault");
    ok(&lorectl(&vault, &["init"], ""));
    let id = propose(&vault, "s1", "Prune worktrees");
    let listed = ok(&lorectl(&vault, &["inbox"], ""));
    assert_eq!(
        listed,
        format!("{id}  codex:s1  howto  repo:3f9a1c0b01234567  Prune worktrees\n")
    );
    let json = ok(&lorectl(&vault, &["inbox", "--json"], ""));
    let items: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(items[0]["anchors"][0]["lines"], "4-8");

    let accepted = ok(&lorectl(&vault, &["accept", &id], ""));
    assert!(accepted.starts_with("accepted repos/3f9a1c0b01234567/prune-worktrees.md "));
    let again = ok(&lorectl(&vault, &["accept", &id], ""));
    assert!(again.starts_with("unchanged repos/3f9a1c0b01234567/prune-worktrees.md "));
    assert_eq!(ok(&lorectl(&vault, &["inbox"], "")), "");
    let retired = ok(&lorectl(
        &vault,
        &["retire", "prune-worktrees", "--reason", "git does it now"],
        "",
    ));
    assert_eq!(
        retired,
        "retired repos/3f9a1c0b01234567/prune-worktrees.md\n"
    );
    let shown = ok(&lorectl(&vault, &["show", "prune-worktrees"], ""));
    assert!(shown.contains("status: retired\n"));
}

#[test]
fn mem_reject_and_refusals_have_exit_codes() {
    let dir = tempfile::tempdir().unwrap();
    let vault = dir.path().join("vault");
    ok(&lorectl(&vault, &["init"], ""));
    let id = propose(&vault, "s2", "Prune worktrees");
    assert_eq!(
        ok(&lorectl(&vault, &["reject", &id, "--reason", "dup"], "")),
        format!("rejected {id}\n")
    );
    let accept = lorectl(&vault, &["accept", &id], "");
    assert_eq!(accept.status.code(), Some(7));
    let missing = lorectl(&vault, &["accept", "01JC9V6Q4M8K2T7X3N5R0B1H6D"], "");
    assert_eq!(missing.status.code(), Some(1));
    let empty = lorectl(
        &vault,
        &[
            "propose",
            "--agent",
            "codex",
            "--session",
            "s",
            "--type",
            "fact",
            "--title",
            "T",
        ],
        "",
    );
    assert_eq!(empty.status.code(), Some(7));
    let no_reason = lorectl(&vault, &["reject", &id], "");
    assert_eq!(no_reason.status.code(), Some(1));
}

#[test]
fn mem_propose_refuses_noncanonical_repo_keys_with_exit_7() {
    let dir = tempfile::tempdir().unwrap();
    let vault = dir.path().join("vault");
    ok(&lorectl(&vault, &["init"], ""));
    for key in ["3f9a1c0b", "3F9A1C0B01234567", "3f9a1c0b012345678"] {
        let out = lorectl(
            &vault,
            &[
                "propose",
                "--agent",
                "codex",
                "--session",
                "s",
                "--type",
                "fact",
                "--title",
                "T",
                "--scope",
                &format!("repo:{key}"),
            ],
            "Statement.\n",
        );
        assert_eq!(
            out.status.code(),
            Some(7),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert!(!vault.join("_inbox").exists());
    }
}

#[test]
fn mem_accept_refuses_a_hand_edited_target_with_exit_7() {
    let dir = tempfile::tempdir().unwrap();
    let vault = dir.path().join("vault");
    ok(&lorectl(&vault, &["init"], ""));
    let id = propose(&vault, "s1", "Prune worktrees");
    let accepted = ok(&lorectl(&vault, &["accept", &id], ""));
    let note_id = accepted.split_whitespace().nth(2).unwrap();
    let path = vault.join("repos/3f9a1c0b01234567/prune-worktrees.md");
    let edited = std::fs::read_to_string(&path)
        .unwrap()
        .replace("Run `git worktree prune`.", "My edited statement.");
    std::fs::write(&path, &edited).unwrap();
    let out = ok(&lorectl(
        &vault,
        &[
            "propose",
            "--agent",
            "codex",
            "--session",
            "s2",
            "--type",
            "fact",
            "--title",
            "Update",
            "--updates",
            note_id,
        ],
        "New statement.\n",
    ));
    let update = out.split_whitespace().nth(1).unwrap();
    let refused = lorectl(&vault, &["accept", update], "");
    assert_eq!(
        refused.status.code(),
        Some(7),
        "{}",
        String::from_utf8_lossy(&refused.stderr)
    );
    assert_eq!(std::fs::read_to_string(path).unwrap(), edited);
}
