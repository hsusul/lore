# Search

> Keyword/structured search first; semantic search only if measured need justifies it. Companions: `DATA_MODEL.md` (`SearchDocument`), `SECRET_SCANNING.md`, ADR-0004. Tags: **DECISION** / **OPINION**.

## 1. Staged plan

| Stage | Tech | Purpose |
|---|---|---|
| V0 | SQLite FTS5 over redacted SearchDocument projections | identifier/error/path recall plus exact filters |
| V0.5 | better ranking, saved searches, facets | repeated cross-repo workflows |
| V1, only if warranted | optional local FTS + embeddings hybrid | concept recall that keyword search demonstrably misses |

No embeddings in V0. Keywords and structured filters match how developers remember code; vectors add model/index/privacy complexity and are not a differentiator.

## 2. V0 storage design (DECISION)

Canonical MessagePart, ToolCall, FileEvent, and Blob rows are not themselves the FTS content table. Ingest builds one redacted projection per searchable field:

```text
canonical source
  → complete secret scan
  → SearchDocument{source_kind, source_id, field, redacted_text}
  → search_fts external-content row (same rowid)
```

`search_fts` uses external content from `SearchDocument` only. This is valid FTS5 external-content design and lets tool input/output and message parts share one search surface without pretending they belong to one base table.

Indexed projections:

- text MessageParts;
- thinking/reasoning only when the user explicitly opts in (off by default);
- recorded patch text after complete scanning/redaction;
- titles and safe file paths as separately weighted fields or joined facets.

*Status (per ROADMAP M6): message-part text, titles, and recorded patches are indexed today. Selected structured tool-input fields and tool output are scanned but not yet indexed — planned; tool payload JSON is explicitly `index=false`.*

Only **native** titles (agent `custom-title`/`ai-title` events) are indexed. A **synthetic** fallback title — derived from the first user message when the agent recorded no title — is kept in `agent_session.title` for display but is neither scanned nor projected: it is a verbatim echo of an already-indexed message, so indexing it would duplicate that message's search hits and secret findings (§6, "without duplicates").

Opaque/encrypted regions and scan-failed/quarantined blobs never produce SearchDocument rows.

## 3. Tokenization, ranking, and snippets

- Start with bundled `unicode61` plus token-character configuration validated against snake_case, camelCase, dotted identifiers, and paths. A truly custom tokenizer is a later implementation only if fixtures prove configuration insufficient.
- Rank with FTS5 BM25 (single weight over the whole projection today); `search_page` supports `Relevance` (BM25 with recency and id as stable tie-breaks), `Newest`, and `Oldest` sorts over keyset pagination — **implemented**. Additional fine-grained field weight boosts for user-authored text and exact matches remain planned.
- Use FTS5 `snippet()` with unique start/end markers and parse those markers into highlight ranges. Do **not** depend on FTS5 `offsets()`; it is unavailable for this context in the bundled SQLite probe and is unnecessary.
- Each result carries `source_kind/source_id/field`, so opening it navigates to the exact MessagePart/ToolCall/FileEvent even when the displayed projection was redacted.

## 4. Structured filters

Filters are indexed SQL predicates/joins, ANDed with FTS:

- agent, has-error, `path:` — **implemented**;
- repository, worktree, date, model, tool, Git evidence source (`agent_recorded`, `lore_captured`), branch/commit, and the context segment — **planned** (ROADMAP M6 remainder).

Query syntax is progressive: plain terms first, then tokens such as `repo:lore branch:billing git-source:recorded agent:codex tool:apply_patch path:auth/ has:error before:2026-07-01 "exact phrase"`. Unknown/malformed tokens remain searchable text or produce a local validation hint; they never become raw SQL.

## 5. Consistency and rebuilds

- Canonical upserts, SearchDocument projections, FTS rows, and the ingest checkpoint commit in one transaction.
- Deleting/replacing a source generation deletes its projections in the same transaction.
- Tokenizer/rule changes rebuild SearchDocument/FTS from Lore's canonical archive; they do not assume original agent logs still exist.
- Rebuild runs as a cancellable durable job into replacement tables, validates row counts/integrity, then swaps atomically. Existing search remains available until the swap.

## 6. Scale plan and acceptance gates

Target: typical queries under 200 ms at 10k sessions/~1M messages on the release reference laptop.

- Keyset pagination on stable `(rank bucket, started_at, id)` or `(started_at, id)` cursors; no deep OFFSET pagination. **Implemented:** `search::search_page` orders by the total key `(bm25, started_at DESC, search_document.id)` for relevance sorting and `(started_at, search_document.id)` for `Newest`/`Oldest` chronological sorts, paging via an opaque colon-delimited cursor (`next_cursor`); non-positive rowids or malformed/oversized cursors degrade safely to the first page. `search` is the first-page wrapper. The `started_at`-NULL block sorts last and has its own keyset case in all sort modes.
- Plain terms filter zero-width characters (`\u{200b}`, `\u{200c}`, `\u{200d}`, `\u{2060}`, `\u{feff}`) and null bytes, returning an empty result set if no printable search terms remain.
- **Limit before the join (DECISION).** `started_at` and `agent_id` are denormalized onto `search_document` (migration 0007), so the ranked page is computed and `LIMIT`ed on `search_fts` + `search_document` alone — sorting by the denormalized `started_at`, filtering `agent:` on the denormalized `agent_id`, and applying `has:error`/`path:` via `EXISTS` on `search_document.session_id`. `agent_session` is joined only for the page's display `title`, not for every match. Measured on a 250k-message corpus: worst-case common term −22%, `agent:`/`path:` filters −35–38%. The keys are session-stable, so re-ingest keeps them consistent; adding them carries no ingest index cost (no new index).
- The desktop search box coalesces rapid typing with a short local debounce before crossing the Tauri/SQLite boundary, so superseded keystrokes do not queue redundant FTS work. It clears results from the previous query immediately, reports the pending state accessibly, and still rejects any response superseded after dispatch.
- Indexed filter joins and segment/session denormalized rollups.
- Bounded query length, token count, prefix expansion, snippet size, result count, and cancellation deadline to prevent pathological FTS work.
- Search executes off the UI thread on read connections; ingest remains the only write queue.
- Benchmark both warm and cold cache, common and adversarial queries, index size, rebuild time, and concurrent ingest.

Passing M6 requires: identifier/path/error recall fixtures; correct provenance filters; stable pagination without duplicates; no raw planted secret in SearchDocument/FTS/snippets; and the reference performance report committed with hardware/SQLite version.

## 7. V1 hybrid search (conditional)

If query studies demonstrate meaningful misses, add optional local embeddings via an approved bundled model and `sqlite-vec`, with no model download at query time. Use reciprocal-rank fusion with FTS as the identifier backbone. The feature remains local, optional, resource-bounded, and requires its own model-distribution/security review.

## 8. Open questions (OPEN)

- Whether thinking/reasoning should ever be indexable; default stays off.
- Whether recorded patches deserve a dedicated rank boost after real relevance testing.
- Sharding is not planned at the V0 target; revisit only from measured 100k-session behavior.
