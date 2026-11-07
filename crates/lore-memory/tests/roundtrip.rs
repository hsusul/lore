//! Parse then render is byte-equal for every fixture note, and bad input is
//! refused with the field it came from.
#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::path::{Path, PathBuf};

use lore_memory::note::{Note, NoteDate, NoteError, NoteId, NoteType, RelPath, Scope};

fn fixture_vault() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/vault")
}

fn fixtures() -> Vec<(String, String)> {
    fn walk(dir: &Path, base: &Path, out: &mut Vec<(String, String)>) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                walk(&path, base, out);
            } else if path.extension().is_some_and(|e| e == "md") {
                let rel = path
                    .strip_prefix(base)
                    .unwrap()
                    .to_string_lossy()
                    .into_owned();
                out.push((rel, std::fs::read_to_string(&path).unwrap()));
            }
        }
    }
    let mut out = Vec::new();
    walk(&fixture_vault(), &fixture_vault(), &mut out);
    out.sort();
    out
}

#[test]
fn roundtrip_every_fixture_is_byte_equal() {
    let fixtures = fixtures();
    assert_eq!(fixtures.len(), 21);
    for (rel, text) in &fixtures {
        let note = Note::parse(text).unwrap_or_else(|e| panic!("{rel}: {e}"));
        assert_eq!(&note.render(), text, "{rel} did not round-trip");
        assert_eq!(&note.rel_path(), rel, "{rel} is not where its slug puts it");
    }
}

#[test]
fn roundtrip_fixtures_cover_every_type_four_times() {
    let notes: Vec<Note> = fixtures()
        .iter()
        .map(|(_, text)| Note::parse(text).unwrap())
        .collect();
    for note_type in NoteType::ALL {
        let count = notes.iter().filter(|n| n.note_type == note_type).count();
        assert!(count >= 4, "{} has {count} fixtures", note_type.as_str());
    }
}

#[test]
fn roundtrip_keeps_unknown_keys_in_place() {
    let text = std::fs::read_to_string(
        fixture_vault().join("global/obsidian-keys-survive-a-round-trip.md"),
    )
    .unwrap();
    let note = Note::parse(&text).unwrap();
    assert_eq!(
        note.extra.unknown_keys(),
        ["aliases", "cssclasses", "plugin", "publish", "tags"]
    );
    assert_eq!(note.render(), text);
}

#[test]
fn roundtrip_reads_the_design_sample_fields() {
    let text = std::fs::read_to_string(
        fixture_vault().join("repos/3f9a1c0b5e2d7a46/reference-id-panics-on-symbolic-refs.md"),
    )
    .unwrap();
    let note = Note::parse(&text).unwrap();
    assert_eq!(note.id.as_str(), "01JC9V6Q4M8K2T7X3N5R0B1H17");
    assert_eq!(note.scope.to_string(), "repo:3f9a1c0b5e2d7a46");
    let anchor = &note.anchors[0];
    assert_eq!(anchor.path.as_str(), "crates/lore-core/src/git.rs");
    let lines = anchor.lines.unwrap();
    assert_eq!((lines.start(), lines.end()), (210, 224));
    assert_eq!(anchor.hash.as_ref().unwrap().as_str(), "b3:9c1e04aa");
    assert_eq!(anchor.commit.as_ref().unwrap().as_str(), "067ef8d");
    assert_eq!(note.relations[0].kind(), "updates");
    assert_eq!(note.sources[0].to_string(), "claude-code:7d2f9a1e");
    assert_eq!(note.forget_after.unwrap().to_string(), "2027-05-01");
    assert_eq!(
        note.body,
        "Peel a symbolic ref before you call `id()` on it.\n\n## Timeline\n- 2026-11-01 accepted from claude-code:7d2f9a1e\n"
    );
}

const DRAFT: &str = "---\ntype: howto\ntitle: Prune worktrees\nmood: calm\n---\nRun it.\n";

#[test]
fn roundtrip_a_draft_gets_its_missing_keys_in_canonical_places() {
    let id = NoteId::parse("01JC9V6Q4M8K2T7X3N5R0B1HZZ").unwrap();
    let today = NoteDate::parse("2026-11-01").unwrap();
    let draft = Note::parse_draft(DRAFT, id, today).unwrap();
    assert!(draft.id_filled && draft.created_filled);
    assert_eq!(
        draft.note.render(),
        "---\nid: 01JC9V6Q4M8K2T7X3N5R0B1HZZ\ntype: howto\ntitle: Prune worktrees\naliases:\n  - Prune worktrees\nscope: global\nstatus: active\ncreated: 2026-11-01\nmood: calm\n---\nRun it.\n"
    );
    assert_eq!(draft.note.rel_path(), "global/prune-worktrees.md");
}

#[test]
fn roundtrip_quotes_titles_yaml_would_misread() {
    let id = NoteId::parse("01JC9V6Q4M8K2T7X3N5R0B1HZZ").unwrap();
    let today = NoteDate::parse("2026-11-01").unwrap();
    for (title, written) in [
        ("plain words", "plain words"),
        ("a: b", "\"a: b\""),
        ("yes", "\"yes\""),
        ("2026", "\"2026\""),
        ("#tag", "\"#tag\""),
        ("say \"hi\"", "say \"hi\""),
        ("- dash", "\"- dash\""),
        ("back\\slash: x", "\"back\\\\slash: x\""),
    ] {
        let text = format!("---\ntype: fact\ntitle: {written}\n---\n");
        let note = Note::parse_draft(&text, id.clone(), today).unwrap().note;
        assert_eq!(note.title, title, "reading {written}");
        assert!(
            note.render().contains(&format!("\ntitle: {written}\n")),
            "{title}"
        );
    }
}

fn parse_error(text: &str) -> NoteError {
    Note::parse(text).expect_err("must be refused")
}

fn with(line: &str) -> String {
    format!(
        "---\nid: 01JC9V6Q4M8K2T7X3N5R0B1H01\ntype: fact\ntitle: T\nscope: global\nstatus: active\ncreated: 2026-11-01\n{line}\n---\nbody\n"
    )
}

#[test]
fn roundtrip_a_repo_key_is_exactly_16_hex() {
    let long = "3f9a1c0b5e2d7a46".repeat(4);
    for key in [
        "3f9a1c0b",
        "3f9a1c0b5e2d7a4",
        "3f9a1c0b5e2d7a461",
        "3F9A1C0B5E2D7A46",
        long.as_str(),
    ] {
        let error = Scope::parse(&format!("repo:{key}")).expect_err(key);
        assert!(error.to_string().contains("16 lowercase hex"), "{error}");
    }
    let scope = Scope::parse("repo:3f9a1c0b5e2d7a46").unwrap();
    assert_eq!(scope.to_string(), "repo:3f9a1c0b5e2d7a46");
}

#[test]
fn roundtrip_refuses_bad_paths_by_field() {
    for path in [
        "/etc/passwd",
        "../outside",
        "a/../../b",
        "C:/x",
        "a//b",
        "./a",
        "a\\b",
    ] {
        let text = with(&format!(
            "anchors:\n  - path: \"{}\"",
            path.replace('\\', "\\\\")
        ));
        match parse_error(&text) {
            NoteError::Field { field, .. } => assert_eq!(field, "anchors[0].path", "{path}"),
            other => panic!("{path}: {other:?}"),
        }
        assert!(RelPath::parse(path).is_err(), "{path}");
    }
    assert_eq!(RelPath::parse("src/lib.rs").unwrap().as_str(), "src/lib.rs");
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
        (&with("anchors:\n  - path: a\n    color: red"), "`color`"),
        (&with("anchors: [a]"), "one `- ` item per line"),
        (
            &with("relations:\n  - replaces: 01JC9V6Q4M8K2T7X3N5R0B1H01"),
            "updates, extends",
        ),
        (&with("sources:\n  - slack:abc"), "claude-code or codex"),
        (&with("just words"), "not `key: value`"),
        (&with("forget_after: soon"), "`forget_after`"),
        (&with("enforce: bash"), "`enforce`: write the list"),
    ];
    for (text, expected) in cases {
        let error = parse_error(text).to_string();
        assert!(error.contains(expected), "{text:?} gave {error}");
    }
}

#[test]
fn an_enforce_rule_round_trips_and_compiles_once() {
    let text = with(
        "enforce:\n  - tool: bash\n    pattern: '(?i)co-authored-by:\\s*claude'\n    unless: 'it''s ok'\n  - tool: path\n    pattern: '(^|/)PLAN\\.md$'",
    );
    let note = Note::parse(&text).unwrap();
    assert_eq!(note.render(), text);
    let rule = &note.enforce[0];
    assert_eq!(rule.pattern.as_str(), r"(?i)co-authored-by:\s*claude");
    assert_eq!(rule.unless.as_ref().unwrap().as_str(), "it's ok");
    assert_eq!(
        rule.pattern.find("x Co-Authored-By: Claude"),
        Some("Co-Authored-By: Claude")
    );
    // A plain pattern parses too, and renders single-quoted.
    let plain = Note::parse(&with("enforce:\n  - tool: write\n    pattern: XXX")).unwrap();
    assert!(plain.render().contains("    pattern: 'XXX'\n"));
}

#[test]
fn a_bad_enforce_rule_is_dropped_and_the_note_stays_valid() {
    let long = "a".repeat(lore_memory::enforce::PATTERN_MAX_LEN + 1);
    let cases = [
        (
            "tool: bash\n    pattern: '(unclosed'".to_string(),
            "`enforce[0].pattern`: `(unclosed` is not a valid regex",
        ),
        (
            "tool: bash\n    pattern: 'a*'".to_string(),
            "matches empty text",
        ),
        (
            format!("tool: bash\n    pattern: '{long}'"),
            "`enforce[0].pattern`: the pattern is longer than",
        ),
        (
            "tool: shell\n    pattern: x".to_string(),
            "`enforce[0].tool`: expected bash, write, or path",
        ),
        ("pattern: x".to_string(), "`enforce[0]`: needs a `tool`"),
    ];
    for (rule, expected) in cases {
        let text = with(&format!(
            "enforce:\n  - {rule}\n  - tool: path\n    pattern: 'PLAN'"
        ));
        let note = Note::parse(&text).unwrap_or_else(|e| panic!("{rule}: {e}"));
        assert_eq!(note.enforce.len(), 1, "{rule}");
        assert_eq!(note.enforce[0].pattern.as_str(), "PLAN");
        assert_eq!(note.enforce_errors.len(), 1, "{rule}");
        assert_eq!(note.enforce_errors[0].index, 0);
        let error = note.enforce_errors[0].to_string();
        assert!(error.contains(expected), "{rule}: {error}");
        // The bad rule is kept as written, in place, for a person to fix.
        assert_eq!(note.render(), text, "{rule}");
    }

    // Rules past the limit are dropped the same way.
    let rules: String = (0..=lore_memory::enforce::MAX_RULES)
        .map(|i| format!("\n  - tool: bash\n    pattern: 'x{i}'"))
        .collect();
    let note = Note::parse(&with(&format!("enforce:{rules}"))).unwrap();
    assert_eq!(note.enforce.len(), lore_memory::enforce::MAX_RULES);
    let error = note.enforce_errors[0].to_string();
    assert!(error.contains("at most 8 rules"), "{error}");
}

const BASE: &str = "---\nid: 01JC9V6Q4M8K2T7X3N5R0B1H6D\ntype: fact\ntitle: T\nscope: global\nstatus: active\ncreated: 2026-11-01\n";

fn draft(text: &str) -> Note {
    Note::parse_draft(
        text,
        NoteId::parse("01JC9V6Q4M8K2T7X3N5R0B1H6D").unwrap(),
        NoteDate::parse("2026-11-01").unwrap(),
    )
    .unwrap()
    .note
}

#[test]
fn roundtrip_refuses_trailing_comments_on_lore_keys() {
    for line in [
        "type: fact # a comment",
        "status: active  # retired soon",
        "title: \"T\" # quoted then comment",
        "created: 2026-11-01 # today",
        "sources:\n  - codex:abc # from codex",
    ] {
        let text = BASE
            .lines()
            .filter(|l| !line.starts_with(l.split(':').next().unwrap()) || l.starts_with("---"))
            .map(|l| format!("{l}\n"))
            .collect::<String>()
            + line
            + "\n---\n";
        let error = Note::parse(&text).unwrap_err().to_string();
        assert!(error.contains("comment"), "{line:?} gave {error}");
    }
    // A comment on a key Lore does not own is that key's business.
    let text = format!("{BASE}mood: calm # very\n---\n");
    assert_eq!(Note::parse(&text).unwrap().render(), text);
}

#[test]
fn roundtrip_keeps_empty_forget_after_and_empty_lists_as_written() {
    for extra in ["forget_after:\n", "anchors:\n", "sources: []\n"] {
        let text = format!("{BASE}{extra}---\nBody.\n");
        let note = Note::parse(&text).unwrap();
        assert_eq!(note.forget_after, None);
        assert_eq!(note.render(), text, "{extra:?}");
    }
    let mut note = Note::parse(&format!("{BASE}forget_after:\n---\n")).unwrap();
    note.forget_after = Some(NoteDate::parse("2027-05-01").unwrap());
    assert_eq!(
        note.render(),
        format!("{BASE}forget_after: 2027-05-01\n---\n")
    );
}

#[test]
fn roundtrip_quotes_yaml_ambiguous_titles() {
    for title in [
        "2026-11-01",
        "42",
        "0x10",
        "0o17",
        ".inf",
        "-.inf",
        ".NaN",
        "12:30",
        "1_000",
        "+1",
        "yes",
        "No",
        "on",
        "OFF",
        "null",
        "~",
        "@mention",
        "`code`",
        "1e3",
    ] {
        let mut note = Note::parse(&format!("{BASE}---\n")).unwrap();
        note.title = title.to_string();
        let text = note.render();
        let line = text.lines().find(|l| l.starts_with("title: ")).unwrap();
        assert!(line.starts_with("title: \""), "{title} rendered as {line}");
        assert_eq!(Note::parse(&text).unwrap().title, title);
    }
    let mut note = Note::parse(&format!("{BASE}---\n")).unwrap();
    note.title = "Plain words stay plain".into();
    assert!(note.render().contains("\ntitle: Plain words stay plain\n"));
}

#[test]
fn roundtrip_slugs_keep_unicode_letters_and_digits() {
    let slug = |title: &str| draft(&format!("---\ntype: fact\ntitle: {title}\n---\n")).slug();
    assert_eq!(
        slug("Café naïve — 日本語 round trip"),
        "café-naïve-日本語-round-trip"
    );
    assert_eq!(slug("Ünïcode ÉDGE 42"), "ünïcode-édge-42");
    assert_eq!(slug("日本語"), "日本語");
    assert_eq!(slug("— !!! —"), "01jc9v6q4m8k2t7x3n5r0b1h6d");
}

#[test]
fn roundtrip_a_draft_gets_its_title_as_an_alias() {
    let note = draft("---\ntype: fact\ntitle: \"Reference::id panics\"\n---\nB.\n");
    let dated = draft("---\ntype: fact\ntitle: \"2026-11-01 retro\"\n---\n").render();
    assert!(
        dated.contains("\naliases:\n  - \"2026-11-01 retro\"\n"),
        "{dated}"
    );
    let text = note.render();
    assert!(
        text.contains("\ntitle: Reference::id panics\naliases:\n  - Reference::id panics\n"),
        "{text}"
    );
    assert_eq!(Note::parse(&text).unwrap().render(), text);
    // A draft that brings its own aliases keeps them untouched.
    let own = draft("---\ntype: fact\ntitle: T\naliases: [x]\n---\n").render();
    assert!(
        own.contains("\naliases: [x]\n") && !own.contains("  - T"),
        "{own}"
    );
}

#[test]
fn roundtrip_repo_notes_live_in_a_folder_named_for_people() {
    let note = draft("---\ntype: fact\ntitle: T\nscope: repo:3f9a1c0b5e2d7a46\nrepo: lore\n---\n");
    assert_eq!(note.repo.as_ref().unwrap().as_str(), "lore");
    assert_eq!(note.rel_path(), "repos/lore-3f9a1c0b5e2d7a46/t.md");
    assert!(note
        .render()
        .contains("\nscope: repo:3f9a1c0b5e2d7a46\nrepo: lore\n"));
    let bare = draft("---\ntype: fact\ntitle: T\nscope: repo:3f9a1c0b5e2d7a46\n---\n");
    assert_eq!(bare.rel_path(), "repos/3f9a1c0b5e2d7a46/t.md");
    use lore_memory::note::RepoName;
    for (origin, name) in [
        ("git@github.com:hsusul/lore.git", "lore"),
        ("https://github.com/hsusul/Lore/", "Lore"),
        ("/srv/git/my repo.git", "my repo"),
        ("git@github.com:hsusul/lore.GIT", "lore"),
        ("https://github.com/hsusul/lore.Git/", "lore"),
        ("/srv/git/日本", "日本"),
        ("/srv/git/日本.git", "日本"),
    ] {
        assert_eq!(RepoName::from_origin(origin).unwrap().as_str(), name);
    }
    assert_eq!(RepoName::parse("My Repo").unwrap().slug(), "my-repo");
    assert!(RepoName::from_origin("").is_none());
    assert!(RepoName::from_origin("https://github.com/hsusul/.git").is_none());
    assert!(RepoName::from_origin("https://github.com/hsusul/.GIT").is_none());
}

#[test]
fn roundtrip_generated_ids_are_valid_and_distinct() {
    let a = NoteId::generate();
    let b = NoteId::generate();
    assert_ne!(a, b);
    assert!(NoteId::parse(a.as_str()).is_ok(), "{}", a.as_str());
    assert!(a.as_str().starts_with('0'), "{}", a.as_str());
}
