//! Message bodies stored zstd-compressed (migration 0021, `lore_core::codec`,
//! `lore_core::compact::compress_bodies`): each compressed column round trips
//! through its view, plain and compressed rows read the same, a corrupt value
//! is a query error, the conversion resumes, ingest writes compressed once a
//! dictionary exists, forget still removes everything, and no SQL outside the
//! writers reads a compressed column raw.
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use std::sync::atomic::AtomicBool;

use lore_core::adapters::claude_code::ClaudeCodeAdapter;
use lore_core::codec::{self, Column, COLUMNS};
use lore_core::compact::{self, Training};
use lore_core::export::export_session_markdown;
use lore_core::forget::{forget_all, forget_session};
use lore_core::ingest::persist_session;
use lore_core::search::search;
use lore_core::storage::blob::BlobStore;
use rusqlite::Connection;
use serde_json::{json, Value};

fn store() -> (tempfile::TempDir, BlobStore) {
    let dir = tempfile::tempdir().unwrap();
    let s = BlobStore::open(dir.path()).unwrap();
    (dir, s)
}

/// Small enough for a fixture archive to train every column.
fn training() -> Training {
    Training {
        dict_size: 2048,
        min_samples: 20,
        probes: 2000,
        sample_cap: 4096,
        sample_budget: 1 << 20,
    }
}

/// Claude Code session `k`: a prompt, an attachment, two tool calls whose
/// outputs are slimmed (migration 0020), and a reply. Every value differs
/// between sessions and most are long enough to compress.
fn transcript(k: usize) -> String {
    let s = format!("s{k:03}");
    let ts = |i: usize| format!("2026-08-11T10:{:02}:{:02}.000Z", k % 60, i);
    let mut ev: Vec<Value> = vec![
        json!({"type": "user", "uuid": format!("u0{s}"), "sessionId": s, "cwd": "/p",
               "timestamp": ts(0), "message": {"role": "user", "content":
               format!("please refactor the frobnicate module number {k} so the retry backoff \
                        uses jitter, and keep the public API of frobnicate_{k} unchanged")}}),
        json!({"type": "attachment", "uuid": format!("at{s}"), "sessionId": s, "timestamp": ts(1),
               "attachment": {"type": "prompt_snapshot", "content":
               format!("# CLAUDE.md for project {k}\nAlways run cargo fmt. Never push to main. \
                        Prefer small commits; session {k} context follows. 🦀 é \"quoted\"")}}),
    ];
    for i in 0..2 {
        ev.push(
            json!({"type": "assistant", "uuid": format!("a{i}{s}"), "sessionId": s,
            "cwd": "/p", "timestamp": ts(2 + 2 * i), "message": {"role": "assistant", "content": [
            {"type": "tool_use", "id": format!("t{i}{s}"), "name": "Bash",
             "input": {"command": format!("rg -n 'frobnicate_{k}' src/ --type rust | head -{i}0"),
                       "description": "search for the frobnicate call sites"}}]}}),
        );
        ev.push(
            json!({"type": "user", "uuid": format!("r{i}{s}"), "sessionId": s, "cwd": "/p",
            "timestamp": ts(3 + 2 * i), "message": {"role": "user", "content": [
            {"type": "tool_result", "tool_use_id": format!("t{i}{s}"), "content":
             format!("src/lib.rs:{k}:    frobnicate_{k}(retry, backoff)\nsrc/net.rs:{i}:    \
                      frobnicate_{k}(jitter)\n\t\"tab\" back\\slash done")}]}}),
        );
    }
    ev.push(
        json!({"type": "assistant", "uuid": format!("z{s}"), "sessionId": s, "cwd": "/p",
        "timestamp": ts(9), "message": {"role": "assistant", "content": [{"type": "text", "text":
        format!("I refactored frobnicate_{k}: the backoff now adds full jitter, and the public \
                 API is unchanged. Tests pass.")}]}}),
    );
    ev.iter().map(|v| format!("{v}\n")).collect()
}

fn ingest(conn: &Connection, blobs: &BlobStore, ks: std::ops::Range<usize>) -> Vec<String> {
    ks.map(|k| {
        let parsed = ClaudeCodeAdapter::new().parse_str(&transcript(k), &format!("s{k:03}"));
        persist_session(conn, "claude-code", "Claude Code", &parsed, blobs).unwrap()
    })
    .collect()
}

/// What a reader sees: each session's detail and Markdown export (masked and
/// full), and the hits of a few searches.
fn reader_view(conn: &Connection, sessions: &[String]) -> Vec<String> {
    let mut out = session_view(conn, sessions);
    for q in ["frobnicate", "jitter", "CLAUDE", "quoted", "backoff"] {
        out.push(format!("{:?}", search(conn, q, 500).unwrap()));
    }
    out
}

/// Each session's detail and Markdown export, masked and full.
fn session_view(conn: &Connection, sessions: &[String]) -> Vec<String> {
    let mut out = Vec::new();
    for s in sessions {
        out.push(format!(
            "{:?}",
            lore_core::query::get_session(conn, s).unwrap()
        ));
        out.push(export_session_markdown(conn, s, false).unwrap().unwrap());
        out.push(export_session_markdown(conn, s, true).unwrap().unwrap());
    }
    out
}

/// Every value of `column` as SQL decodes it, by rowid.
fn decoded(conn: &Connection, column: &Column) -> Vec<(i64, Option<String>)> {
    let sql = format!(
        "SELECT rowid, {} FROM {} ORDER BY rowid",
        codec::unpack_sql(column.column),
        column.table
    );
    conn.prepare(&sql)
        .unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap()
}

fn compressed(conn: &Connection, column: &Column) -> i64 {
    conn.query_row(
        &format!(
            "SELECT count(*) FROM {} WHERE typeof({}) = 'blob'",
            column.table, column.column
        ),
        [],
        |r| r.get(0),
    )
    .unwrap()
}

fn compress(conn: &Connection, batch: usize) -> lore_core::storage::Result<compact::ZstdReport> {
    compact::compress_bodies(
        conn,
        &training(),
        batch,
        &AtomicBool::new(false),
        |_| Ok(()),
    )
}

#[test]
fn every_column_round_trips_and_readers_see_the_same_bytes() {
    let conn = lore_core::storage::open_in_memory().unwrap();
    let (_bd, blobs) = store();
    let sessions = ingest(&conn, &blobs, 0..60);
    let view = reader_view(&conn, &sessions);
    let before: Vec<_> = COLUMNS.iter().map(|c| decoded(&conn, c)).collect();
    assert!(COLUMNS.iter().all(|c| compressed(&conn, c) == 0));

    let report = compress(&conn, 50).unwrap();
    let trained: Vec<String> = conn
        .prepare("SELECT \"column\" FROM zstd_dict")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(report.dicts_trained, 4, "{trained:?}");
    assert_eq!(report.failed, 0);
    assert!(!report.pending);
    assert!(report.bytes_after < report.bytes_before);
    for (c, before) in COLUMNS.iter().zip(&before) {
        assert!(compressed(&conn, c) > 0, "{} has compressed rows", c.key);
        assert_eq!(
            &decoded(&conn, c),
            before,
            "{} decodes to the same bytes",
            c.key
        );
    }
    same(&reader_view(&conn, &sessions), &view);

    // Rerunning is a no-op, and the dictionaries are never replaced.
    let ids: Vec<i64> = conn
        .prepare("SELECT id FROM zstd_dict ORDER BY id")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    let again = compress(&conn, 50).unwrap();
    assert_eq!((again.dicts_trained, again.converted), (0, 0));
    let count: i64 = conn
        .query_row("SELECT count(*) FROM zstd_dict", [], |r| r.get(0))
        .unwrap();
    assert_eq!(usize::try_from(count).unwrap(), ids.len());
}

#[test]
fn ingest_after_a_dictionary_writes_compressed_and_mixed_rows_read_the_same() {
    let plain = lore_core::storage::open_in_memory().unwrap();
    let (_pd, plain_blobs) = store();
    let all = ingest(&plain, &plain_blobs, 0..80);

    let conn = lore_core::storage::open_in_memory().unwrap();
    let (_bd, blobs) = store();
    let mut sessions = ingest(&conn, &blobs, 0..60);
    compress(&conn, 50).unwrap();
    let before: Vec<i64> = COLUMNS.iter().map(|c| compressed(&conn, c)).collect();
    sessions.extend(ingest(&conn, &blobs, 60..80));
    for (c, before) in COLUMNS.iter().zip(before) {
        assert!(
            compressed(&conn, c) > before,
            "new {} rows are compressed",
            c.key
        );
    }
    // Every third row plain again, beside compressed ones.
    conn.execute_batch(
        "UPDATE message_part SET text = (SELECT v.text FROM message_part_full v
                                          WHERE v.id = message_part.id)
         WHERE rowid % 3 = 0;",
    )
    .unwrap();
    let plain_text: i64 = conn
        .query_row(
            "SELECT count(*) FROM message_part WHERE typeof(text) = 'text'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert!(plain_text > 0);
    assert_eq!(sessions, all);
    same(&reader_view(&conn, &sessions), &reader_view(&plain, &all));
    assert!(!compact::zstd_pending(&conn).unwrap());
}

#[test]
fn a_corrupt_value_is_a_query_error() {
    let conn = lore_core::storage::open_in_memory().unwrap();
    let (_bd, blobs) = store();
    let sessions = ingest(&conn, &blobs, 0..40);
    compress(&conn, 50).unwrap();
    let (rowid, mut value): (i64, Vec<u8>) = conn
        .query_row(
            "SELECT p.rowid, p.text FROM message_part p JOIN message m ON m.id = p.message_id
             WHERE m.session_id = ?1 AND typeof(p.text) = 'blob' LIMIT 1",
            [&sessions[0]],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    let last = value.len() - 6;
    value[last] ^= 0x20;
    conn.execute(
        "UPDATE message_part SET text = ?2 WHERE rowid = ?1",
        rusqlite::params![rowid, value],
    )
    .unwrap();
    assert!(lore_core::query::get_session(&conn, &sessions[0]).is_err());
    assert!(export_session_markdown(&conn, &sessions[0], true).is_err());
    // Other sessions still read.
    assert!(lore_core::query::get_session(&conn, &sessions[1])
        .unwrap()
        .is_some());

    // A value naming a dictionary that is not there is an error too.
    conn.execute(
        "DELETE FROM zstd_dict WHERE \"column\" = ?1",
        [codec::TOOL_INPUT.key],
    )
    .unwrap();
    let missing = conn.query_row("SELECT count(input_json) FROM tool_call_full", [], |r| {
        r.get::<_, i64>(0)
    });
    assert!(missing.is_err());
}

#[test]
fn an_error_mid_conversion_keeps_every_row_and_the_rerun_finishes() {
    let conn = lore_core::storage::open_in_memory().unwrap();
    let (_bd, blobs) = store();
    let sessions = ingest(&conn, &blobs, 0..60);
    let view = reader_view(&conn, &sessions);
    let mid: i64 = conn
        .query_row("SELECT max(rowid) / 2 FROM message_part", [], |r| r.get(0))
        .unwrap();
    conn.execute_batch(&format!(
        "CREATE TEMP TRIGGER boom BEFORE UPDATE OF text ON message_part
         WHEN old.rowid = {mid} BEGIN SELECT RAISE(ABORT, 'boom'); END;"
    ))
    .unwrap();
    assert!(compress(&conn, 7).is_err());
    let partial = compressed(&conn, &codec::PART_TEXT);
    assert!(partial > 0, "batches before the error committed");
    assert!(compact::zstd_pending(&conn).unwrap());
    same(&reader_view(&conn, &sessions), &view);

    conn.execute_batch("DROP TRIGGER boom;").unwrap();
    let report = compress(&conn, 7).unwrap();
    assert_eq!(
        report.dicts_trained, 0,
        "the dictionaries stored before are kept"
    );
    assert!(!report.pending);
    assert!(compressed(&conn, &codec::PART_TEXT) > partial);
    same(&reader_view(&conn, &sessions), &view);
}

#[test]
fn a_value_that_does_not_read_back_is_left_as_it_was_and_counted() {
    let conn = lore_core::storage::open_in_memory().unwrap();
    let (_bd, blobs) = store();
    let sessions = ingest(&conn, &blobs, 0..40);
    let view = reader_view(&conn, &sessions);
    // Damage one value as it is written: its read-back fails.
    let target: i64 = conn
        .query_row(
            "SELECT max(rowid) FROM tool_call WHERE length(input_json) > 64",
            [],
            |r| r.get(0),
        )
        .unwrap();
    conn.execute_batch(&format!(
        "CREATE TEMP TRIGGER damage AFTER UPDATE OF input_json ON tool_call
         WHEN new.rowid = {target} BEGIN
           UPDATE tool_call SET input_json = X'FF000000000000' WHERE rowid = {target};
         END;"
    ))
    .unwrap();
    let report = compress(&conn, 50).unwrap();
    assert_eq!(report.failed, 1);
    let kind: String = conn
        .query_row(
            "SELECT typeof(input_json) FROM tool_call WHERE rowid = ?1",
            [target],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(kind, "text", "rolled back to the plain value");
    same(&reader_view(&conn, &sessions), &view);
}

#[test]
fn forget_removes_compressed_rows() {
    let conn = lore_core::storage::open_in_memory().unwrap();
    let (_bd, blobs) = store();
    let sessions = ingest(&conn, &blobs, 0..40);
    compress(&conn, 50).unwrap();
    let rest = session_view(&conn, &sessions[1..]);
    forget_session(&conn, &blobs, &sessions[0]).unwrap();
    assert!(lore_core::query::get_session(&conn, &sessions[0])
        .unwrap()
        .is_none());
    same(&session_view(&conn, &sessions[1..]), &rest);

    forget_all(&conn, &blobs).unwrap();
    for c in COLUMNS {
        let n: i64 = conn
            .query_row(&format!("SELECT count(*) FROM {}", c.table), [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(n, 0, "{} is empty", c.table);
    }
}

#[test]
fn a_small_archive_gets_no_dictionary_and_compact_still_finishes() {
    let dir = tempfile::tempdir().unwrap();
    let conn = lore_core::storage::open(&dir.path().join("lore.db")).unwrap();
    let blobs = BlobStore::open(dir.path().join("blobs")).unwrap();
    let sessions = ingest(&conn, &blobs, 0..5);
    let view = reader_view(&conn, &sessions);
    let report = compact::compact(&conn, &AtomicBool::new(false)).unwrap();
    assert_eq!(report.zstd.dicts_trained, 0);
    assert_eq!(report.zstd.converted, 0);
    assert!(!report.zstd.pending);
    same(&reader_view(&conn, &sessions), &view);
}

/// Every SQL string literal in `source`, with its own quoted SQL literals
/// removed. Comments are skipped.
fn sql_literals(source: &str) -> Vec<String> {
    let b = source.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'/' if b.get(i + 1) == Some(&b'/') => {
                while i < b.len() && b[i] != b'\n' {
                    i += 1;
                }
            }
            b'/' if b.get(i + 1) == Some(&b'*') => {
                i += 2;
                while i + 1 < b.len() && !(b[i] == b'*' && b[i + 1] == b'/') {
                    i += 1;
                }
                i += 2;
            }
            b'\'' => {
                // A char literal, or a lifetime.
                if b.get(i + 1) == Some(&b'\\') {
                    i += 2;
                    while i < b.len() && b[i] != b'\'' {
                        i += 1;
                    }
                    i += 1;
                } else if b.get(i + 2) == Some(&b'\'') {
                    i += 3;
                } else {
                    i += 1;
                }
            }
            b'r' if matches!(b.get(i + 1), Some(b'"' | b'#'))
                && (i == 0 || !(b[i - 1].is_ascii_alphanumeric() || b[i - 1] == b'_')) =>
            {
                let mut j = i + 1;
                while b.get(j) == Some(&b'#') {
                    j += 1;
                }
                if b.get(j) != Some(&b'"') {
                    i += 1;
                    continue;
                }
                let close = format!("\"{}", "#".repeat(j - i - 1));
                let start = j + 1;
                let end = source[start..].find(&close).map_or(b.len(), |e| start + e);
                out.push(source[start..end].to_owned());
                i = end + close.len();
            }
            b'"' => {
                let start = i + 1;
                i += 1;
                while i < b.len() && b[i] != b'"' {
                    i += if b[i] == b'\\' { 2 } else { 1 };
                }
                out.push(source[start..i.min(b.len())].to_owned());
                i += 1;
            }
            _ => i += 1,
        }
    }
    out.into_iter().map(|s| strip_sql_strings(&s)).collect()
}

fn strip_sql_strings(sql: &str) -> String {
    let mut out = String::with_capacity(sql.len());
    let mut quoted = false;
    for ch in sql.chars() {
        if ch == '\'' {
            quoted = !quoted;
        } else if !quoted {
            out.push(ch);
        }
    }
    out
}

fn has_word(hay: &str, word: &str) -> bool {
    let is_word = |c: char| c.is_ascii_alphanumeric() || c == '_';
    hay.match_indices(word).any(|(at, _)| {
        !hay[..at].chars().next_back().is_some_and(is_word)
            && !hay[at + word.len()..].chars().next().is_some_and(is_word)
    })
}

/// The SQL in `source` that names a compressed column's table raw together
/// with that column.
fn raw_reads(source: &str) -> Vec<String> {
    sql_literals(source)
        .into_iter()
        .filter(|sql| {
            COLUMNS
                .iter()
                .any(|c| has_word(sql, c.table) && has_word(sql, c.column))
        })
        .collect()
}

/// The writers and the codec itself: they store and convert the raw values.
const WRITERS: &[&str] = &[
    "crates/lore-core/src/codec.rs",
    "crates/lore-core/src/compact.rs",
    "crates/lore-core/src/ingest.rs",
    "crates/lore-core/src/storage/migrations.rs",
];

fn rust_files(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            rust_files(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

#[test]
fn the_guard_catches_a_raw_read() {
    assert_eq!(
        raw_reads("let q = \"SELECT tc.input_json FROM tool_call tc\";").len(),
        1
    );
    assert_eq!(
        raw_reads("conn.prepare(r#\"SELECT text FROM message_part\"#)").len(),
        1
    );
    assert!(raw_reads("\"SELECT input_json FROM tool_call_full\"").is_empty());
    assert!(raw_reads(
        "\"SELECT redacted_text FROM search_document JOIN message_part p ON p.kind = 'text'\""
    )
    .is_empty());
    assert!(raw_reads("// SELECT text FROM message_part\nlet x = 'a';").is_empty());
}

#[test]
fn no_sql_outside_the_writers_reads_a_compressed_column_raw() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let root = root.canonicalize().unwrap();
    let mut files = Vec::new();
    for crate_dir in std::fs::read_dir(root.join("crates")).unwrap() {
        let src = crate_dir.unwrap().path().join("src");
        if src.is_dir() {
            rust_files(&src, &mut files);
        }
    }
    rust_files(&root.join("src-tauri/src"), &mut files);
    assert!(files.len() > 50, "found the sources");
    let mut offenders = Vec::new();
    for file in files {
        let rel = file
            .strip_prefix(&root)
            .unwrap()
            .to_string_lossy()
            .replace('\\', "/");
        if WRITERS.contains(&rel.as_str()) {
            continue;
        }
        for sql in raw_reads(&std::fs::read_to_string(&file).unwrap()) {
            offenders.push(format!(
                "{rel}: {}",
                sql.split_whitespace().collect::<Vec<_>>().join(" ")
            ));
        }
    }
    assert!(
        offenders.is_empty(),
        "read these through part_body_full, message_part_full or tool_call_full:\n{}",
        offenders.join("\n")
    );
}

/// Assert two reader views are equal, showing only where they first differ.
fn same(got: &[String], want: &[String]) {
    assert_eq!(got.len(), want.len(), "as many reads");
    for (i, (g, w)) in got.iter().zip(want).enumerate() {
        if g != w {
            let at = g.bytes().zip(w.bytes()).take_while(|(a, b)| a == b).count();
            let from = at.saturating_sub(120);
            let cut = |s: &str| {
                s.get(from..(at + 120).min(s.len()))
                    .unwrap_or("")
                    .to_owned()
            };
            panic!(
                "read {i} differs at byte {at}:\n got: {}\nwant: {}",
                cut(g),
                cut(w)
            );
        }
    }
}

#[test]
fn the_tool_output_marker_is_put_back_after_decoding() {
    let conn = lore_core::storage::open_in_memory().unwrap();
    let (_bd, blobs) = store();
    ingest(&conn, &blobs, 0..1);
    let (id, text): (String, String) = conn
        .query_row(
            "SELECT id, text FROM message_part WHERE kind = 'tool_result' LIMIT 1",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    let pad = "metadata ".repeat(40);
    let slim = format!(
        "{{\"pad\":\"{pad}\",\"out\":{}}}",
        compact::TEXT_QUOTED_MARKER
    );
    let whole = format!(
        "{{\"pad\":\"{pad}\",\"out\":{}}}",
        serde_json::to_string(&text).unwrap()
    );
    let packed = codec::Encoder::new(0, None).unwrap().encode(&slim).unwrap();
    let packed_text = codec::Encoder::new(0, None)
        .unwrap()
        .encode(&format!("{text}{}", " ".repeat(200)));
    assert!(packed_text.is_some());
    conn.execute(
        "UPDATE message_part SET content_json = ?2 WHERE id = ?1",
        rusqlite::params![id, packed],
    )
    .unwrap();
    assert_eq!(
        compact::part_content(&conn, &id).unwrap().as_deref(),
        Some(whole.as_str())
    );

    // With the text compressed too, the marker takes the decoded text.
    let long_text = format!("{text}{}", " ".repeat(200));
    let whole = format!(
        "{{\"pad\":\"{pad}\",\"out\":{}}}",
        serde_json::to_string(&long_text).unwrap()
    );
    conn.execute(
        "UPDATE message_part SET text = ?2 WHERE id = ?1",
        rusqlite::params![id, packed_text.unwrap()],
    )
    .unwrap();
    assert_eq!(
        compact::part_content(&conn, &id).unwrap().as_deref(),
        Some(whole.as_str())
    );
}

#[test]
fn one_vacuum_after_compression_returns_the_slack() {
    let dir = tempfile::tempdir().unwrap();
    let conn = lore_core::storage::open(&dir.path().join("lore.db")).unwrap();
    let blobs = BlobStore::open(dir.path().join("blobs")).unwrap();
    let sessions = ingest(&conn, &blobs, 0..200);
    let view = reader_view(&conn, &sessions);
    assert_eq!(
        compact::vacuum_after_compression(&conn).unwrap(),
        None,
        "no dictionary yet"
    );
    compress(&conn, 100).unwrap();
    match compact::vacuum_after_compression(&conn).unwrap() {
        Some(compact::VacuumOutcome::Vacuumed {
            pages_before,
            pages_after,
        }) => {
            assert!(pages_after < pages_before);
        }
        other => panic!("expected a vacuum, got {other:?}"),
    }
    assert_eq!(
        compact::vacuum_after_compression(&conn).unwrap(),
        None,
        "once"
    );
    same(&reader_view(&conn, &sessions), &view);
}
