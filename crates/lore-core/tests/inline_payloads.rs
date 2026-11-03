//! Inline base64 payloads (screenshots, image attachments, PDFs) are moved out
//! of `content_json` into the blob store, stored once by content hash, with a
//! reference left in their place. They are never secret-scanned or indexed.
#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::path::Path;

use lore_core::adapters::{AdapterRegistry, DiscoveryRoots};
use lore_core::discovery::{discover, DiscoveryConfig};
use lore_core::ingest::ingest_discovered;
use lore_core::storage::blob::BlobStore;
use rusqlite::Connection;
use serde_json::json;

/// `n` bytes of deterministic noise, base64-encoded: what a screenshot looks
/// like to the archive, high entropy and all.
fn base64_noise(n: usize, seed: u32) -> String {
    const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut state = seed.wrapping_mul(2_654_435_761).wrapping_add(1);
    let bytes: Vec<u8> = (0..n)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            (state >> 7) as u8
        })
        .collect();
    let mut out = String::new();
    for chunk in bytes.chunks(3) {
        let b = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(ALPHABET[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

fn source(media_type: &str, data: &str) -> serde_json::Value {
    json!({ "type": "base64", "media_type": media_type, "data": data })
}

/// One Claude Code transcript: a screenshot tool result, an image attachment,
/// a PDF attachment, the same screenshot again, and a small inline image.
fn archive(dir: &Path, png: &str, jpeg: &str, pdf: &str, tiny: &str) -> Connection {
    write_transcript(
        dir,
        "cccccccc-0000-4000-8000-000000000001",
        png,
        jpeg,
        pdf,
        tiny,
    );
    ingest(dir)
}

fn write_transcript(dir: &Path, sid: &str, png: &str, jpeg: &str, pdf: &str, tiny: &str) {
    let project = dir.join("claude/projects/-repo-app");
    std::fs::create_dir_all(&project).unwrap();
    let line = |n: u32, kind: &str, parent: Option<String>, content: serde_json::Value| {
        let message = if kind == "user" {
            json!({ "role": "user", "content": content })
        } else {
            json!({ "role": "assistant", "model": "claude-x", "content": content })
        };
        json!({
            "type": kind, "uuid": format!("u{n}"), "parentUuid": parent, "sessionId": sid,
            "timestamp": format!("2026-10-01T10:00:{n:02}.000Z"), "cwd": "/repo/app",
            "version": "2.1.0", "isSidechain": false, "userType": "external",
            "message": message,
        })
    };
    let lines = [
        line(
            0,
            "assistant",
            None,
            json!([
                { "type": "tool_use", "id": "t1", "name": "screenshot", "input": {} }
            ]),
        ),
        line(
            1,
            "user",
            Some("u0".into()),
            json!([{
                "type": "tool_result", "tool_use_id": "t1",
                "content": [
                    { "type": "text", "text": "Screenshot taken" },
                    { "type": "image", "source": source("image/png", png) },
                ],
            }]),
        ),
        line(
            2,
            "user",
            Some("u1".into()),
            json!([
                { "type": "text", "text": "what is in this photo" },
                { "type": "image", "source": source("image/jpeg", jpeg) },
            ]),
        ),
        line(
            3,
            "user",
            Some("u2".into()),
            json!([
                { "type": "text", "text": "summarize the spec" },
                { "type": "document", "source": source("application/pdf", pdf) },
            ]),
        ),
        line(
            4,
            "assistant",
            Some("u3".into()),
            json!([
                { "type": "tool_use", "id": "t2", "name": "screenshot", "input": {} }
            ]),
        ),
        line(
            5,
            "user",
            Some("u4".into()),
            json!([{
                "type": "tool_result", "tool_use_id": "t2",
                "content": [{ "type": "image", "source": source("image/png", png) }],
            }]),
        ),
        line(
            6,
            "user",
            Some("u5".into()),
            json!([
                { "type": "image", "source": source("image/png", tiny) },
            ]),
        ),
    ];
    let body: String = lines.iter().map(|l| format!("{l}\n")).collect();
    std::fs::write(project.join(format!("{sid}.jsonl")), body).unwrap();
}

fn ingest(dir: &Path) -> Connection {
    let mut config = DiscoveryConfig::new();
    config.set_roots(
        "claude-code",
        DiscoveryRoots::new(vec![dir.join("claude/projects")]),
    );
    config.set_roots("codex", DiscoveryRoots::new(vec![dir.join("none")]));
    let registry = AdapterRegistry::v0();
    let found = discover(&registry, &config);
    let conn = lore_core::storage::open_in_memory().unwrap();
    let blobs = BlobStore::open(dir.join("blobs")).unwrap();
    let report = ingest_discovered(&conn, &registry, &found.sessions, &blobs);
    assert!(report.failures.is_empty());
    conn
}

#[test]
fn inline_base64_payloads_are_stored_once_as_blobs_and_never_scanned() {
    let dir = tempfile::tempdir().unwrap();
    let (png, jpeg, pdf) = (
        base64_noise(9_000, 1),
        base64_noise(7_000, 2),
        base64_noise(12_000, 3),
    );
    let tiny = base64_noise(60, 4);
    let conn = archive(dir.path(), &png, &jpeg, &pdf, &tiny);

    let parts: Vec<String> = conn
        .prepare("SELECT content_json FROM message_part WHERE content_json IS NOT NULL")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    for payload in [&png, &jpeg, &pdf] {
        assert!(
            !parts.iter().any(|p| p.contains(payload.as_str())),
            "a payload stayed inline"
        );
    }
    let references: usize = parts
        .iter()
        .map(|p| p.matches("\"lore-blob:").count())
        .sum();
    assert_eq!(
        references, 4,
        "two screenshots, the attachment, and the PDF"
    );
    assert!(
        parts.iter().any(|p| p.contains(tiny.as_str())),
        "a small payload stays inline"
    );

    let mut rows: Vec<(String, i64, String)> = conn
        .prepare("SELECT media_type, byte_len, scan_state FROM blob ORDER BY media_type")
        .unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    rows.sort();
    let len = |s: &str| i64::try_from(s.len()).unwrap();
    assert_eq!(
        rows,
        [
            (
                "application/pdf".to_owned(),
                len(&pdf),
                "opaque_excluded".to_owned()
            ),
            (
                "image/jpeg".to_owned(),
                len(&jpeg),
                "opaque_excluded".to_owned()
            ),
            (
                "image/png".to_owned(),
                len(&png),
                "opaque_excluded".to_owned()
            ),
        ]
    );
    let relpath: String = conn
        .query_row(
            "SELECT storage_relpath FROM blob WHERE media_type = 'image/png'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    let store = BlobStore::open_existing(dir.path().join("blobs")).unwrap();
    assert_eq!(store.read(&relpath).unwrap(), png.as_bytes());

    // The only finding in part JSON is the small payload left inline, which
    // is scanned like any other text. Nothing moved, and no reference, is.
    let spans: Vec<String> = conn
        .prepare(
            "SELECT substr(p.content_json, f.span_start + 1, f.span_end - f.span_start)
             FROM secret_finding f JOIN message_part p ON p.id = f.source_id
             WHERE f.field = 'content_json'",
        )
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(spans, std::slice::from_ref(&tiny));
    let indexed: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM search_document WHERE redacted_text LIKE '%' || ?1 || '%'",
            [&png[..64]],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(indexed, 0);
}

#[test]
fn forgetting_one_session_keeps_a_payload_another_still_uses() {
    let dir = tempfile::tempdir().unwrap();
    let png = base64_noise(9_000, 1);
    let (a, b) = (base64_noise(7_000, 2), base64_noise(7_000, 5));
    let (pdf, tiny) = (base64_noise(5_000, 3), base64_noise(60, 4));
    write_transcript(
        dir.path(),
        "cccccccc-0000-4000-8000-000000000001",
        &png,
        &a,
        &pdf,
        &tiny,
    );
    write_transcript(
        dir.path(),
        "cccccccc-0000-4000-8000-000000000002",
        &png,
        &b,
        &pdf,
        &tiny,
    );
    let conn = ingest(dir.path());
    let blobs = BlobStore::open(dir.path().join("blobs")).unwrap();
    let count = |media: &str| -> i64 {
        conn.query_row(
            "SELECT COUNT(*) FROM blob WHERE media_type = ?1",
            [media],
            |r| r.get(0),
        )
        .unwrap()
    };
    assert_eq!(
        (count("image/png"), count("image/jpeg")),
        (1, 2),
        "a shared png is stored once"
    );
    let sessions: Vec<String> = conn
        .prepare("SELECT id FROM agent_session ORDER BY native_session_id")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();

    lore_core::forget::forget_session(&conn, &blobs, &sessions[0]).unwrap();
    assert_eq!((count("image/png"), count("image/jpeg")), (1, 1));
    lore_core::forget::forget_session(&conn, &blobs, &sessions[1]).unwrap();
    assert_eq!((count("image/png"), count("image/jpeg")), (0, 0));
    let files = walk_files(&dir.path().join("blobs"));
    assert_eq!(
        files,
        Vec::<String>::new(),
        "a forgotten payload file was left behind"
    );
}

fn walk_files(dir: &Path) -> Vec<String> {
    let mut out = Vec::new();
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            if path.file_name().unwrap() != "tmp" {
                out.extend(walk_files(&path));
            }
        } else {
            out.push(path.display().to_string());
        }
    }
    out
}
