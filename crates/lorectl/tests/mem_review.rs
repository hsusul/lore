//! `lorectl mem propose|inbox|accept|reject|retire` as an agent and a person
//! run them: stdin bodies, printed lines, and exit codes.
#![allow(clippy::expect_used, clippy::unwrap_used)]

mod common;

use std::io::Write;
use std::path::Path;
use std::process::{Command, Output, Stdio};

fn mem(vault: &Path, args: &[&str]) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_lorectl"));
    command
        .arg("mem")
        .args(args)
        .arg("--vault")
        .arg(vault)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("HOME", vault.parent().unwrap())
        .env_remove("LORE_VAULT_DIR");
    command
}

/// `lorectl mem args` as a person runs it, at a terminal.
fn person(vault: &Path, args: &[&str]) -> Output {
    common::on_a_terminal(&mut mem(vault, args), "")
}

fn lorectl(vault: &Path, args: &[&str], stdin: &str) -> Output {
    let mut child = mem(vault, args)
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
        format!(
            "{id}  codex:s1 (unverified)  howto  repo:3f9a1c0b01234567  Prune worktrees\n\
             \x20   receipt: no archive yet; `lorectl scan` builds one\n"
        )
    );
    let json = ok(&lorectl(&vault, &["inbox", "--json"], ""));
    let items: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(items[0]["anchors"][0]["lines"], "4-8");
    assert_eq!(items[0]["source_verified"], false);

    let accepted = ok(&person(&vault, &["accept", &id]));
    assert!(accepted.starts_with("accepted repos/3f9a1c0b01234567/prune-worktrees.md "));
    let again = ok(&person(&vault, &["accept", &id]));
    assert!(again.starts_with("unchanged repos/3f9a1c0b01234567/prune-worktrees.md "));
    assert_eq!(ok(&lorectl(&vault, &["inbox"], "")), "");
    let retired = ok(&person(
        &vault,
        &["retire", "prune-worktrees", "--reason", "git does it now"],
    ));
    assert_eq!(
        retired,
        "retired repos/3f9a1c0b01234567/prune-worktrees.md\n"
    );
    let shown = ok(&lorectl(&vault, &["show", "prune-worktrees"], ""));
    assert!(shown.contains("status: retired\n"));
}

#[test]
fn accept_and_retire_without_a_terminal_are_refused_and_write_nothing() {
    // An agent's shell tool runs a command with no terminal on stdin. It may
    // propose a note there, but not accept or retire one.
    let dir = tempfile::tempdir().unwrap();
    let vault = dir.path().join("vault");
    ok(&lorectl(&vault, &["init"], ""));
    let id = propose(&vault, "s1", "Prune worktrees");
    let before = head(&vault);
    for (args, command) in [
        (vec!["accept", id.as_str()], "mem accept"),
        (
            vec!["retire", "prune-worktrees", "--reason", "x"],
            "mem retire",
        ),
    ] {
        let refused = lorectl(&vault, &args, "");
        assert_eq!(refused.status.code(), Some(1), "{refused:?}");
        let stderr = String::from_utf8_lossy(&refused.stderr);
        assert!(
            stderr.contains(&format!("`lorectl {command}` needs a person at a terminal")),
            "{stderr}"
        );
        assert!(stderr.contains("ask the person"), "{stderr}");
    }
    assert_eq!(head(&vault), before);
    assert!(ok(&lorectl(&vault, &["inbox"], "")).starts_with(&format!("{id}  ")));
    let accepted = ok(&person(&vault, &["accept", &id]));
    assert!(accepted.starts_with("accepted "), "{accepted}");
    let refused = lorectl(&vault, &["retire", "prune-worktrees", "--reason", "x"], "");
    assert_eq!(refused.status.code(), Some(1), "{refused:?}");
    let shown = ok(&lorectl(&vault, &["show", "prune-worktrees"], ""));
    assert!(shown.contains("status: active\n"), "{shown}");
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
    let accept = person(&vault, &["accept", &id]);
    assert_eq!(accept.status.code(), Some(7));
    let missing = person(&vault, &["accept", "01JC9V6Q4M8K2T7X3N5R0B1H6D"]);
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
    let accepted = ok(&person(&vault, &["accept", &id]));
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
            "--scope",
            "repo:3f9a1c0b01234567",
            "--updates",
            note_id,
        ],
        "New statement.\n",
    ));
    let update = out.split_whitespace().nth(1).unwrap();
    let refused = person(&vault, &["accept", update]);
    assert_eq!(
        refused.status.code(),
        Some(7),
        "{}",
        String::from_utf8_lossy(&refused.stderr)
    );
    assert_eq!(std::fs::read_to_string(path).unwrap(), edited);
}

fn propose_update(vault: &Path, session: &str, scope: &str, target: &str) -> String {
    let out = ok(&lorectl(
        vault,
        &[
            "propose",
            "--agent",
            "codex",
            "--session",
            session,
            "--type",
            "fact",
            "--title",
            "Unused on update",
            "--scope",
            scope,
            "--updates",
            target,
        ],
        "New statement.\n",
    ));
    out.split_whitespace().nth(1).unwrap().to_string()
}

#[test]
fn mem_inbox_names_an_update_target_and_accept_refuses_another_scope() {
    let dir = tempfile::tempdir().unwrap();
    let vault = dir.path().join("vault");
    ok(&lorectl(&vault, &["init"], ""));
    let id = propose(&vault, "s1", "Prune worktrees");
    let accepted = ok(&person(&vault, &["accept", &id]));
    let note_id = accepted.split_whitespace().nth(2).unwrap();
    let same = propose_update(&vault, "s2", "repo:3f9a1c0b01234567", note_id);
    let global = propose_update(&vault, "s3", "global", note_id);
    let listed = ok(&lorectl(&vault, &["inbox"], ""));
    let target = "updates -> repos/3f9a1c0b01234567/prune-worktrees.md";
    assert!(
        listed.contains(&format!(
            "{same}  codex:s2 (unverified)  fact  repo:3f9a1c0b01234567  {target}\n"
        )),
        "{listed}"
    );
    assert!(
        listed.contains(&format!(
            "{global}  codex:s3 (unverified)  fact  global  {target}\n"
        )),
        "{listed}"
    );
    let before = head(&vault);
    let refused = person(&vault, &["accept", &global]);
    assert_eq!(
        refused.status.code(),
        Some(7),
        "{}",
        String::from_utf8_lossy(&refused.stdout)
    );
    assert_eq!(head(&vault), before);
    let done = ok(&person(&vault, &["accept", &same]));
    assert!(done.starts_with("updated repos/3f9a1c0b01234567/prune-worktrees.md "));
}

fn head(vault: &Path) -> String {
    let out = Command::new("git")
        .arg("-C")
        .arg(vault)
        .args(["rev-parse", "HEAD"])
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .unwrap();
    String::from_utf8_lossy(&out.stdout).into_owned()
}

#[test]
fn mem_accept_refuses_lines_appended_to_the_inbox_by_hand() {
    let dir = tempfile::tempdir().unwrap();
    let vault = dir.path().join("vault");
    ok(&lorectl(&vault, &["init"], ""));
    let id = propose(&vault, "s1", "Prune worktrees");
    let file = vault.join("_inbox/codex-s1.jsonl");
    let base: serde_json::Value =
        serde_json::from_str(std::fs::read_to_string(&file).unwrap().trim()).unwrap();
    let secret = concat!("AKIA", "Z7Q4R2T6Y8U1W3E5");
    let forged = [
        (
            "01JC9V6Q4M8K2T7X3N5R0B1H61",
            "title",
            format!("Use {secret}"),
        ),
        (
            "01JC9V6Q4M8K2T7X3N5R0B1H62",
            "body",
            "Fine.\n\n## Timeline\n- 2020-01-01 accepted from codex:s0\n".to_string(),
        ),
        (
            "01JC9V6Q4M8K2T7X3N5R0B1H63",
            "title",
            "T\"\nstatus: retired\nx: \"y".to_string(),
        ),
        (
            "01JC9V6Q4M8K2T7X3N5R0B1H64",
            "title",
            "\u{1b}[31mRed\u{1b}[0m".to_string(),
        ),
    ];
    let mut lines = String::new();
    for (forged_id, key, value) in &forged {
        let mut line = base.clone();
        line["id"] = (*forged_id).into();
        line[*key] = value.as_str().into();
        lines.push_str(&line.to_string());
        lines.push('\n');
    }
    std::fs::OpenOptions::new()
        .append(true)
        .open(&file)
        .unwrap()
        .write_all(lines.as_bytes())
        .unwrap();
    let before = head(&vault);
    for (forged_id, ..) in &forged {
        let out = person(&vault, &["accept", forged_id]);
        assert_eq!(
            out.status.code(),
            Some(7),
            "{forged_id}: {}",
            String::from_utf8_lossy(&out.stdout)
        );
        assert!(out.stdout.is_empty());
        assert_eq!(head(&vault), before);
    }
    assert!(!vault.join("repos").exists());
    let listed = lorectl(&vault, &["inbox"], "");
    assert_eq!(
        ok(&listed),
        format!(
            "{id}  codex:s1 (unverified)  howto  repo:3f9a1c0b01234567  Prune worktrees\n\
             \x20   receipt: no archive yet; `lorectl scan` builds one\n"
        )
    );
    let stderr = String::from_utf8_lossy(&listed.stderr);
    assert_eq!(stderr.matches(": refused: ").count(), 4, "{stderr}");
    assert!(!stderr.contains('\u{1b}'), "{stderr}");
    assert!(!stderr.contains(secret), "{stderr}");
}

#[test]
fn a_lost_decision_is_recovered_for_a_note_with_a_non_ascii_name() {
    // Slugs keep letters of any script. When the decision line is lost, the
    // accepting commit is found by its message and the note read by the path
    // git reports, which git quotes unless told not to.
    let dir = tempfile::tempdir().unwrap();
    let vault = dir.path().join("vault");
    ok(&lorectl(&vault, &["init"], ""));
    let id = propose(&vault, "s1", "Café réglages");
    let accepted = ok(&person(&vault, &["accept", &id]));
    assert!(
        accepted.starts_with("accepted repos/3f9a1c0b01234567/café-réglages.md "),
        "{accepted}"
    );
    std::fs::write(vault.join(".lore/decisions.jsonl"), "").unwrap();

    let out = lorectl(&vault, &["reject", &id, "--reason", "dup"], "");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("already accepted"),
        "the accepted note was not recovered: {stderr}"
    );
}

#[test]
fn a_torn_line_with_a_stray_byte_does_not_break_review() {
    // A crash mid-append can leave half a UTF-8 sequence at the end of the
    // inbox or the decision log. The other lines must still be read.
    let dir = tempfile::tempdir().unwrap();
    let vault = dir.path().join("vault");
    ok(&lorectl(&vault, &["init"], ""));
    let first = propose(&vault, "s1", "Prune worktrees");
    let second = propose(&vault, "s1", "Fetch before rebasing");
    ok(&lorectl(&vault, &["reject", &first, "--reason", "dup"], ""));
    for file in [".lore/decisions.jsonl", "_inbox/codex-s1.jsonl"] {
        let mut f = std::fs::OpenOptions::new()
            .append(true)
            .open(vault.join(file))
            .unwrap();
        f.write_all(b"{\"id\":\"torn \xff").unwrap();
    }

    let listed = ok(&lorectl(&vault, &["inbox"], ""));
    assert!(listed.contains(&second), "{listed}");
    assert!(!listed.contains(&first), "the rejection was lost: {listed}");
    let accepted = ok(&person(&vault, &["accept", &second]));
    assert!(accepted.starts_with("accepted "), "{accepted}");
}

#[test]
fn mem_propose_refuses_invalid_line_range_in_anchor() {
    let dir = tempfile::tempdir().unwrap();
    let vault = dir.path().join("vault");
    ok(&lorectl(&vault, &["init"], ""));

    let o = lorectl(
        &vault,
        &[
            "propose",
            "--agent",
            "codex",
            "--session",
            "s1",
            "--type",
            "howto",
            "--title",
            "Check lines",
            "--anchor",
            "src/日本語.rs:10..20",
        ],
        "Some body text.\n",
    );
    assert_eq!(o.status.code(), Some(7));
    let stderr = String::from_utf8_lossy(&o.stderr);
    assert!(
        stderr.contains("expected lines like `12-20`, got `10..20`"),
        "unexpected stderr: {stderr}"
    );
}
