//! What a reply receipt shows around the person's words: the agent text they
//! answered and the agent text that answered them, both from the reply's own
//! session, and which replies are only go-aheads a lessons run need not read.
#![allow(clippy::expect_used, clippy::unwrap_used)]

#[path = "common/archive.rs"]
mod archive;

use lore_memory::mine::{is_approval, read_turns, replies_in, Candidate, Limits};

fn at(s: u32) -> String {
    format!("2026-08-03T10:00:{s:02}.000Z")
}

fn reply<'a>(found: &'a [Candidate], text: &str) -> &'a Candidate {
    found
        .iter()
        .find(|c| c.text == text)
        .expect("the reply is listed")
}

#[test]
fn a_reply_after_tool_only_agent_turns_gets_the_last_agent_text() {
    let dir = tempfile::tempdir().unwrap();
    let ts: Vec<String> = (0..8).map(at).collect();
    let turns: Vec<archive::Turn<'_>> = vec![
        ("user", "set up the build", &ts[0]),
        ("assistant", "I set up the build with npm.", &ts[1]),
        // Agent turns with only a tool call and no text.
        ("skill", "build", &ts[2]),
        ("user", "use the lockfile we have", &ts[3]),
        ("skill", "lockfile", &ts[4]),
        ("user", "and pin the versions", &ts[5]),
        ("skill", "pin", &ts[6]),
        (
            "assistant",
            "Pinned them and switched to the lockfile.",
            &ts[7],
        ),
    ];
    let conn = archive::claude_archive(
        dir.path(),
        &[("33333333-3333-4333-8333-333333333333", &turns)],
    );
    let found = replies_in(&read_turns(&conn).unwrap(), &Limits::default(), i64::MIN);
    for text in ["use the lockfile we have", "and pin the versions"] {
        let r = reply(&found, text);
        assert_eq!(
            r.preceding.as_deref(),
            Some("I set up the build with npm."),
            "{text}"
        );
        assert_eq!(
            r.following.as_deref(),
            Some("Pinned them and switched to the lockfile."),
            "{text}"
        );
    }
}

#[test]
fn before_and_after_never_come_from_another_session() {
    let dir = tempfile::tempdir().unwrap();
    let ts: Vec<String> = (0..3).map(at).collect();
    // One session ends on the person's reply and opens with their words; the
    // other opens and ends with the agent's. In archive order one sits right
    // after the other, whichever it is.
    let ends_on_reply: Vec<archive::Turn<'_>> = vec![
        ("user", "rename the parser module", &ts[0]),
        ("assistant", "Renamed it to reader.", &ts[1]),
        ("user", "call it lexer after all", &ts[2]),
    ];
    let agent_at_edges: Vec<archive::Turn<'_>> = vec![
        ("assistant", "Opening words of the other session.", &ts[0]),
        ("user", "keep the old header", &ts[1]),
        ("assistant", "Closing words of the other session.", &ts[2]),
    ];
    let conn = archive::claude_archive(
        dir.path(),
        &[
            ("44444444-4444-4444-8444-444444444444", &ends_on_reply),
            ("55555555-5555-4555-8555-555555555555", &agent_at_edges),
        ],
    );
    let found = replies_in(&read_turns(&conn).unwrap(), &Limits::default(), i64::MIN);
    let last = reply(&found, "call it lexer after all");
    assert_eq!(last.preceding.as_deref(), Some("Renamed it to reader."));
    assert_eq!(last.following, None);
    let other = reply(&found, "keep the old header");
    assert_eq!(
        other.preceding.as_deref(),
        Some("Opening words of the other session.")
    );
    assert_eq!(
        other.following.as_deref(),
        Some("Closing words of the other session.")
    );
    assert_ne!(last.session, other.session);
    // A session's first words answer no agent turn of its own.
    assert!(found.iter().all(|c| c.text != "rename the parser module"));
}

#[test]
fn go_aheads_are_not_replies_but_qualified_ones_are() {
    for approval in [
        "Yes, merge them",
        "Yes, commit",
        "Push and reply",
        "Try again",
        "Not now",
        "ok",
        "continue",
        "Yes, open both PRs",
        "go",
        "go ahead",
        "sounds good, thanks",
        "LGTM!",
    ] {
        assert!(is_approval(approval), "{approval}");
    }
    for kept in [
        "yes but don't push",
        "yes, but use pnpm",
        "yes, only the first one",
        "merge, not squash",
        "ok why did that fail",
        "yes, commit?",
        "try again with the other key",
        "go back to the old layout",
        "yes, also bump the version",
        "Referral stack",
        "yes please make sure the tests run first",
        "yes, merge them and then rebuild the docs site tonight",
        "",
    ] {
        assert!(!is_approval(kept), "{kept}");
    }

    let dir = tempfile::tempdir().unwrap();
    let ts: Vec<String> = (0..5).map(at).collect();
    let turns: Vec<archive::Turn<'_>> = vec![
        ("user", "land the stack", &ts[0]),
        ("assistant", "Ready to merge both branches.", &ts[1]),
        ("user", "Yes, merge them", &ts[2]),
        ("assistant", "Merged. Shall I tag a release?", &ts[3]),
        ("user", "tag it as a beta", &ts[4]),
    ];
    let conn = archive::claude_archive(
        dir.path(),
        &[("66666666-6666-4666-8666-666666666666", &turns)],
    );
    let found = replies_in(&read_turns(&conn).unwrap(), &Limits::default(), i64::MIN);
    let texts: Vec<&str> = found.iter().map(|c| c.text.as_str()).collect();
    assert_eq!(texts, ["tag it as a beta"]);
}
