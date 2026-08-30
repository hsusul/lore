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
fn tool_filter_matches_invoked_tool_name() {
    let conn = lore_core::storage::open_in_memory().unwrap();
    let (_bd, blobs) = store();
    // A session whose assistant invoked Edit, and whose text says "the fix".
    let jsonl = format!(
        "{}{}",
        user_message("t1", "/p", "here is the fix"),
        "{\"type\":\"assistant\",\"uuid\":\"a1\",\"sessionId\":\"t1\",\"cwd\":\"/p\",\"message\":{\"role\":\"assistant\",\"content\":[{\"type\":\"tool_use\",\"id\":\"t1\",\"name\":\"Edit\",\"input\":{\"file_path\":\"auth/login.ts\"}}]}}\n"
    );
    persist_claude(&conn, &blobs, &jsonl, "t1");

    assert_eq!(search(&conn, "fix tool:Edit", 20).unwrap().len(), 1);
    assert!(search(&conn, "fix tool:Bash", 20).unwrap().is_empty());
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
        let page =
            search_page(&conn, "oldestterm", 2, cursor.as_deref(), SortOrder::Oldest).unwrap();
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

#[test]
fn search_page_with_only_zero_width_characters_returns_empty_page() {
    let conn = lore_core::storage::open_in_memory().unwrap();
    let (_bd, blobs) = store();
    persist_claude(
        &conn,
        &blobs,
        &user_message("c1", "/p", "some text content"),
        "c1",
    );

    let res = search_page(
        &conn,
        "\u{feff}\u{200b}\u{200c}\u{200d}\u{2060}",
        10,
        None,
        SortOrder::Relevance,
    )
    .unwrap();
    assert!(res.hits.is_empty());
    assert!(res.next_cursor.is_none());
}

#[test]
fn search_page_sanitizes_structured_filters_with_zero_width_chars() {
    let conn = lore_core::storage::open_in_memory().unwrap();
    let (_bd, blobs) = store();
    let id = persist_claude(
        &conn,
        &blobs,
        &user_message("c1", "/p", "feature verification logic"),
        "c1",
    );

    // Agent filter containing zero-width characters should be sanitized and match
    let res = search(&conn, "agent:\u{200b}claude-code\u{200c} verification", 10).unwrap();
    assert_eq!(res.len(), 1);
    assert_eq!(res[0].session_id, id);

    // Purely zero-width agent filter is ignored and falls back to term match
    let res_fallback = search(&conn, "agent:\u{200b}\u{200c} verification", 10).unwrap();
    assert_eq!(res_fallback.len(), 1);
    assert_eq!(res_fallback[0].session_id, id);

    // Search terms containing control characters are sanitized and matched
    let res_ctrl = search(&conn, "ver\u{0007}ifi\u{001f}cation", 10).unwrap();
    assert_eq!(res_ctrl.len(), 1);
    assert_eq!(res_ctrl[0].session_id, id);
}

// ── Git-dimension filters (I1, migration 0011) ───────────────────────────────

/// A Codex session recording `branch`/`commit` in `session_meta.git`, so the
/// evidence lands as `agent_recorded` without needing a real repository.
fn codex_recorded(session: &str, cwd: &str, branch: &str, commit: &str, text: &str) -> String {
    format!(
        concat!(
            "{{\"type\":\"session_meta\",\"timestamp\":\"2026-08-11T10:00:00.000Z\",\"payload\":{{\"id\":\"{id}\",\"cli_version\":\"1\",\"cwd\":\"{cwd}\",\"model_provider\":\"openai\",\"git\":{{\"branch\":\"{branch}\",\"commit_hash\":\"{commit}\"}}}}}}\n",
            "{{\"type\":\"response_item\",\"timestamp\":\"2026-08-11T10:00:01.000Z\",\"payload\":{{\"type\":\"message\",\"role\":\"user\",\"content\":\"{text}\"}}}}\n"
        ),
        id = session, cwd = cwd, branch = branch, commit = commit, text = text
    )
}

fn persist_codex(conn: &Connection, blobs: &BlobStore, jsonl: &str, dedupe: &str) -> String {
    let parsed = CodexAdapter::new().parse_str(jsonl, dedupe);
    persist_session(conn, "codex", "Codex", &parsed, blobs).unwrap()
}

#[test]
fn branch_filter_matches_the_recorded_branch() {
    let conn = lore_core::storage::open_in_memory().unwrap();
    let (_bd, blobs) = store();
    let sha = "0123456789abcdef0123456789abcdef01234567";
    persist_codex(
        &conn,
        &blobs,
        &codex_recorded("g1", "/p", "billing", sha, "webhook signature mismatch"),
        "g1",
    );
    persist_codex(
        &conn,
        &blobs,
        &codex_recorded("g2", "/p", "main", sha, "webhook signature mismatch"),
        "g2",
    );

    assert_eq!(search(&conn, "webhook", 20).unwrap().len(), 2);
    let billing = search(&conn, "webhook branch:billing", 20).unwrap();
    assert_eq!(billing.len(), 1, "only the billing session matches");
    assert_eq!(search(&conn, "webhook branch:nope", 20).unwrap().len(), 0);
}

#[test]
fn commit_filter_accepts_a_short_prefix_and_does_not_wildcard() {
    let conn = lore_core::storage::open_in_memory().unwrap();
    let (_bd, blobs) = store();
    let sha = "0123456789abcdef0123456789abcdef01234567";
    persist_codex(
        &conn,
        &blobs,
        &codex_recorded("g1", "/p", "main", sha, "signature mismatch"),
        "g1",
    );

    assert_eq!(
        search(&conn, "signature commit:01234567", 20)
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        search(&conn, &format!("signature commit:{sha}"), 20)
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        search(&conn, "signature commit:99999999", 20)
            .unwrap()
            .len(),
        0
    );
    // A LIKE wildcard in the value is escaped, not honoured.
    assert_eq!(search(&conn, "signature commit:%", 20).unwrap().len(), 0);
    assert_eq!(
        search(&conn, "signature commit:_1234567", 20)
            .unwrap()
            .len(),
        0
    );
}

#[test]
fn git_source_constrains_the_branch_it_is_paired_with() {
    // THE point of the search_git design: the agent recorded `billing`, and a
    // filter naming a *different* provenance class must not match it. A
    // flattened branch column could not express this.
    let conn = lore_core::storage::open_in_memory().unwrap();
    let (_bd, blobs) = store();
    let sha = "0123456789abcdef0123456789abcdef01234567";
    persist_codex(
        &conn,
        &blobs,
        &codex_recorded("g1", "/p", "billing", sha, "webhook mismatch"),
        "g1",
    );

    assert_eq!(
        search(
            &conn,
            "webhook branch:billing git-source:agent_recorded",
            20
        )
        .unwrap()
        .len(),
        1,
        "the agent did record this branch"
    );
    assert_eq!(
        search(&conn, "webhook branch:billing git-source:lore_captured", 20)
            .unwrap()
            .len(),
        0,
        "Lore never observed this branch itself — the classes must not mix"
    );
    assert_eq!(
        search(&conn, "webhook git-source:agent_recorded", 20)
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn an_unknown_git_source_is_ignored_rather_than_matching_nothing() {
    // A typo must not look like "no session has this evidence".
    let conn = lore_core::storage::open_in_memory().unwrap();
    let (_bd, blobs) = store();
    let sha = "0123456789abcdef0123456789abcdef01234567";
    persist_codex(
        &conn,
        &blobs,
        &codex_recorded("g1", "/p", "billing", sha, "webhook mismatch"),
        "g1",
    );
    assert_eq!(
        search(&conn, "webhook git-source:agent_recorderd", 20)
            .unwrap()
            .len(),
        1,
        "an unrecognized class is dropped, leaving the rest of the query intact"
    );
}

#[test]
fn git_filters_do_not_duplicate_a_session_with_many_observations() {
    // A semi-join must not fan out: one session with several observations is
    // still one hit per matching document.
    let conn = lore_core::storage::open_in_memory().unwrap();
    let (_bd, blobs) = store();
    let sha = "0123456789abcdef0123456789abcdef01234567";
    let sid = persist_codex(
        &conn,
        &blobs,
        &codex_recorded("g1", "/p", "billing", sha, "webhook mismatch"),
        "g1",
    );
    // Plant extra observations for the same session, as re-verification would.
    for (i, class) in ["lore_captured", "lore_reverified"].iter().enumerate() {
        conn.execute(
            "INSERT INTO git_observation
                (id, session_id, segment_id, source, observed_at, temporal_confidence, branch)
             SELECT 'extra' || ?2, ?1, s.id, ?3, 1, 'retrospective', 'billing'
             FROM session_segment s WHERE s.session_id = ?1 LIMIT 1",
            rusqlite::params![&sid, i as i64, class],
        )
        .unwrap();
    }
    lore_core::search::reproject_for_test(&conn, &sid).unwrap();

    let hits = search(&conn, "webhook branch:billing", 20).unwrap();
    assert_eq!(hits.len(), 1, "three observations, still one hit");
}

#[test]
fn repo_filter_matches_a_segment_with_no_observation_of_its_own() {
    // `segment_link` rows carry repository resolution even when no git
    // observation was recorded, so repo: still finds the session.
    let conn = lore_core::storage::open_in_memory().unwrap();
    let (_bd, blobs) = store();
    let sid = persist_claude(
        &conn,
        &blobs,
        &user_message("s1", "/p", "retryBackoff helper"),
        "s1",
    );
    conn.execute(
        "INSERT INTO repository (id, identity_key, display_name, identity_confidence,
                                 created_at, updated_at)
         VALUES ('repo_x', 'gcd:test', 'x', 'high', 0, 0)",
        [],
    )
    .unwrap();
    conn.execute(
        "UPDATE session_segment SET repository_id = 'repo_x' WHERE session_id = ?1",
        [&sid],
    )
    .unwrap();
    lore_core::search::reproject_for_test(&conn, &sid).unwrap();

    assert_eq!(
        search(&conn, "retryBackoff repo:repo_x", 20).unwrap().len(),
        1
    );
    assert_eq!(
        search(&conn, "retryBackoff repo:repo_y", 20).unwrap().len(),
        0
    );
    // …but a branch filter must not be satisfied by a link row's NULL branch.
    assert_eq!(
        search(&conn, "retryBackoff repo:repo_x branch:main", 20)
            .unwrap()
            .len(),
        0
    );
}

#[test]
fn before_and_after_date_filters_scope_search_results() {
    let conn = lore_core::storage::open_in_memory().unwrap();
    let (_bd, blobs) = store();

    let jsonl_early = "{\"type\":\"user\",\"uuid\":\"u_early\",\"sessionId\":\"s_early\",\"cwd\":\"/p\",\"message\":{\"role\":\"user\",\"content\":\"database connection pooling\"}}\n";
    let jsonl_late = "{\"type\":\"user\",\"uuid\":\"u_late\",\"sessionId\":\"s_late\",\"cwd\":\"/p\",\"message\":{\"role\":\"user\",\"content\":\"database connection retry\"}}\n";

    let sid_early = persist_claude(&conn, &blobs, jsonl_early, "s_early");
    let sid_late = persist_claude(&conn, &blobs, jsonl_late, "s_late");

    // s_early at 2026-08-01T10:00:00Z (epoch ms: 1785578400000)
    // s_late at 2026-08-20T10:00:00Z (epoch ms: 1787220000000)
    let early_ms = 1_785_578_400_000_i64;
    let late_ms = 1_787_220_000_000_i64;

    conn.execute(
        "UPDATE agent_session SET started_at = ?1 WHERE id = ?2",
        rusqlite::params![early_ms, sid_early],
    )
    .unwrap();
    conn.execute(
        "UPDATE search_document SET started_at = ?1 WHERE session_id = ?2",
        rusqlite::params![early_ms, sid_early],
    )
    .unwrap();

    conn.execute(
        "UPDATE agent_session SET started_at = ?1 WHERE id = ?2",
        rusqlite::params![late_ms, sid_late],
    )
    .unwrap();
    conn.execute(
        "UPDATE search_document SET started_at = ?1 WHERE session_id = ?2",
        rusqlite::params![late_ms, sid_late],
    )
    .unwrap();

    // Query with before filter
    let hits_before = search(&conn, "database before:2026-08-15", 20).unwrap();
    assert_eq!(hits_before.len(), 1);
    assert_eq!(hits_before[0].session_id, sid_early);

    // Query with after filter
    let hits_after = search(&conn, "database after:2026-08-15", 20).unwrap();
    assert_eq!(hits_after.len(), 1);
    assert_eq!(hits_after[0].session_id, sid_late);

    // Query with range (both after and before)
    let hits_range = search(&conn, "database after:2026-08-01 before:2026-08-10", 20).unwrap();
    assert_eq!(hits_range.len(), 1);
    assert_eq!(hits_range[0].session_id, sid_early);
}

#[test]
fn model_filter_matches_session_and_segment_models() {
    let conn = lore_core::storage::open_in_memory().unwrap();
    let (_bd, blobs) = store();

    let sid1 = persist_claude(
        &conn,
        &blobs,
        &user_message("m1", "/p", "optimize vector operations"),
        "m1",
    );
    let sid2 = persist_claude(
        &conn,
        &blobs,
        &user_message("m2", "/p", "optimize matrix multiplication"),
        "m2",
    );

    conn.execute(
        "UPDATE agent_session SET primary_model = 'claude-3-5-sonnet-20241022' WHERE id = ?1",
        [&sid1],
    )
    .unwrap();
    conn.execute(
        "UPDATE session_segment SET model = 'gpt-4o', provider = 'openai' WHERE session_id = ?1",
        [&sid2],
    )
    .unwrap();

    let hits_claude = search(&conn, "optimize model:claude-3-5", 20).unwrap();
    assert_eq!(hits_claude.len(), 1);
    assert_eq!(hits_claude[0].session_id, sid1);

    let hits_openai = search(&conn, "optimize model:openai", 20).unwrap();
    assert_eq!(hits_openai.len(), 1);
    assert_eq!(hits_openai[0].session_id, sid2);
}

#[test]
fn title_matches_are_ranked_higher_than_body_matches() {
    let conn = lore_core::storage::open_in_memory().unwrap();
    let (_bd, blobs) = store();

    // Session 1 has keyword in body text only
    let s1 = user_message("s1", "/p", "refactor auth token caching layer");
    let sid1 = persist_claude(&conn, &blobs, &s1, "s1");

    // Session 2 has keyword in its native title
    let s2 = format!(
        "{}{}",
        "{\"type\":\"custom-title\",\"sessionId\":\"s2\",\"customTitle\":\"Auth token refactoring\"}\n",
        user_message("s2", "/p", "general conversation without the keyword")
    );
    let sid2 = persist_claude(&conn, &blobs, &s2, "s2");

    let hits = search(&conn, "token", 20).unwrap();
    assert_eq!(hits.len(), 2);
    // Title hit should rank first
    assert_eq!(hits[0].session_id, sid2);
    assert_eq!(hits[0].field, "title");
    assert_eq!(hits[1].session_id, sid1);
    assert_eq!(hits[1].field, "text");
}

#[test]
fn has_patch_filter_scopes_search_results() {
    let conn = lore_core::storage::open_in_memory().unwrap();
    let (_bd, blobs) = store();

    // Session 1 has patch
    let patch_session = concat!(
        "{\"type\":\"session_meta\",\"payload\":{\"id\":\"s_patch\",\"cwd\":\"/p\",\"model_provider\":\"openai\"}}\n",
        "{\"type\":\"response_item\",\"payload\":{\"type\":\"message\",\"role\":\"user\",\"content\":\"refactor auth\"}}\n",
        "{\"type\":\"event_msg\",\"payload\":{\"type\":\"patch_apply_end\",\"call_id\":\"c1\",\"changes\":{\"auth.rs\":{\"type\":\"modify\",\"unified_diff\":\"+added auth logic\"}}}}\n"
    );
    let parsed1 = CodexAdapter::new().parse_str(patch_session, "s_patch");
    let sid1 = persist_session(&conn, "codex", "Codex", &parsed1, &blobs).unwrap();

    // Session 2 has no patch
    let no_patch_session = concat!(
        "{\"type\":\"session_meta\",\"payload\":{\"id\":\"s_nopatch\",\"cwd\":\"/p\",\"model_provider\":\"openai\"}}\n",
        "{\"type\":\"response_item\",\"payload\":{\"type\":\"message\",\"role\":\"user\",\"content\":\"refactor logging\"}}\n"
    );
    let parsed2 = CodexAdapter::new().parse_str(no_patch_session, "s_nopatch");
    let _sid2 = persist_session(&conn, "codex", "Codex", &parsed2, &blobs).unwrap();

    let hits_all = search(&conn, "refactor", 20).unwrap();
    assert_eq!(hits_all.len(), 2);

    let hits_patch = search(&conn, "refactor has:patch", 20).unwrap();
    assert_eq!(hits_patch.len(), 1);
    assert_eq!(hits_patch[0].session_id, sid1);
}
