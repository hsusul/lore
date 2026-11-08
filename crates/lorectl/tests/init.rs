//! `lorectl init` as a person runs it the first time, and again: the vault,
//! a scan of recent sessions, and the lessons in them, then what to do next.
#![allow(clippy::expect_used, clippy::unwrap_used)]

mod common;

use std::io::Write;
use std::path::Path;
use std::process::{Command, Output, Stdio};

fn lorectl(home: &Path, args: &[&str]) -> Output {
    lorectl_in(home, args, "")
}

fn command(home: &Path, args: &[&str]) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_lorectl"));
    command
        .args(args)
        .env("HOME", home)
        .env("LORE_VAULT_DIR", home.join("vault"))
        .env("LORE_ARCHIVE_DIR", home.join("archive"))
        .env("CLAUDE_CONFIG_DIR", home.join("claude"))
        .env("CODEX_HOME", home.join("codex"))
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1");
    command
}

/// `lorectl args` as a person runs it, typing `answers` at a terminal. The
/// terminal shows what they type, so stdout has only the prompts.
fn person(home: &Path, args: &[&str], answers: &str) -> Output {
    common::on_a_terminal(&mut command(home, args), answers)
}

/// `lorectl args` with `stdin` as its piped input.
fn lorectl_in(home: &Path, args: &[&str], stdin: &str) -> Output {
    let mut child = command(home, args)
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

/// Stdout of a run that exited 0, with the elapsed seconds masked as `N`.
fn masked(o: &Output) -> String {
    assert_eq!(
        o.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&o.stderr)
    );
    let text = String::from_utf8(o.stdout.clone()).unwrap();
    // The scan's seconds, wherever they sit on the Archive line.
    text.lines()
        .map(|line| match line.split_once(" in ") {
            Some((head, tail)) if line.starts_with("Archive") => {
                let rest = tail.split_once(" s").map_or("", |(_, rest)| rest);
                format!("{head} in N s{rest}\n")
            }
            _ => format!("{line}\n"),
        })
        .collect()
}

/// One Claude Code session on `day`: a request, the agent's reply, and the
/// person's correction.
fn claude_session(home: &Path, n: u32, day: &str, reply: &str, correction: &str) {
    let project = home.join("claude/projects/-proj");
    std::fs::create_dir_all(&project).unwrap();
    let session = format!("5e550000-0000-4000-8000-{n:012x}");
    let line = |i: u32, kind: &str, parent: Option<String>, message: serde_json::Value| {
        serde_json::json!({
            "type": kind, "uuid": format!("{kind}-{n}-{i}"), "parentUuid": parent,
            "sessionId": session, "cwd": "/proj",
            "timestamp": format!("{day}T10:00:0{i}.000Z"), "message": message,
        })
    };
    let lines = [
        line(
            0,
            "user",
            None,
            serde_json::json!({"role": "user", "content": "tidy up the repository"}),
        ),
        line(
            1,
            "assistant",
            Some(format!("user-{n}-0")),
            serde_json::json!({"role": "assistant",
                "content": [{"type": "text", "text": reply}]}),
        ),
        line(
            2,
            "user",
            Some(format!("assistant-{n}-1")),
            serde_json::json!({"role": "user", "content": correction}),
        ),
    ];
    let body: String = lines.iter().map(|l| format!("{l}\n")).collect();
    std::fs::write(project.join(format!("{session}.jsonl")), body).unwrap();
}

fn archived_id(home: &Path, n: u32) -> String {
    let conn = lore_core::storage::open_read_only(&home.join("archive/lore.db")).unwrap();
    conn.query_row(
        "SELECT id FROM agent_session WHERE native_session_id = ?1",
        [format!("5e550000-0000-4000-8000-{n:012x}")],
        |r| r.get(0),
    )
    .unwrap()
}

const NEXT: &str = "\
Next
  Better lessons: in Claude Code, run /lore:lore-lessons, then lorectl init --review
  Keep one:       lorectl mem mine --propose <id>, then lorectl mem accept
  Connect agents: lorectl connect
";

#[test]
fn init_creates_the_vault_scans_and_shows_the_lesson_then_reuses_both() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path();
    claude_session(
        home,
        1,
        "2026-10-20",
        "Added docs/ to the index.",
        "no, docs are never tracked in git",
    );
    claude_session(
        home,
        2,
        "2026-10-28",
        "Staged docs/notes.md with the rest.",
        "I told you, docs are never tracked in git",
    );
    // A one-off task order, mined as a lesson but with a rule score below 0
    // for the branch it names, so init and `mem mine` leave it out.
    for (n, day) in [(3, "2026-10-25"), (4, "2026-10-26")] {
        claude_session(
            home,
            n,
            day,
            "Pushed to main.",
            "no, push to the feat/login-fix branch",
        );
    }

    let first = masked(&lorectl(home, &["init"]));
    let id = first
        .lines()
        .find(|l| l.contains("2 conversations"))
        .and_then(|l| l.split_whitespace().next())
        .unwrap_or("<missing>")
        .to_owned();
    let lessons = format!(
        "Lessons    1 candidate from your corrections:\n\
         \n\
         \x20 {id}  2 conversations, latest 2026-10-28\n\
         \x20     \"I told you, docs are never tracked in git\"\n\
         \x20     suggested: Docs are never tracked in git\n\
         \x20     - claude-code {s2} 2026-10-28: \"I told you, docs are never tracked in git\"\n\
         \n",
        s2 = archived_id(home, 2),
    );
    let vault = home.join("vault");
    assert_eq!(
        first,
        format!(
            "Lore\n\
             Vault      created {vault}\n\
             Archive    scanned 4 new sessions from the last 30d in N s\n\
             {lessons}{NEXT}",
            vault = vault.display()
        )
    );
    assert!(vault.join(".git").exists(), "init made no vault");
    let mined = String::from_utf8(lorectl(home, &["mem", "mine"]).stdout).unwrap();
    assert!(
        mined.starts_with("1 lesson from 4 corrections, 1 more scored as one-off orders"),
        "{mined}"
    );

    let second = masked(&lorectl(home, &["init", "--since", "7d", "--limit", "1"]));
    assert_eq!(
        second,
        format!(
            "Lore\n\
             Vault      {vault}, 0 notes\n\
             Archive    no new sessions from the last 7d in N s\n\
             {lessons}{NEXT}",
            vault = vault.display()
        )
    );
}

#[test]
fn init_on_a_home_with_no_sessions_says_there_is_nothing_yet() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path();
    let out = masked(&lorectl(home, &["init"]));
    assert_eq!(
        out,
        format!(
            "Lore\n\
             Vault      created {vault}\n\
             Archive    no new sessions from the last 30d in N s\n\
             Lessons    none found in your sessions yet.\n\
             \n\
             {NEXT}",
            vault = home.join("vault").display()
        )
    );
}

#[test]
fn init_refuses_a_bad_limit_a_bad_window_and_a_stray_operand() {
    let dir = tempfile::tempdir().unwrap();
    for args in [
        &["init", "--limit", "0"][..],
        &["init", "--since", "soon"],
        &["init", "stray"],
    ] {
        let out = lorectl(dir.path(), args);
        assert_eq!(out.status.code(), Some(1), "{args:?}");
    }
    assert!(
        !dir.path().join("vault").exists(),
        "a usage error wrote a vault"
    );
}

const DOCS: &str = "make sure that docs are never tracked in git please";
const FMT: &str = "make sure you always run cargo fmt before you commit";
const PROMPT: &str = "      [a]ccept  [e]dit  [s]kip  [q]uit ";

/// Two lessons, each said in two conversations: docs (newest 2026-10-28,
/// sessions 1 and 2) and fmt (newest 2026-10-26, sessions 3 and 4).
fn two_lessons(home: &Path) {
    claude_session(home, 1, "2026-10-20", "Added docs/ to the index.", DOCS);
    claude_session(home, 2, "2026-10-28", "Staged docs/notes.md.", DOCS);
    claude_session(home, 3, "2026-10-25", "Committed the change.", FMT);
    claude_session(home, 4, "2026-10-26", "Committed it unformatted.", FMT);
}

/// The block init prints for a lesson, without what follows it.
fn block(home: &Path, id: &str, n: u32, day: &str, text: &str, rule: &str) -> String {
    format!(
        "  {id}  2 conversations, latest {day}\n      \"{text}\"\n      suggested: {rule}\n      - claude-code {s} {day}: \"{text}\"\n",
        s = archived_id(home, n),
    )
}

/// The lesson ids in init's output, in the order shown.
fn ids(out: &str) -> Vec<String> {
    out.lines()
        .filter(|l| l.contains(" conversations, latest "))
        .filter_map(|l| l.split_whitespace().next().map(str::to_owned))
        .collect()
}

fn kept(n: usize) -> String {
    done(0, n)
}

/// init's last line after accepting `accepted` proposals and keeping `kept`
/// lessons.
fn done(accepted: usize, kept: usize) -> String {
    let s = |n: usize| if n == 1 { "" } else { "s" };
    let what = match (accepted, kept) {
        (0, k) => format!("Kept {k} lesson{}", s(k)),
        (a, 0) => format!("Accepted {a} proposal{}", s(a)),
        (a, k) => format!("Accepted {a} proposal{} and kept {k} lesson{}", s(a), s(k)),
    };
    format!("{what}. Your next Claude Code or Codex session will see them once Lore is connected (lorectl connect).\n")
}

fn git(vault: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .arg("-C")
        .arg(vault)
        .args(args)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .unwrap();
    String::from_utf8(out.stdout).unwrap()
}

fn header(home: &Path) -> String {
    format!(
        "Lore\n\
         Vault      created {}\n\
         Archive    scanned 4 new sessions from the last 30d in N s\n\
         Lessons    2 candidates from your corrections:\n\n",
        home.join("vault").display()
    )
}

#[test]
fn review_a_keeps_the_suggested_rule_as_a_note_and_a_second_run_shows_it_kept() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path();
    two_lessons(home);
    let vault = home.join("vault");

    let first = masked(&person(home, &["init", "--review"], "a\ns\n"));
    let [docs, fmt] = <[String; 2]>::try_from(ids(&first)).unwrap();
    let docs_rule = "Docs are never tracked in git";
    let fmt_rule = "Always run cargo fmt before you commit";
    let docs_block = block(home, &docs, 2, "2026-10-28", DOCS, docs_rule);
    let fmt_block = block(home, &fmt, 4, "2026-10-26", FMT, fmt_rule);
    assert_eq!(
        first,
        format!(
            "{}{docs_block}{PROMPT}      kept global/docs-are-never-tracked-in-git.md\n\n\
             {fmt_block}{PROMPT}\n{}",
            header(home),
            kept(1)
        )
    );
    let note =
        std::fs::read_to_string(vault.join("global/docs-are-never-tracked-in-git.md")).unwrap();
    for line in [
        "title: Docs are never tracked in git",
        "type: convention",
        "scope: global",
        DOCS,
        &format!(
            "Mined by `lorectl mem mine` (lesson {docs}) from corrections in 2 conversations:"
        ),
    ] {
        assert!(note.lines().any(|l| l == line), "no `{line}` in\n{note}");
    }
    assert_eq!(git(&vault, &["status", "--porcelain"]), "");
    assert_eq!(git(&vault, &["rev-list", "--count", "HEAD"]), "2\n");

    // Again, with stdin at its end: the kept lesson is not offered, and the
    // end of input stops the review at the next one.
    let second = masked(&person(home, &["init", "--review"], ""));
    assert_eq!(
        second,
        format!(
            "Lore\n\
             Vault      {}, 1 note\n\
             Archive    no new sessions from the last 30d in N s\n\
             Lessons    2 candidates from your corrections:\n\n\
             {docs_block}      (already kept)\n\n\
             {fmt_block}{PROMPT}\n\n{}",
            vault.display(),
            kept(0)
        )
    );

    // The accept is a decision mem eval scores; the skip is not.
    let eval = String::from_utf8(lorectl(home, &["mem", "eval"]).stdout).unwrap();
    assert!(
        eval.starts_with(
            "Policy never, scored on 1 proposals you decided: 1 accepted, 0 rejected.\n"
        ),
        "{eval}"
    );
}

#[test]
fn review_e_keeps_the_new_text_and_an_empty_edit_keeps_the_suggestion() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path();
    two_lessons(home);
    let vault = home.join("vault");

    // A stray answer asks once more.
    let script = "e\nKeep docs out of git\nx\ne\n\n";
    let out = masked(&person(home, &["init", "--review"], script));
    let [docs, fmt] = <[String; 2]>::try_from(ids(&out)).unwrap();
    let docs_block = block(
        home,
        &docs,
        2,
        "2026-10-28",
        DOCS,
        "Docs are never tracked in git",
    );
    let fmt_rule = "Always run cargo fmt before you commit";
    let fmt_block = block(home, &fmt, 4, "2026-10-26", FMT, fmt_rule);
    assert_eq!(
        out,
        format!(
            "{}{docs_block}{PROMPT}      rule:       kept global/keep-docs-out-of-git.md\n\n\
             {fmt_block}{PROMPT}{PROMPT}      rule:       kept global/always-run-cargo-fmt-before-you-commit.md\n\n{}",
            header(home),
            kept(2)
        )
    );
    let note = std::fs::read_to_string(vault.join("global/keep-docs-out-of-git.md")).unwrap();
    assert!(
        note.lines().any(|l| l == "title: Keep docs out of git"),
        "{note}"
    );
    let note =
        std::fs::read_to_string(vault.join("global/always-run-cargo-fmt-before-you-commit.md"))
            .unwrap();
    assert!(
        note.lines().any(|l| l == format!("title: {fmt_rule}")),
        "{note}"
    );
    assert_eq!(git(&vault, &["status", "--porcelain"]), "");
    assert_eq!(git(&vault, &["rev-list", "--count", "HEAD"]), "3\n");
}

#[test]
fn review_s_and_two_stray_answers_leave_the_vault_unchanged() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path();
    two_lessons(home);
    let vault = home.join("vault");

    let out = masked(&person(home, &["init", "--review"], "s\nx\ny\n"));
    let [docs, fmt] = <[String; 2]>::try_from(ids(&out)).unwrap();
    let docs_block = block(
        home,
        &docs,
        2,
        "2026-10-28",
        DOCS,
        "Docs are never tracked in git",
    );
    let fmt_rule = "Always run cargo fmt before you commit";
    let fmt_block = block(home, &fmt, 4, "2026-10-26", FMT, fmt_rule);
    assert_eq!(
        out,
        format!(
            "{}{docs_block}{PROMPT}\n{fmt_block}{PROMPT}{PROMPT}\n{}",
            header(home),
            kept(0)
        )
    );
    assert!(!vault.join("_inbox").exists(), "a skip filed a proposal");
    assert!(
        !vault.join(".lore/decisions.jsonl").exists(),
        "a skip was recorded"
    );
    assert_eq!(git(&vault, &["rev-list", "--count", "HEAD"]), "1\n");
}

#[test]
fn review_q_stops_before_the_next_lesson() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path();
    two_lessons(home);
    let vault = home.join("vault");

    let out = masked(&person(home, &["init", "--review"], "q\na\n"));
    let [docs] = <[String; 1]>::try_from(ids(&out)).unwrap();
    let docs_block = block(
        home,
        &docs,
        2,
        "2026-10-28",
        DOCS,
        "Docs are never tracked in git",
    );
    assert_eq!(
        out,
        format!("{}{docs_block}{PROMPT}\n{}", header(home), kept(0))
    );
    assert!(!vault.join("_inbox").exists(), "quitting filed a proposal");
    assert_eq!(git(&vault, &["rev-list", "--count", "HEAD"]), "1\n");

    // The last of --review and --no-review counts; off prints the listing.
    let plain = masked(&lorectl_in(
        home,
        &["init", "--review", "--no-review"],
        "a\n",
    ));
    assert!(plain.ends_with(NEXT), "{plain}");
    assert!(!plain.contains("[a]ccept"), "{plain}");
}

const INBOX_PROMPT: &str = "      [a]ccept  [r]eject  [s]kip  [q]uit ";
const ELSEWHERE: &str = "elsewhere-1";

/// File a proposal the way an agent without MCP does. Returns its id.
fn propose(home: &Path, session: &str, title: &str, body: &str) -> String {
    let args = [
        "mem",
        "propose",
        "--agent",
        "claude-code",
        "--session",
        session,
        "--type",
        "convention",
        "--title",
        title,
    ];
    let out = lorectl_in(home, &args, body);
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    let text = String::from_utf8(out.stdout).unwrap();
    text.split_whitespace().nth(1).unwrap().to_owned()
}

/// A vault holding two pending proposals: one from an archived session that
/// said nothing to mine, one from a session the archive never saw.
fn two_proposals(home: &Path) -> (String, String) {
    claude_session(
        home,
        1,
        "2026-10-28",
        "Tidied it.",
        "thanks, that looks good",
    );
    assert_eq!(lorectl(home, &["mem", "init"]).status.code(), Some(0));
    let native = "5e550000-0000-4000-8000-000000000001";
    let tidy = propose(
        home,
        native,
        "Tidy up the repository weekly",
        "Tidy up the repository once a week.\n",
    );
    let friday = propose(
        home,
        ELSEWHERE,
        "Never push on a Friday",
        "Never push on a Friday.\n",
    );
    (tidy, friday)
}

/// The `Lore`, `Vault`, `Archive`, and `Inbox` lines of a first init over
/// [`two_proposals`].
fn inbox_header(home: &Path) -> String {
    format!(
        "Lore\n\
         Vault      {}, 0 notes\n\
         Archive    scanned 1 new session from the last 30d in N s\n\
         Inbox      2 proposals waiting for review:\n\n",
        home.join("vault").display()
    )
}

fn tidy_block(home: &Path, id: &str) -> String {
    format!(
        "  {id}  Tidy up the repository weekly\n\
         \x20     claude-code:5e550000-0000-4000-8000-000000000001 (unverified)\n\
         \x20     from claude-code session {s}, 2026-10-28: \"tidy up the repository\"\n",
        s = archived_id(home, 1)
    )
}

fn friday_block(id: &str) -> String {
    format!(
        "  {id}  Never push on a Friday\n\
         \x20     claude-code:{ELSEWHERE} (unverified)\n\
         \x20     receipt: session {ELSEWHERE} is not in the archive\n"
    )
}

fn pending(home: &Path) -> String {
    String::from_utf8(lorectl(home, &["mem", "inbox"]).stdout).unwrap()
}

#[test]
fn review_accepts_one_proposal_and_rejects_the_other_with_a_reason() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path();
    let (tidy, friday) = two_proposals(home);
    let vault = home.join("vault");

    let out = masked(&person(home, &["init", "--review"], "a\nr\ntoo vague\n"));
    assert_eq!(
        out,
        format!(
            "{}{}{INBOX_PROMPT}      accepted global/tidy-up-the-repository-weekly.md\n\n\
             {}{INBOX_PROMPT}      reason:       rejected\n\n\
             Lessons    none found in your sessions yet.\n\n{}",
            inbox_header(home),
            tidy_block(home, &tidy),
            friday_block(&friday),
            done(1, 0)
        )
    );
    let note =
        std::fs::read_to_string(vault.join("global/tidy-up-the-repository-weekly.md")).unwrap();
    for line in [
        "title: Tidy up the repository weekly",
        "type: convention",
        "Tidy up the repository once a week.",
    ] {
        assert!(note.lines().any(|l| l == line), "no `{line}` in\n{note}");
    }
    assert_eq!(git(&vault, &["status", "--porcelain"]), "");
    assert_eq!(pending(home), "");
    let decisions = std::fs::read_to_string(vault.join(".lore/decisions.jsonl")).unwrap();
    assert!(
        decisions.contains(&format!(
            "\"decision\":\"rejected\",\"id\":\"{friday}\",\"reason\":\"too vague\""
        )),
        "{decisions}"
    );
    let eval = String::from_utf8(lorectl(home, &["mem", "eval"]).stdout).unwrap();
    assert!(
        eval.starts_with(
            "Policy never, scored on 2 proposals you decided: 1 accepted, 1 rejected.\n"
        ),
        "{eval}"
    );

    // Nothing is pending now, so a second run has no Inbox step.
    let again = masked(&lorectl(home, &["init"]));
    assert_eq!(
        again,
        format!(
            "Lore\n\
             Vault      {}, 1 note\n\
             Archive    no new sessions from the last 30d in N s\n\
             Lessons    none found in your sessions yet.\n\n{NEXT}",
            vault.display()
        )
    );
}

#[test]
fn review_skip_leaves_a_proposal_pending_and_an_empty_reason_is_filled_in() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path();
    let (tidy, friday) = two_proposals(home);
    let vault = home.join("vault");

    let out = masked(&person(home, &["init", "--review"], "s\nr\n\n"));
    assert_eq!(
        out,
        format!(
            "{}{}{INBOX_PROMPT}\n{}{INBOX_PROMPT}      reason:       rejected\n\n\
             Lessons    none found in your sessions yet.\n\n{}",
            inbox_header(home),
            tidy_block(home, &tidy),
            friday_block(&friday),
            kept(0)
        )
    );
    let decisions = std::fs::read_to_string(vault.join(".lore/decisions.jsonl")).unwrap();
    assert!(
        decisions.contains("\"reason\":\"rejected in lorectl init\""),
        "{decisions}"
    );
    assert!(
        !decisions.contains(&tidy),
        "a skip was recorded: {decisions}"
    );
    let still = pending(home);
    assert!(still.starts_with(&format!("{tidy}  ")), "{still}");
    assert!(!still.contains(&friday), "{still}");
    assert_eq!(git(&vault, &["rev-list", "--count", "HEAD"]), "1\n");
}

#[test]
fn review_with_piped_answers_is_refused_and_accepts_nothing() {
    // Answers piped in could come from an agent's shell tool, so `--review`
    // without a terminal is refused before anything is offered.
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path();
    let (tidy, friday) = two_proposals(home);
    two_lessons(home);

    let out = lorectl_in(home, &["init", "--review"], "a\na\na\na\n");
    assert_eq!(out.status.code(), Some(1), "{out:?}");
    assert_eq!(String::from_utf8_lossy(&out.stdout), "");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("`lorectl init --review` needs a person at a terminal"),
        "{stderr}"
    );
    let still = pending(home);
    assert!(still.contains(&tidy) && still.contains(&friday), "{still}");
    assert!(!home.join("vault/global").exists(), "a note was written");
}

#[test]
fn review_q_on_a_proposal_stops_the_review() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path();
    let (tidy, friday) = two_proposals(home);

    let out = masked(&person(home, &["init", "--review"], "q\na\n"));
    assert_eq!(
        out,
        format!(
            "{}{}{INBOX_PROMPT}\n{}",
            inbox_header(home),
            tidy_block(home, &tidy),
            kept(0)
        )
    );
    let still = pending(home);
    assert!(still.contains(&tidy) && still.contains(&friday), "{still}");
}

#[test]
fn init_without_review_lists_pending_proposals_before_the_lessons() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path();
    let (tidy, friday) = two_proposals(home);

    let out = masked(&lorectl(home, &["init"]));
    assert_eq!(
        out,
        format!(
            "{}{}\n{}\n\
             Lessons    none found in your sessions yet.\n\n{NEXT}",
            inbox_header(home),
            tidy_block(home, &tidy),
            friday_block(&friday),
        )
    );
    let still = pending(home);
    assert!(still.contains(&tidy) && still.contains(&friday), "{still}");
}

#[test]
fn review_says_a_lesson_was_rejected_or_is_waiting_instead_of_kept() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path();
    two_lessons(home);
    let listed = masked(&lorectl(home, &["init", "--no-review"]));
    let [docs, fmt] = <[String; 2]>::try_from(ids(&listed)).unwrap();
    for id in [&docs, &fmt] {
        let out = lorectl(home, &["mem", "mine", "--propose", id]);
        assert_eq!(
            out.status.code(),
            Some(0),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
    let inbox: serde_json::Value =
        serde_json::from_slice(&lorectl(home, &["mem", "inbox", "--json"]).stdout).unwrap();
    let docs_proposal = inbox
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["body"].as_str().unwrap().contains(&docs))
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let out = lorectl(
        home,
        &["mem", "reject", &docs_proposal, "--reason", "not mine"],
    );
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );

    // The fmt proposal is still pending: skip it in the inbox step.
    let out = masked(&person(home, &["init", "--review"], "s\n"));
    let docs_block = block(
        home,
        &docs,
        2,
        "2026-10-28",
        DOCS,
        "Docs are never tracked in git",
    );
    let fmt_block = block(
        home,
        &fmt,
        4,
        "2026-10-26",
        FMT,
        "Always run cargo fmt before you commit",
    );
    assert!(
        out.contains(&format!("{docs_block}      (rejected)\n\n")),
        "{out}"
    );
    assert!(
        out.contains(&format!("{fmt_block}      (in the inbox)\n\n")),
        "{out}"
    );
}

#[test]
fn the_last_line_counts_accepted_proposals_apart_from_kept_lessons() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path();
    two_proposals(home);
    two_lessons(home);
    // Accept the first proposal, skip the second, keep the first lesson,
    // skip the second.
    let out = masked(&person(home, &["init", "--review"], "a\ns\na\ns\n"));
    assert!(out.ends_with(&done(1, 1)), "{out}");
}

#[test]
fn init_stops_scanning_at_its_time_budget_and_says_what_is_left() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path();
    two_lessons(home);
    let vault = home.join("vault");
    let first = masked(&lorectl(
        home,
        &["init", "--no-review", "--scan-seconds", "0"],
    ));
    assert_eq!(
        first,
        format!(
            "Lore\n\
             Vault      created {}\n\
             Archive    no new sessions from the last 30d in N s; 4 more files waiting, \
             `lorectl scan --since 30d` reads them\n\
             Lessons    none found in your sessions yet.\n\n{NEXT}",
            vault.display()
        )
    );
    // The next run picks the waiting files up.
    let second = masked(&lorectl(home, &["init", "--no-review"]));
    assert!(
        second.contains("Archive    scanned 4 new sessions from the last 30d in N s\n"),
        "{second}"
    );
}

#[test]
fn init_accepts_vault_flag_before_the_command() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path();
    let custom_vault = home.join("custom-日本語-vault");
    let out = masked(&lorectl(
        home,
        &[
            "--vault",
            custom_vault.to_str().unwrap(),
            "init",
            "--no-review",
        ],
    ));
    assert!(
        out.contains(&format!("Vault      created {}", custom_vault.display())),
        "{out}"
    );
    assert!(custom_vault.exists());
}

#[test]
fn vault_flag_is_refused_for_commands_that_do_not_take_it() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path();
    let o = lorectl(home, &["--vault", "/tmp/v", "status"]);
    assert_eq!(o.status.code(), Some(1));
    let err = String::from_utf8_lossy(&o.stderr);
    assert!(
        err.contains("--vault works only with `lorectl init`, `lorectl mem`, and `lorectl report`"),
        "{err}"
    );
}

#[test]
fn init_accepts_json_flag_on_either_side_of_the_command() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path();
    let before_help = lorectl(home, &["--json", "init", "--help"]);
    assert_eq!(before_help.status.code(), Some(0));
    let after_help = lorectl(home, &["init", "--json", "--help"]);
    assert_eq!(after_help.status.code(), Some(0));
    assert_eq!(
        String::from_utf8_lossy(&before_help.stdout),
        String::from_utf8_lossy(&after_help.stdout)
    );

    let out = masked(&lorectl(home, &["init", "--json", "--no-review"]));
    assert!(out.contains("Lore\nVault      created"), "{out}");
}
