//! A session a program started is not the person talking: a `codex exec`
//! run, a Codex subagent thread, and a `claude -p` run (Lore's own Find
//! lessons runs among them) hold prompts a program wrote, so none of their
//! turns is mined, even one that states a rule.
#![allow(clippy::expect_used, clippy::unwrap_used)]

#[path = "common/archive.rs"]
mod archive;

use lore_memory::mine::{corrections, read_turns, replies_in, Limits};
use serde_json::json;

const RULE: &str = "Make sure you always run tests";

#[test]
fn codex_exec_and_subagent_sessions_are_not_mined() {
    let dir = tempfile::tempdir().unwrap();
    let turns: &[(&str, &str)] = &[
        ("user", RULE),
        ("assistant", "Ran them."),
        ("user", "no, that's wrong, run them again"),
    ];
    let conn = archive::codex_archive_with(
        dir.path(),
        &[
            (json!({"source": "exec", "originator": "codex_exec"}), turns),
            (
                json!({"source": {"subagent": {"thread_spawn": {"depth": 1}}}}),
                turns,
            ),
            (json!({"originator": "codex_exec"}), turns),
            (
                json!({"source": "cli", "originator": "codex_cli_rs"}),
                turns,
            ),
        ],
    );
    let interactive = archive::session_id(&conn, 4);
    let found = corrections(&conn).unwrap();
    assert_eq!(found.len(), 2, "{found:?}");
    assert!(found.iter().all(|c| c.session == interactive), "{found:?}");
    let replies = replies_in(&read_turns(&conn).unwrap(), &Limits::default(), 0);
    assert!(
        replies.iter().all(|c| c.session == interactive),
        "{replies:?}"
    );
}

#[test]
fn claude_print_mode_sessions_are_not_mined() {
    let dir = tempfile::tempdir().unwrap();
    let line = |sid: &str, n: usize, role: &str, text: &str, entrypoint: &str| {
        let message = if role == "user" {
            json!({"role": "user", "content": text})
        } else {
            json!({"role": "assistant", "model": "claude-x",
                "content": [{"type": "text", "text": text}]})
        };
        json!({
            "type": role, "uuid": format!("{}-{n:04}", &sid[..30]),
            "parentUuid": (n > 0).then(|| format!("{}-{:04}", &sid[..30], n - 1)),
            "sessionId": sid, "timestamp": format!("2026-08-01T10:00:{n:02}.000Z"),
            "cwd": "/repo/app", "version": "2.1.0", "entrypoint": entrypoint,
            "isSidechain": false, "userType": "external", "message": message,
        })
    };
    let session = |sid: &str, entrypoint: &str| {
        vec![
            line(sid, 0, "user", RULE, entrypoint),
            line(sid, 1, "assistant", "Ran them.", entrypoint),
            line(
                sid,
                2,
                "user",
                "no, that's wrong, run them again",
                entrypoint,
            ),
        ]
    };
    for (sid, entrypoint) in [
        ("11111111-0000-4000-8000-000000000001", "sdk-cli"),
        ("11111111-0000-4000-8000-000000000002", "sdk-ts"),
    ] {
        let conn = archive::claude_raw_archive(dir.path(), sid, &session(sid, entrypoint));
        let found = corrections(&conn).unwrap();
        assert!(found.is_empty(), "{entrypoint}: {found:?}");
    }
    let sid = "11111111-0000-4000-8000-000000000003";
    let conn = archive::claude_raw_archive(dir.path(), sid, &session(sid, "cli"));
    // The earlier transcripts are still in the folder: only this one counts.
    let found = corrections(&conn).unwrap();
    let interactive = archive::claude_session_id(&conn, sid);
    assert_eq!(found.len(), 2, "{found:?}");
    assert!(found.iter().all(|c| c.session == interactive), "{found:?}");
}
