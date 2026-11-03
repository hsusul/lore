//! `lorectl mem inbox` shows each proposal's receipt: the archived session it
//! came from and a redacted excerpt of that session, read from the archive.
#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::io::Write;
use std::path::Path;
use std::process::{Command, Output, Stdio};

fn lorectl(home: &Path, args: &[&str], stdin: &str) -> Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_lorectl"))
        .args(args)
        .env("HOME", home)
        .env("LORE_VAULT_DIR", home.join("vault"))
        .env("LORE_ARCHIVE_DIR", home.join("archive"))
        .env("CLAUDE_CONFIG_DIR", home.join("claude"))
        .env("CODEX_HOME", home.join("codex"))
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
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

/// The fixture's native Codex session id.
const NATIVE: &str = "019e0000-0000-7000-8000-000000000002";

/// A vault, and an archive holding the Codex `function_call.jsonl` fixture.
/// Returns the session's archive id.
fn home_with_archive(home: &Path) -> String {
    ok(&lorectl(home, &["mem", "init"], ""));
    let day = home.join("codex/sessions/2026/08/11");
    std::fs::create_dir_all(&day).unwrap();
    std::fs::copy(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../lore-core/fixtures/codex/function_call.jsonl"),
        day.join("rollout-000000000002.jsonl"),
    )
    .unwrap();
    ok(&lorectl(home, &["scan"], ""));
    let conn = lore_core::storage::open_read_only(&home.join("archive/lore.db")).unwrap();
    conn.query_row("SELECT id FROM agent_session", [], |r| r.get(0))
        .unwrap()
}

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
            "howto",
            "--title",
            title,
        ],
        "Run the migration with npm before you start the server.\n",
    ));
    out.split_whitespace().nth(1).unwrap().to_owned()
}

#[test]
fn inbox_shows_the_source_session_and_an_excerpt_beside_each_proposal() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path();
    let archive_id = home_with_archive(home);
    let by_native = propose(home, NATIVE, "Run the migration first");
    let missing = propose(home, "s-gone", "Restart the server");

    assert_eq!(
        ok(&lorectl(home, &["mem", "inbox"], "")),
        format!(
            "{by_native}  codex:{NATIVE} (unverified)  howto  global  Run the migration first\n\
             \x20   from codex session {archive_id}, 2026-08-11: \"run the migration\"\n\
             {missing}  codex:s-gone (unverified)  howto  global  Restart the server\n\
             \x20   receipt: session s-gone is not in the archive\n"
        )
    );

    let json: serde_json::Value =
        serde_json::from_str(&ok(&lorectl(home, &["mem", "inbox", "--json"], ""))).unwrap();
    assert_eq!(
        json[0]["receipt"],
        serde_json::json!({
            "status": "found",
            "session": archive_id,
            "agent": "codex",
            "started": "2026-08-11",
            "excerpt": "run the migration",
        })
    );
    assert_eq!(
        json[1]["receipt"],
        serde_json::json!({ "status": "not_archived" })
    );
}

#[test]
fn inbox_says_so_in_one_line_when_there_is_no_archive() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path();
    ok(&lorectl(home, &["mem", "init"], ""));
    let id = propose(home, "s1", "Run the migration first");
    assert_eq!(
        ok(&lorectl(home, &["mem", "inbox"], "")),
        format!(
            "{id}  codex:s1 (unverified)  howto  global  Run the migration first\n\
             \x20   receipt: no archive yet; `lorectl scan` builds one\n"
        )
    );
    let json: serde_json::Value =
        serde_json::from_str(&ok(&lorectl(home, &["mem", "inbox", "--json"], ""))).unwrap();
    assert_eq!(
        json[0]["receipt"],
        serde_json::json!({ "status": "no_archive" })
    );
    assert!(!home.join("archive").exists(), "inbox created an archive");
}

#[test]
fn a_receipt_quotes_the_person_before_the_agent() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path();
    let archive_id = home_with_archive(home);
    // The agent's turn shares more words with this proposal than the
    // person's does, as a mined lesson's receipts do.
    let out = ok(&lorectl(
        home,
        &[
            "mem",
            "propose",
            "--agent",
            "codex",
            "--session",
            NATIVE,
            "--type",
            "fact",
            "--title",
            "Migration done",
        ],
        "Migration done after: \"Migration done.\"\n",
    ));
    let id = out.split_whitespace().nth(1).unwrap().to_owned();
    assert_eq!(
        ok(&lorectl(home, &["mem", "inbox"], "")),
        format!(
            "{id}  codex:{NATIVE} (unverified)  fact  global  Migration done\n\
             \x20   from codex session {archive_id}, 2026-08-11: \"run the migration\"\n"
        )
    );
}
