//! A redirect changes the task at hand ("no i want you to give me X
//! instead"); it is not a lesson for future work. A lesson states a rule: it
//! names a tool, command, file, or path, or says always, never, don't, stop,
//! or use X not Y.
#![allow(clippy::expect_used, clippy::unwrap_used)]

#[path = "common/archive.rs"]
mod archive;

use lore_memory::mine::{corrections, lessons, states_rule};

#[test]
fn redirects_said_in_many_sessions_are_not_lessons() {
    let dir = tempfile::tempdir().unwrap();
    let redirect: &[(&str, &str)] = &[
        ("user", "summarize the thread"),
        ("assistant", "Here is a long summary."),
        ("user", "no i want you to give me the short summary instead"),
        ("assistant", "Here is a list."),
        ("user", "no, give me a table instead of a list"),
    ];
    let rule: &[(&str, &str)] = &[
        ("user", "run the script"),
        ("assistant", "Running python3 build.py."),
        ("user", "no, run it with uv run not python3"),
    ];
    let conn = archive::codex_archive(dir.path(), &[redirect, redirect, rule, rule]);
    assert_eq!(
        corrections(&conn).unwrap().len(),
        6,
        "all six still are candidates"
    );
    let texts: Vec<(String, usize)> = lessons(corrections(&conn).unwrap())
        .into_iter()
        .map(|l| (l.text, l.sessions))
        .collect();
    assert_eq!(
        texts,
        [("no, run it with uv run not python3".to_owned(), 2)]
    );
}

#[test]
fn a_rule_names_a_tool_file_or_path_or_says_always_never_dont_stop() {
    for rule in [
        "no, use pnpm not npm",
        "always run the formatter before you commit",
        "never commit on main",
        "don't touch the lockfile",
        "stop editing src/generated/schema.rs",
        "no, the config lives in config/app.toml",
        "wrong flag, pass --locked",
        "use tabs instead",
        "follow the repo's naming convention",
    ] {
        assert!(states_rule(rule), "{rule}");
    }
    for redirect in [
        "no i want you to give me the short summary instead",
        "no, give me a table instead of a list",
        "no, the other one",
        "actually make it shorter",
        "wrong answer, try again",
    ] {
        assert!(!states_rule(redirect), "{redirect}");
    }
}
