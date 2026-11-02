//! The note's value types on their own, until the parser tests cover them.
#![allow(clippy::expect_used, clippy::unwrap_used)]

use lore_memory::note::{
    Anchor, CommitId, LineHash, LineRange, Note, NoteDate, NoteId, NoteType, RelPath, Relation,
    RepoKey, RepoName, Scope, SourceRef, Status,
};

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

#[test]
fn types_a_note_holds_anchors_relations_and_sources() {
    for note_type in NoteType::ALL {
        assert_eq!(NoteType::parse(note_type.as_str()).unwrap(), note_type);
    }
    assert!(NoteType::parse("idea").is_err());
    for scope in ["global", "repo:3f9a1c0b"] {
        assert_eq!(Scope::parse(scope).unwrap().to_string(), scope);
    }
    assert!(Scope::parse("repo:XYZ").is_err());
    assert_eq!(Status::parse("retired").unwrap().as_str(), "retired");
    assert!(Status::parse("done").is_err());
    let id = "01JC9V6Q4M8K2T7X3N5R0B1H01";
    let relation = Relation::parse("updates", id).unwrap();
    assert_eq!(
        (relation.kind(), relation.target().as_str()),
        ("updates", id)
    );
    assert!(Relation::parse("replaces", id).is_err());
    let source = SourceRef::parse("claude-code:7d2f9a1e").unwrap();
    assert_eq!(source.to_string(), "claude-code:7d2f9a1e");
    assert!(SourceRef::parse("slack:abc").is_err());
    assert!(SourceRef::parse("codex:a b").is_err());
    assert!(LineHash::parse("b3:9C1E04AA").is_err());
    assert!(CommitId::parse("067ef8").is_err());
    let note = Note {
        id: NoteId::parse(id).unwrap(),
        note_type: NoteType::Pitfall,
        title: "T".into(),
        scope: Scope::Global,
        repo: None,
        status: Status::Active,
        anchors: vec![Anchor {
            path: RelPath::parse("src/git.rs").unwrap(),
            lines: Some(LineRange::new(210, 224).unwrap()),
            hash: Some(LineHash::parse("b3:9c1e04aa").unwrap()),
            commit: Some(CommitId::parse("067ef8d").unwrap()),
        }],
        relations: vec![relation],
        sources: vec![source],
        created: NoteDate::parse("2026-11-01").unwrap(),
        forget_after: None,
        body: String::new(),
    };
    assert_eq!(
        note.anchors[0].hash.as_ref().unwrap().as_str(),
        "b3:9c1e04aa"
    );
    assert_eq!(note.anchors[0].commit.as_ref().unwrap().as_str(), "067ef8d");
}
