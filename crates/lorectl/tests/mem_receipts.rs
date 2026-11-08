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

/// The made-up Claude Code session's native id.
const CONTINUED: &str = "cccccccc-0000-4000-8000-000000000003";

/// A vault, and an archive holding one Claude Code session of `turns`,
/// alternating person and agent, the person first.
fn home_with_claude_session(home: &Path, turns: &[&str]) {
    let messages: Vec<(&str, serde_json::Value)> = (turns.iter().enumerate())
        .map(|(i, text)| {
            if i % 2 == 0 {
                ("user", serde_json::json!(text))
            } else {
                ("assistant", agent_text(text))
            }
        })
        .collect();
    home_with_claude_messages(home, &messages);
}

/// An agent message of one text part.
fn agent_text(text: &str) -> serde_json::Value {
    serde_json::json!([{ "type": "text", "text": text }])
}

/// A vault, and an archive holding one Claude Code session of `messages`,
/// each a role and the message content as Claude Code writes it.
fn home_with_claude_messages(home: &Path, messages: &[(&str, serde_json::Value)]) {
    ok(&lorectl(home, &["mem", "init"], ""));
    let project = home.join("claude/projects/p");
    std::fs::create_dir_all(&project).unwrap();
    let mut jsonl = String::new();
    let mut parent = serde_json::Value::Null;
    for (i, (kind, content)) in messages.iter().enumerate() {
        let uuid = format!("t{i}");
        let line = serde_json::json!({
            "type": kind, "uuid": uuid, "parentUuid": parent, "sessionId": CONTINUED,
            "timestamp": format!("2026-08-12T10:00:{i:02}.000Z"), "cwd": "/p",
            "isSidechain": false, "message": { "role": kind, "content": content },
        });
        jsonl.push_str(&format!("{line}\n"));
        parent = serde_json::json!(uuid);
    }
    std::fs::write(project.join(format!("{CONTINUED}.jsonl")), jsonl).unwrap();
    ok(&lorectl(home, &["scan"], ""));
}

/// The excerpt `mem inbox --json` shows for a proposal from the session.
fn excerpt_for(home: &Path, title: &str, body: &str) -> serde_json::Value {
    ok(&lorectl(
        home,
        &[
            "mem",
            "propose",
            "--agent",
            "claude-code",
            "--session",
            CONTINUED,
            "--type",
            "convention",
            "--title",
            title,
        ],
        body,
    ));
    let json: serde_json::Value =
        serde_json::from_str(&ok(&lorectl(home, &["mem", "inbox", "--json"], ""))).unwrap();
    json[0]["receipt"]["excerpt"].clone()
}

/// The summary a continued session opens with shares the most words with
/// the proposal, but the harness wrote it, not the person.
const SUMMARY: &str = "This session is being continued from a previous conversation. \
     Summary: the schema version pin and the migration retry were discussed; \
     pin the schema version before a migration retry.";

#[test]
fn a_receipt_skips_the_summary_a_continued_session_opens_with() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path();
    home_with_claude_session(
        home,
        &[
            SUMMARY,
            "Picking up from the summary.",
            "Stop retrying the flaky migration, pin the schema version first.",
            "Pinned it.",
        ],
    );
    assert_eq!(
        excerpt_for(
            home,
            "Pin the schema version before a migration retry",
            "Pin the schema version before a migration retry.\n",
        ),
        "Stop retrying the flaky migration, pin the schema version first."
    );
}

#[test]
fn a_receipt_shows_the_turn_the_proposal_quotes() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path();
    home_with_claude_session(
        home,
        &[
            SUMMARY,
            "Picking up from the summary.",
            "Stop retrying the flaky migration, pin the schema version first.",
            "Retrying once more.",
            "No, I said pin it before anything else runs.",
            "Pinned it.",
        ],
    );
    // The third turn shares more words; the fifth is the one quoted.
    assert_eq!(
        excerpt_for(
            home,
            "Pin the schema version before a migration retry",
            "Pin the schema version before a migration retry.\n\n\
             - 2026-08-12 claude-code s: \"I said pin it before anything else runs.\"\n",
        ),
        "...I said pin it before anything else runs."
    );
}

/// A lesson about leaving other people's branches alone.
const BRANCHES: &str = "Don't push to, merge, or edit other people's branches or PRs";
const BRANCHES_BODY: &str = "Only change work the person owns. Do not push commits to, \
     merge, rebase or edit a branch or PR someone else opened. If their branch needs a \
     change, say what and stop.\n\n\
     Why: the agent pushed onto a teammate's branch and the person had to have it reverted.\n";

/// A person's turn that ran `/ship-mode` with `args` typed after it.
fn command(args: &str) -> serde_json::Value {
    serde_json::json!(format!(
        "<command-message>ship-mode</command-message>\n\
         <command-name>/ship-mode</command-name>\n\
         <command-args>{args}</command-args>"
    ))
}

#[test]
fn a_receipt_shows_the_words_typed_after_a_command_not_the_skill_or_a_tool() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path();
    home_with_claude_messages(
        home,
        &[
            (
                "user",
                command("leave Sam's branch alone, never push to it"),
            ),
            // The skill's text, which Claude Code adds as the person's turn.
            (
                "user",
                serde_json::json!(
                    "Base directory for this skill: /home/me/.claude/skills/ship-mode\n\n\
                     # Ship mode\n\nYou own what lands. Push, merge, rebase and edit each \
                     branch and PR the person owns; verify every change before it lands."
                ),
            ),
            (
                "assistant",
                serde_json::json!([{
                    "type": "tool_use", "id": "read1", "name": "Read",
                    "input": { "file_path": "/home/me/.claude/skills/ship-mode/shipping.md" },
                }]),
            ),
            // A file the agent read, which arrives in the person's turn.
            (
                "user",
                serde_json::json!([{
                    "type": "tool_result", "tool_use_id": "read1",
                    "content": "### Shipping\n\nYou own what lands. Push commits, merge each \
                                PR, rebase a branch someone opened, edit and change what \
                                needs it, then stop.",
                }]),
            ),
            ("assistant", agent_text("Leaving it alone.")),
        ],
    );
    assert_eq!(
        excerpt_for(home, BRANCHES, BRANCHES_BODY),
        "leave Sam's branch alone, never push to it"
    );
}

#[test]
fn a_receipt_shows_the_persons_turn_most_about_the_lesson() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path();
    home_with_claude_messages(
        home,
        &[
            // A request that shares two words with the lesson by chance.
            (
                "user",
                command(
                    "can we start merging my older prs, group them prior to the launch, \
                     so 519 and below, group so we don't have to test every single one \
                     and make sure it works then start merging",
                ),
            ),
            ("assistant", agent_text("Merging 519 and below.")),
            // A go-ahead, all of whose words are the lesson's.
            ("user", serde_json::json!("Yes, merge them")),
            (
                "assistant",
                agent_text("Merged, and pushed a fix onto Sam's branch."),
            ),
            (
                "user",
                serde_json::json!("don't touch any of Sam's stuff, revert it"),
            ),
            ("assistant", agent_text("Reverted.")),
        ],
    );
    assert_eq!(
        excerpt_for(home, BRANCHES, BRANCHES_BODY),
        "don't touch any of Sam's stuff, revert it"
    );
}
