//! The note's value types on their own, until the parser tests cover them.
#![allow(clippy::expect_used, clippy::unwrap_used)]

use lore_memory::note::{LineRange, NoteDate, RelPath, RepoKey, RepoName};

#[test]
fn types_keep_valid_values_and_refuse_the_rest() {
    assert_eq!(RepoKey::parse("3f9a1c0b").unwrap().as_str(), "3f9a1c0b");
    for key in ["3F9A1C0B", "3f9a1c0", "3f9a1c0g"] {
        assert!(RepoKey::parse(key).is_err(), "{key}");
    }
    assert_eq!(RepoName::parse("my repo").unwrap().as_str(), "my repo");
    for name in ["", " lore", "a\nb"] {
        assert!(RepoName::parse(name).is_err(), "{name:?}");
    }
    assert_eq!(RelPath::parse("src/lib.rs").unwrap().as_str(), "src/lib.rs");
    for path in ["", "/etc/passwd", "../x", "a//b", "./a", "a\\b", "C:/x"] {
        assert!(RelPath::parse(path).is_err(), "{path}");
    }
    let lines = LineRange::parse("210-224").unwrap();
    assert_eq!((lines.start(), lines.end()), (210, 224));
    assert_eq!(lines.to_string(), "210-224");
    assert_eq!(LineRange::parse("7").unwrap().to_string(), "7");
    for range in ["0", "9-3", "+1", "a-b"] {
        assert!(LineRange::parse(range).is_err(), "{range}");
    }
    assert_eq!(
        NoteDate::parse("2028-02-29").unwrap().to_string(),
        "2028-02-29"
    );
    for date in ["2026-02-30", "2026-13-01", "2026-1-01", "yesterday"] {
        assert!(NoteDate::parse(date).is_err(), "{date}");
    }
    assert_eq!(NoteDate::today().to_string().len(), 10);
}
