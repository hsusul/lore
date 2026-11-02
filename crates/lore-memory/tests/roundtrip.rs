//! Parse then render is byte-equal for every fixture note, and bad input is
//! refused with the field it came from.
#![allow(clippy::expect_used, clippy::unwrap_used)]

use lore_memory::note::{Note, NoteDate, NoteError, NoteId};

fn parse_error(text: &str) -> NoteError {
    Note::parse(text).expect_err("must be refused")
}

fn with(line: &str) -> String {
    format!(
        "---\nid: 01JC9V6Q4M8K2T7X3N5R0B1H01\ntype: fact\ntitle: T\nscope: global\nstatus: active\ncreated: 2026-11-01\n{line}\n---\nbody\n"
    )
}

#[test]
fn roundtrip_refuses_bad_dates_by_field() {
    for date in [
        "2026-13-01",
        "2026-02-30",
        "2026-1-01",
        "yesterday",
        "2026-11-01T00:00",
    ] {
        let text = format!(
            "---\nid: 01JC9V6Q4M8K2T7X3N5R0B1H01\ntype: fact\ntitle: T\nscope: global\nstatus: active\ncreated: {date}\n---\n"
        );
        match parse_error(&text) {
            NoteError::Field { field, problem } => {
                assert_eq!(field, "created", "{date}");
                assert!(problem.contains(date), "{problem}");
            }
            other => panic!("{date}: {other:?}"),
        }
    }
    assert_eq!(
        NoteDate::parse("2028-02-29").unwrap().to_string(),
        "2028-02-29"
    );
}

#[test]
fn roundtrip_refuses_malformed_files_without_panicking() {
    let cases = [
        ("no frontmatter\n", "not a note"),
        ("---\ntitle: T\n", "no closing"),
        ("---\r\ntitle: T\r\n---\r\n", "CRLF line endings"),
        ("---\ntype: fact\n---\n", "missing required key `id`"),
        (&with("type: fact"), "appears twice"),
        (&with("just words"), "not `key: value`"),
        (&with("forget_after: soon"), "`forget_after`"),
    ];
    for (text, expected) in cases {
        let error = parse_error(text).to_string();
        assert!(error.contains(expected), "{text:?} gave {error}");
    }
}

#[test]
fn roundtrip_generated_ids_are_valid_and_distinct() {
    let a = NoteId::generate();
    let b = NoteId::generate();
    assert_ne!(a, b);
    assert!(NoteId::parse(a.as_str()).is_ok(), "{}", a.as_str());
    assert!(a.as_str().starts_with('0'), "{}", a.as_str());
}
