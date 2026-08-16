//! M6 acceptance: FTS5 search over redacted projections — identifier/title/path
//! recall, provenance filters, highlighted snippets, and no secret ever
//! surfacing in a result snippet.
#![allow(clippy::expect_used, clippy::unwrap_used)]

use lore_core::adapters::claude_code::ClaudeCodeAdapter;
use lore_core::adapters::codex::CodexAdapter;
use lore_core::ingest::persist_session;
use lore_core::search::{search, search_page, SortOrder, HIGHLIGHT_START};
use lore_core::storage::blob::BlobStore;
use rusqlite::Connection;

fn store() -> (tempfile::TempDir, BlobStore) {
    let dir = tempfile::tempdir().unwrap();
    let s = BlobStore::open(dir.path()).unwrap();
    (dir, s)
}

fn user_message(session: &str, cwd: &str, text: &str) -> String {
    format!(
        "{{\"type\":\"user\",\"uuid\":\"u_{session}\",\"sessionId\":\"{session}\",\"cwd\":\"{cwd}\",\"message\":{{\"role\":\"user\",\"content\":\"{text}\"}}}}\n"
    )
}

fn persist_claude(conn: &Connection, blobs: &BlobStore, jsonl: &str, dedupe: &str) -> String {
    let parsed = ClaudeCodeAdapter::new().parse_str(jsonl, dedupe);
    persist_session(conn, "claude-code", "Claude Code", &parsed, blobs).unwrap()
}

#[test]
fn identifier_recall_with_highlighted_snippet() {
    let conn = lore_core::storage::open_in_memory().unwrap();
    let (_bd, blobs) = store();
    persist_claude(
        &conn,
        &blobs,
        &user_message("s1", "/p", "the retryBackoff helper handles jitter"),
        "s1",
    );

    let hits = search(&conn, "retryBackoff", 20).unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].source_kind, "message_part");
    assert!(hits[0].snippet.contains(HIGHLIGHT_START));
    assert!(hits[0].snippet.contains("retryBackoff"));

    // A term that is not present returns nothing.
    assert!(search(&conn, "nonexistentterm", 20).unwrap().is_empty());
    // An empty query returns nothing (never a bare FTS MATCH).
    assert!(search(&conn, "   ", 20).unwrap().is_empty());
}

#[test]
fn projection_carries_session_sort_and_filter_keys() {
    // Migration 0007 denormalizes started_at/agent_id onto search_document so the
    // ranked page can order and filter before joining agent_session. Those keys
    // must always mirror the owning session, or search would rank/filter wrong.
    let conn = lore_core::storage::open_in_memory().unwrap();
    let (_bd, blobs) = store();
    persist_claude(
        &conn,
        &blobs,
        &user_message("s1", "/p", "deploy the retryBackoff helper"),
        "s1",
    );

    // `IS NOT` is NULL-safe, so a null started_at on either side must also match.
    let mismatches: i64 = conn
        .query_row(
            "SELECT count(*) FROM search_document sd
             JOIN agent_session s ON s.id = sd.session_id
             WHERE sd.agent_id IS NOT s.agent_id OR sd.started_at IS NOT s.started_at",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(mismatches, 0, "projection keys must mirror agent_session");

    let agent: String = conn
        .query_row("SELECT DISTINCT agent_id FROM search_document", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(agent, "claude-code");
}

#[test]
fn title_is_searchable() {
    let conn = lore_core::storage::open_in_memory().unwrap();
    let (_bd, blobs) = store();
    let jsonl = format!(
        "{}{}",
        "{\"type\":\"custom-title\",\"sessionId\":\"t1\",\"customTitle\":\"Fix billing webhook signature\"}\n",
        user_message("t1", "/p", "hello there")
    );
    persist_claude(&conn, &blobs, &jsonl, "t1");

    let hits = search(&conn, "webhook", 20).unwrap();
    assert!(hits.iter().any(|h| h.field == "title"));
}

/// Regression: a synthetic fallback title (derived from the first user message
/// when no native title event exists) must not be indexed as its own document.
/// Indexing it duplicated every message-text hit into a second `title` hit
/// (`SEARCH.md` §6, "without duplicates"). A native title is still indexed
/// (`title_is_searchable`); only the redundant fallback is suppressed.
#[test]
fn synthetic_fallback_title_is_not_indexed() {
    let conn = lore_core::storage::open_in_memory().unwrap();
    let (_bd, blobs) = store();
    // No custom-title/ai-title event: the title is derived from this message.
    persist_claude(
        &conn,
        &blobs,
        &user_message("f1", "/p", "the retryBackoff helper handles jitter"),
        "f1",
    );

    // Exactly one hit, from the message part — never a second `title` hit for
    // the same term, even though the display title echoes this text.
    let hits = search(&conn, "retryBackoff", 20).unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].source_kind, "message_part");
    assert!(hits.iter().all(|h| h.field != "title"));

    // And no `session`/`title` projection row was written at all.
    let title_docs: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM search_document WHERE source_kind = 'session'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(title_docs, 0, "fallback title must not be projected");
}

#[test]
fn agent_filter_narrows_results() {
    let conn = lore_core::storage::open_in_memory().unwrap();
    let (_bd, blobs) = store();
    persist_claude(
        &conn,
        &blobs,
        &user_message("c1", "/p", "deploy the service"),
        "c1",
    );
    let codex = concat!(
        "{\"type\":\"session_meta\",\"timestamp\":\"2026-08-11T10:00:00.000Z\",\"payload\":{\"id\":\"x1\",\"cli_version\":\"1\",\"cwd\":\"/p\"}}\n",
        "{\"type\":\"response_item\",\"timestamp\":\"2026-08-11T10:00:01.000Z\",\"payload\":{\"type\":\"message\",\"role\":\"user\",\"content\":\"deploy the service\"}}\n"
    );
    let parsed = CodexAdapter::new().parse_str(codex, "x1");
    persist_session(&conn, "codex", "Codex", &parsed, &blobs).unwrap();

    assert_eq!(search(&conn, "deploy", 20).unwrap().len(), 2);
    let codex_only = search(&conn, "deploy agent:codex", 20).unwrap();
    assert_eq!(codex_only.len(), 1);
    assert_eq!(codex_only[0].agent_id, "codex");
}

#[test]
fn path_filter_matches_touched_files() {
    let conn = lore_core::storage::open_in_memory().unwrap();
    let (_bd, blobs) = store();
    // A session whose assistant edits auth/login.ts and whose text says "the fix".
    let jsonl = format!(
        "{}{}",
        user_message("p1", "/p", "here is the fix"),
        "{\"type\":\"assistant\",\"uuid\":\"a1\",\"sessionId\":\"p1\",\"cwd\":\"/p\",\"message\":{\"role\":\"assistant\",\"content\":[{\"type\":\"tool_use\",\"id\":\"t1\",\"name\":\"Edit\",\"input\":{\"file_path\":\"auth/login.ts\"}}]}}\n"
    );
    persist_claude(&conn, &blobs, &jsonl, "p1");

    assert_eq!(search(&conn, "fix path:auth/", 20).unwrap().len(), 1);
    assert!(search(&conn, "fix path:billing/", 20).unwrap().is_empty());
}

#[test]
fn has_error_filter_selects_sessions_with_a_failed_tool() {
    let conn = lore_core::storage::open_in_memory().unwrap();
    let (_bd, blobs) = store();
    // e1: text mentions "migration" and has a failing tool call.
    let with_error = format!(
        "{}{}{}",
        user_message("e1", "/p", "the migration failed here"),
        "{\"type\":\"assistant\",\"uuid\":\"a1\",\"sessionId\":\"e1\",\"cwd\":\"/p\",\"message\":{\"role\":\"assistant\",\"content\":[{\"type\":\"tool_use\",\"id\":\"t1\",\"name\":\"Bash\",\"input\":{\"command\":\"npm test\"}}]}}\n",
        "{\"type\":\"user\",\"uuid\":\"u2\",\"sessionId\":\"e1\",\"cwd\":\"/p\",\"message\":{\"role\":\"user\",\"content\":[{\"type\":\"tool_result\",\"tool_use_id\":\"t1\",\"is_error\":true,\"content\":\"1 failing\"}]}}\n"
    );
    persist_claude(&conn, &blobs, &with_error, "e1");
    // e2: mentions "migration" but no failure.
    persist_claude(
        &conn,
        &blobs,
        &user_message("e2", "/p", "the migration succeeded"),
        "e2",
    );

    assert_eq!(search(&conn, "migration", 20).unwrap().len(), 2);
    let failed = search(&conn, "migration has:error", 20).unwrap();
    assert_eq!(failed.len(), 1);
    assert_eq!(
        failed[0].session_id,
        search(&conn, "failed has:error", 20).unwrap()[0].session_id
    );
}

#[test]
fn snippets_never_surface_a_secret() {
    let conn = lore_core::storage::open_in_memory().unwrap();
    let (_bd, blobs) = store();
    let secret = format!("ghp{}", "_0123456789abcdefghijklmnopqrstuvwxyz");
    let body = "0123456789abcdefghijklmnopqrstuvwxyz";
    persist_claude(
        &conn,
        &blobs,
        &user_message("k1", "/p", &format!("deploy with token {secret} now")),
        "k1",
    );

    let hits = search(&conn, "deploy", 20).unwrap();
    assert_eq!(hits.len(), 1);
    assert!(hits[0].snippet.contains("deploy"));
    assert!(!hits[0].snippet.contains(&secret));
    assert!(
        !hits[0].snippet.contains(body),
        "no raw secret in a snippet"
    );
}

#[test]
fn keyset_pagination_is_stable_and_complete() {
    // SEARCH.md §6: paging must reproduce the single-shot ranked order exactly,
    // never dropping or repeating a row. Every document shares the same text so
    // all bm25 ranks tie — the tie-break (started_at DESC, id ASC) alone drives
    // the order, maximally stressing the keyset cursor. Codex sessions carry a
    // timestamp (non-null started_at → the `Some` cursor branch); Claude ones
    // do not (null started_at → the `None`/NULLs-last branch).
    let conn = lore_core::storage::open_in_memory().unwrap();
    let (_bd, blobs) = store();
    for i in 0..10 {
        persist_claude(
            &conn,
            &blobs,
            &user_message(&format!("cl{i}"), "/p", "commonterm appears here"),
            &format!("cl{i}"),
        );
    }
    for i in 0..6 {
        let codex = format!(
            "{{\"type\":\"session_meta\",\"timestamp\":\"2026-08-11T10:00:0{i}.000Z\",\"payload\":{{\"id\":\"cx{i}\",\"cli_version\":\"1\",\"cwd\":\"/p\"}}}}\n\
             {{\"type\":\"response_item\",\"timestamp\":\"2026-08-11T10:00:0{i}.000Z\",\"payload\":{{\"type\":\"message\",\"role\":\"user\",\"content\":\"commonterm appears here\"}}}}\n"
        );
        let parsed = CodexAdapter::new().parse_str(&codex, &format!("cx{i}"));
        persist_session(&conn, "codex", "Codex", &parsed, &blobs).unwrap();
    }

    // Ground truth: one big page.
    let full = search(&conn, "commonterm", 100).unwrap();
    assert_eq!(full.len(), 16, "all 16 sessions match");

    // Page through in small pages and reassemble.
    let mut paged: Vec<(String, String)> = Vec::new();
    let mut cursor: Option<String> = None;
    let mut pages = 0;
    loop {
        let page = search_page(
            &conn,
            "commonterm",
            3,
            cursor.as_deref(),
            SortOrder::Relevance,
        )
        .unwrap();
        pages += 1;
        assert!(pages <= 20, "pagination must terminate");
        for h in &page.hits {
            paged.push((h.session_id.clone(), h.source_id.clone()));
        }
        match page.next_cursor {
            Some(c) => cursor = Some(c),
            None => break,
        }
    }

    let truth: Vec<(String, String)> = full
        .iter()
        .map(|h| (h.session_id.clone(), h.source_id.clone()))
        .collect();
    assert_eq!(
        paged, truth,
        "paged order matches single-shot order exactly"
    );

    let unique: std::collections::HashSet<_> = paged.iter().cloned().collect();
    assert_eq!(unique.len(), paged.len(), "no row is repeated across pages");
}

/// Build a Codex session whose `session_meta` timestamp fixes `started_at`.
fn codex_at(conn: &Connection, blobs: &BlobStore, id: &str, ts: &str, text: &str) {
    let jsonl = format!(
        "{{\"type\":\"session_meta\",\"timestamp\":\"{ts}\",\"payload\":{{\"id\":\"{id}\",\"cli_version\":\"1\",\"cwd\":\"/p\"}}}}\n\
         {{\"type\":\"response_item\",\"timestamp\":\"{ts}\",\"payload\":{{\"type\":\"message\",\"role\":\"user\",\"content\":\"{text}\"}}}}\n"
    );
    let parsed = CodexAdapter::new().parse_str(&jsonl, id);
    persist_session(conn, "codex", "Codex", &parsed, blobs).unwrap();
}

#[test]
fn newest_and_oldest_sorts_order_by_start_time_nulls_last() {
    let conn = lore_core::storage::open_in_memory().unwrap();
    let (_bd, blobs) = store();
    // Three timestamped Codex sessions (non-null started_at, ascending) and one
    // Claude session with no timestamp (null started_at).
    codex_at(
        &conn,
        &blobs,
        "cx0",
        "2026-08-11T10:00:00.000Z",
        "sortterm one",
    );
    codex_at(
        &conn,
        &blobs,
        "cx1",
        "2026-08-11T10:00:01.000Z",
        "sortterm two",
    );
    codex_at(
        &conn,
        &blobs,
        "cx2",
        "2026-08-11T10:00:02.000Z",
        "sortterm three",
    );
    persist_claude(
        &conn,
        &blobs,
        &user_message("clx", "/p", "sortterm four"),
        "clx",
    );

    let times = |sort| -> Vec<Option<i64>> {
        search_page(&conn, "sortterm", 50, None, sort)
            .unwrap()
            .hits
            .iter()
            .map(|h| h.started_at)
            .collect()
    };

    // Newest: non-null timestamps descending, then the null-start session.
    let newest = times(SortOrder::Newest);
    let non_null: Vec<i64> = newest.iter().flatten().copied().collect();
    let mut sorted_desc = non_null.clone();
    sorted_desc.sort_by(|a, b| b.cmp(a));
    assert_eq!(non_null, sorted_desc, "newest orders non-null desc");
    assert_eq!(newest.last(), Some(&None), "null-start session sorts last");

    // Oldest: non-null timestamps ascending, null-start still last.
    let oldest = times(SortOrder::Oldest);
    let non_null: Vec<i64> = oldest.iter().flatten().copied().collect();
    let mut sorted_asc = non_null.clone();
    sorted_asc.sort_unstable();
    assert_eq!(non_null, sorted_asc, "oldest orders non-null asc");
    assert_eq!(oldest.last(), Some(&None), "null-start session sorts last");

    // The non-null timestamps are reverses of each other; nulls stay last in
    // both, so the full vectors are not simple reverses.
    let newest_non_null: Vec<i64> = newest.iter().flatten().copied().collect();
    let oldest_non_null_rev: Vec<i64> = oldest.iter().flatten().rev().copied().collect();
    assert_eq!(
        newest_non_null, oldest_non_null_rev,
        "non-null order reverses between newest and oldest"
    );
}

#[test]
fn newest_sort_paginates_without_duplicates() {
    let conn = lore_core::storage::open_in_memory().unwrap();
    let (_bd, blobs) = store();
    // Ten timestamped sessions plus two null-start ones; page the newest sort in
    // twos and require the reassembly to equal the single-shot newest order.
    for i in 0..10 {
        codex_at(
            &conn,
            &blobs,
            &format!("cx{i}"),
            &format!("2026-08-11T10:00:{i:02}.000Z"),
            "pageterm here",
        );
    }
    for i in 0..2 {
        persist_claude(
            &conn,
            &blobs,
            &user_message(&format!("cl{i}"), "/p", "pageterm here"),
            &format!("cl{i}"),
        );
    }

    let full: Vec<String> = search_page(&conn, "pageterm", 50, None, SortOrder::Newest)
        .unwrap()
        .hits
        .iter()
        .map(|h| h.session_id.clone())
        .collect();
    assert_eq!(full.len(), 12);

    let mut paged = Vec::new();
    let mut cursor: Option<String> = None;
    loop {
        let page = search_page(&conn, "pageterm", 2, cursor.as_deref(), SortOrder::Newest).unwrap();
        for h in &page.hits {
            paged.push(h.session_id.clone());
        }
        match page.next_cursor {
            Some(c) => cursor = Some(c),
            None => break,
        }
    }
    assert_eq!(
        paged, full,
        "newest paging matches single-shot newest order"
    );
    let unique: std::collections::HashSet<_> = paged.iter().cloned().collect();
    assert_eq!(
        unique.len(),
        paged.len(),
        "no row repeats across newest pages"
    );
}

#[test]
fn oldest_sort_paginates_without_duplicates() {
    let conn = lore_core::storage::open_in_memory().unwrap();
    let (_bd, blobs) = store();
    // Ten timestamped sessions plus two null-start ones; page the oldest sort in
    // twos and require the reassembly to equal the single-shot oldest order.
    for i in 0..10 {
        codex_at(
            &conn,
            &blobs,
            &format!("cx{i}"),
            &format!("2026-08-11T10:00:{i:02}.000Z"),
            "oldestterm here",
        );
    }
    for i in 0..2 {
        persist_claude(
            &conn,
            &blobs,
            &user_message(&format!("cl{i}"), "/p", "oldestterm here"),
            &format!("cl{i}"),
        );
    }

    let full: Vec<String> = search_page(&conn, "oldestterm", 50, None, SortOrder::Oldest)
        .unwrap()
        .hits
        .iter()
        .map(|h| h.session_id.clone())
        .collect();
    assert_eq!(full.len(), 12);

    let mut paged = Vec::new();
    let mut cursor: Option<String> = None;
    loop {
        let page = search_page(&conn, "oldestterm", 2, cursor.as_deref(), SortOrder::Oldest).unwrap();
        for h in &page.hits {
            paged.push(h.session_id.clone());
        }
        match page.next_cursor {
            Some(c) => cursor = Some(c),
            None => break,
        }
    }
    assert_eq!(
        paged, full,
        "oldest paging matches single-shot oldest order"
    );
    let unique: std::collections::HashSet<_> = paged.iter().cloned().collect();
    assert_eq!(
        unique.len(),
        paged.len(),
        "no row repeats across oldest pages"
    );
}
