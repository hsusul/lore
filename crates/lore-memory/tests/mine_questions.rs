//! A question to the agent is not a correction, even when it says "no" or
//! "wait". Only turns that tell the agent what to do differently count.
#![allow(clippy::expect_used, clippy::unwrap_used)]

#[path = "common/archive.rs"]
mod archive;

use lore_memory::mine::corrections;

#[test]
fn questions_and_a_bare_wait_are_not_corrections() {
    let dir = tempfile::tempdir().unwrap();
    let turns: Vec<(&str, &str)> = [
        "wait so what does the hook print?",
        "wait but it said three files",
        "why is the build failing",
        "how do I run it locally",
        "what does stop do here",
        "can you check the logs instead",
        "is it wrong to use npm here",
        "no, did that actually work?",
        "wait, open the other file",
        "no, use pnpm not npm",
    ]
    .iter()
    .flat_map(|user| [("assistant", "Ran the build."), ("user", *user)])
    .collect();
    let mut session = vec![("user", "fix the build")];
    session.extend(turns);
    let conn = archive::codex_archive(dir.path(), &[&session]);
    let texts: Vec<String> = corrections(&conn)
        .unwrap()
        .into_iter()
        .map(|c| c.text)
        .collect();
    assert_eq!(texts, ["no, use pnpm not npm"]);
}

#[test]
fn a_question_that_insists_is_a_correction() {
    let dir = tempfile::tempdir().unwrap();
    let turns: Vec<(&str, &str)> = [
        "why do you always skip the slow tests?",
        "can you not add comments, i dont want them",
        "never mind, what does the flag do?",
    ]
    .iter()
    .flat_map(|user| [("assistant", "Ran the build."), ("user", *user)])
    .collect();
    let mut session = vec![("user", "fix the build")];
    session.extend(turns);
    let conn = archive::codex_archive(dir.path(), &[&session]);
    let texts: Vec<String> = corrections(&conn)
        .unwrap()
        .into_iter()
        .map(|c| c.text)
        .collect();
    assert_eq!(
        texts,
        [
            "why do you always skip the slow tests?",
            "can you not add comments, i dont want them",
        ]
    );
}
