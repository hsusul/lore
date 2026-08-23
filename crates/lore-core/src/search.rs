//! FTS5 search over the redacted `SearchDocument` projections (`SEARCH.md`).
//!
//! The FTS index is external-content over `SearchDocument.redacted_text` only,
//! so results and snippets are inherently secret-safe — a flagged span was
//! masked before it was ever indexed. The user query is parsed into plain terms
//! plus a few structured filters; plain terms are wrapped as FTS5 phrases and
//! bound as a parameter, so a malformed or hostile query can never become raw
//! SQL or FTS syntax. Ranking is BM25; matches are wrapped in highlight markers.

use lore_ipc::{SearchHit, SearchPage};
use rusqlite::types::Value;
use rusqlite::Connection;

use crate::storage::Result;

/// Marker inserted before a matched term in a snippet (private-use code point,
/// so it will not collide with indexed text). The UI splits on these.
pub const HIGHLIGHT_START: &str = "\u{e000}";
/// Marker inserted after a matched term in a snippet.
pub const HIGHLIGHT_END: &str = "\u{e001}";

/// Rebuild the `search_git` filter rows for one session from canonical
/// evidence (migration 0011).
///
/// Delete-then-insert per session, mirroring how `search_document` is
/// maintained: it is idempotent, it cannot leave a stale row behind when a
/// session is replaced, and it keeps the projection reconstructible from the
/// archive alone — no agent log is reread (`SEARCH.md` §5).
///
/// **Must be called inside the same transaction as the canonical rows it
/// projects.** Every write site that touches `git_observation` or a segment's
/// repository linkage calls it before committing: session replace in `ingest`,
/// `enrich_session`, and `write_outcomes`.
///
/// The `segment_link` class carries a segment's repository/worktree resolution
/// even when no observation was recorded for it, so `repo:` matches a session
/// whose repository is known but which produced no git evidence of its own.
pub(crate) fn project_session_git(tx: &Connection, session_id: &str) -> Result<()> {
    tx.execute("DELETE FROM search_git WHERE session_id = ?1", [session_id])?;
    tx.execute(
        "INSERT OR IGNORE INTO search_git
            (session_id, segment_id, repository_id, worktree_id, source_class,
             branch, commit_sha, observed_at)
         SELECT o.session_id, o.segment_id, s.repository_id, s.worktree_id,
                o.source, o.branch, o.commit_sha, o.observed_at
         FROM git_observation o
         LEFT JOIN session_segment s ON s.id = o.segment_id
         WHERE o.session_id = ?1",
        [session_id],
    )?;
    tx.execute(
        "INSERT OR IGNORE INTO search_git
            (session_id, segment_id, repository_id, worktree_id, source_class,
             branch, commit_sha, observed_at)
         SELECT s.session_id, s.id, s.repository_id, s.worktree_id,
                'segment_link', NULL, NULL, 0
         FROM session_segment s
         WHERE s.session_id = ?1 AND s.repository_id IS NOT NULL",
        [session_id],
    )?;
    Ok(())
}

/// Test-only re-projection hook: rebuild one session's `search_git` rows after
/// a test has planted evidence directly. Production code calls
/// [`project_session_git`] inside the transaction that wrote the evidence.
#[doc(hidden)]
pub fn reproject_for_test(conn: &Connection, session_id: &str) -> Result<()> {
    project_session_git(conn, session_id)
}

/// Caps to keep pathological queries bounded.
const MAX_QUERY_LEN: usize = 512;
const MAX_TERMS: usize = 16;
const MAX_LIMIT: i64 = 200;

/// Result ordering for [`search_page`]. `Relevance` is BM25 best-first (recency
/// then id as a stable tie-break); `Newest`/`Oldest` order by session start
/// time. Sessions with no start timestamp sort last in every mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SortOrder {
    #[default]
    Relevance,
    Newest,
    Oldest,
}

impl SortOrder {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Relevance => "relevance",
            Self::Newest => "newest",
            Self::Oldest => "oldest",
        }
    }

    /// Parse the wire value (`"newest"` / `"oldest"`); anything else — including
    /// `None` or an unknown string — is `Relevance`.
    #[must_use]
    pub fn parse(s: Option<&str>) -> Self {
        match s {
            Some("newest") => Self::Newest,
            Some("oldest") => Self::Oldest,
            _ => Self::Relevance,
        }
    }
}

impl std::fmt::Display for SortOrder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A parsed query: plain terms plus optional structured filters.
struct ParsedQuery {
    terms: Vec<String>,
    agent: Option<String>,
    path: Option<String>,
    tool: Option<String>,
    has_error: bool,
    /// Git-dimension filters, resolved against `search_git` (migration 0011).
    git: GitFilters,
}

/// Filters over provenance-labelled Git evidence.
///
/// `source_class` is a **constraint on the other three**, not a filter in its
/// own right: `branch:billing git-source:agent_recorded` must match only
/// sessions where *the agent recorded* that branch, never one where Lore merely
/// observed it later. Keeping them in one struct is what makes that expressible
/// — a flattened branch column could not distinguish the two
/// (`GIT_INTEGRATION.md` §1, §7).
#[derive(Default)]
struct GitFilters {
    repo: Option<String>,
    worktree: Option<String>,
    branch: Option<String>,
    commit: Option<String>,
    source_class: Option<String>,
}

impl GitFilters {
    fn is_empty(&self) -> bool {
        self.repo.is_none()
            && self.worktree.is_none()
            && self.branch.is_none()
            && self.commit.is_none()
            && self.source_class.is_none()
    }
}

/// The provenance classes a `git-source:` filter may name. An unrecognised
/// value is rejected at parse time rather than silently matching nothing, so a
/// typo cannot look like "no sessions have this".
const GIT_SOURCE_CLASSES: &[&str] = &["agent_recorded", "lore_captured", "lore_reverified"];

/// Search the archive, returning the first page only. `raw` is the user's query
/// (plain terms plus optional `agent:`, `path:`, `tool:`, and `has:error`
/// filters); returns up to `limit` hits ranked best-first. An empty term set
/// yields no results. Convenience wrapper over [`search_page`] for callers that
/// do not paginate.
pub fn search(conn: &Connection, raw: &str, limit: i64) -> Result<Vec<SearchHit>> {
    Ok(search_page(conn, raw, limit, None, SortOrder::Relevance)?.hits)
}

/// Search the archive with stable keyset pagination (`SEARCH.md` §6) in the
/// requested [`SortOrder`]. Each mode is a total order (relevance falls back to
/// recency then id; newest/oldest fall back to id, with null-start sessions
/// last), so paging never drops or repeats a row. Pass `cursor = None` for the
/// first page; on each result, if `next_cursor` is `Some`, pass it back verbatim
/// to fetch the next page. A cursor is only meaningful for the identical query
/// **and sort** that produced it; a malformed cursor degrades to the first page
/// rather than erroring.
pub fn search_page(
    conn: &Connection,
    raw: &str,
    limit: i64,
    cursor: Option<&str>,
    sort: SortOrder,
) -> Result<SearchPage> {
    let query = parse_query(raw);
    let Some(match_expr) = fts_match(&query.terms) else {
        return Ok(SearchPage {
            hits: Vec::new(),
            next_cursor: None,
        });
    };
    let limit = limit.clamp(1, MAX_LIMIT);
    let cursor = cursor.and_then(Cursor::decode);

    // Rank and page the candidates on (search_fts + search_document) ALONE, then
    // join agent_session only for the page's rows. `started_at` and `agent_id`
    // are denormalized onto search_document (migration 0007), so the sort key and
    // the agent filter no longer force a join over every match — cutting the
    // worst-case common-term query ~40% at scale (SEARCH.md §6). The FTS
    // auxiliary functions (bm25/snippet) are only valid alongside the MATCH, so
    // they live in the innermost `m` subquery; `p` applies the keyset predicate,
    // ordering, and LIMIT on the exposed `rank`/`sa`/`did` aliases (before the
    // join); the outer query joins `title` and re-sorts the page. Params are
    // appended in SQL text order.
    let mut sql = String::with_capacity(1024);
    sql.push_str(
        "SELECT p.session_id, p.source_kind, p.source_id, p.field, p.snip, p.rank,
                s.title AS title, p.agent_id, p.sa, p.did
         FROM (
           SELECT * FROM (
             SELECT sd.session_id AS session_id, sd.source_kind AS source_kind,
                    sd.source_id AS source_id, sd.field AS field,
                    snippet(search_fts, 0, ?, ?, '…', 12) AS snip,
                    bm25(search_fts) AS rank,
                    sd.agent_id AS agent_id, sd.started_at AS sa, sd.id AS did
             FROM search_fts
             JOIN search_document sd ON sd.id = search_fts.rowid
             WHERE search_fts MATCH ?",
    );
    let mut params: Vec<Value> = Vec::with_capacity(16);
    params.push(Value::Text(HIGHLIGHT_START.to_string()));
    params.push(Value::Text(HIGHLIGHT_END.to_string()));
    params.push(Value::Text(match_expr));

    if let Some(agent) = query.agent {
        sql.push_str(" AND sd.agent_id = ?");
        params.push(Value::Text(agent));
    }
    if query.has_error {
        sql.push_str(
            " AND EXISTS (SELECT 1 FROM tool_call tc
                          WHERE tc.session_id = sd.session_id AND tc.is_error = 1)",
        );
    }
    if let Some(path) = query.path {
        sql.push_str(
            " AND EXISTS (SELECT 1 FROM file_event fe
                          WHERE fe.session_id = sd.session_id AND fe.path LIKE ?)",
        );
        params.push(Value::Text(format!("{path}%")));
    }
    if let Some(tool) = query.tool {
        sql.push_str(
            " AND EXISTS (SELECT 1 FROM tool_call tc
                          WHERE tc.session_id = sd.session_id AND tc.name = ?)",
        );
        params.push(Value::Text(tool));
    }
    // Git-dimension filters (migration 0011). ONE semi-join carries all of them,
    // which is the whole point: `branch:billing git-source:agent_recorded` must
    // be satisfied by a SINGLE search_git row, so the branch and the provenance
    // class have to be true of the same piece of evidence. Splitting them into
    // separate EXISTS clauses would match a session where the agent recorded
    // some branch and Lore separately observed `billing` — exactly the
    // conflation GIT_INTEGRATION.md §1 forbids. A semi-join also cannot fan out,
    // so a session with many observations still counts once.
    if !query.git.is_empty() {
        sql.push_str(" AND EXISTS (SELECT 1 FROM search_git g WHERE g.session_id = sd.session_id");
        if let Some(repo) = query.git.repo {
            sql.push_str(" AND g.repository_id = ?");
            params.push(Value::Text(repo));
        }
        if let Some(worktree) = query.git.worktree {
            sql.push_str(" AND g.worktree_id = ?");
            params.push(Value::Text(worktree));
        }
        if let Some(branch) = query.git.branch {
            sql.push_str(" AND g.branch = ?");
            params.push(Value::Text(branch));
        }
        if let Some(commit) = query.git.commit {
            // Prefix match so a short sha works, anchored so it cannot wildcard.
            sql.push_str(" AND g.commit_sha LIKE ? ESCAPE '\\'");
            params.push(Value::Text(format!("{}%", like_escape(&commit))));
        }
        if let Some(class) = query.git.source_class {
            sql.push_str(" AND g.source_class = ?");
            params.push(Value::Text(class));
        }
        // No guard is needed against the synthetic `segment_link` rows: they
        // carry NULL branch and NULL commit_sha, and `NULL = ?` is never true,
        // so they can only ever satisfy repo:/worktree: — which is exactly what
        // they exist for.
        sql.push(')');
    }
    sql.push_str(") m");

    // Keyset predicate: keep only rows strictly after the cursor in the chosen
    // total order. Applied in `p`, before the agent_session join.
    let (keyset_sql, keyset_params) = keyset(sort, &cursor);
    sql.push_str(&keyset_sql);
    params.extend(keyset_params);

    sql.push_str(match sort {
        // NULLs sort last in every mode: `sa DESC` already trails NULLs, and the
        // explicit `(sa IS NULL)` key does so for the recency sorts.
        SortOrder::Relevance => " ORDER BY rank, sa DESC, did LIMIT ?",
        SortOrder::Newest => " ORDER BY (sa IS NULL), sa DESC, did LIMIT ?",
        SortOrder::Oldest => " ORDER BY (sa IS NULL), sa ASC, did LIMIT ?",
    });
    params.push(Value::Integer(limit));

    // Join the display title for just the page's rows, then re-establish the page
    // order (a join does not preserve the subquery's ordering).
    sql.push_str(") p JOIN agent_session s ON s.id = p.session_id");
    sql.push_str(match sort {
        SortOrder::Relevance => " ORDER BY p.rank, p.sa DESC, p.did",
        SortOrder::Newest => " ORDER BY (p.sa IS NULL), p.sa DESC, p.did",
        SortOrder::Oldest => " ORDER BY (p.sa IS NULL), p.sa ASC, p.did",
    });

    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt
        .query_map(rusqlite::params_from_iter(params), |row| {
            let hit = SearchHit {
                session_id: row.get(0)?,
                source_kind: row.get(1)?,
                source_id: row.get(2)?,
                field: row.get(3)?,
                snippet: row.get(4)?,
                rank: row.get(5)?,
                title: row.get(6)?,
                agent_id: row.get(7)?,
                started_at: row.get(8)?,
            };
            let did: i64 = row.get(9)?;
            Ok((hit, did))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;

    // Only advertise a cursor when the page was full: a short page is the end,
    // so there is nothing more to fetch.
    let next_cursor = (rows.len() as i64 == limit)
        .then(|| {
            rows.last()
                .map(|(hit, did)| Cursor::from_row(hit, *did).encode())
        })
        .flatten();
    let hits = rows.into_iter().map(|(hit, _)| hit).collect();
    Ok(SearchPage { hits, next_cursor })
}

/// Build the outer keyset predicate (with a leading ` WHERE `) and its bound
/// params for `sort` and `cursor`. Empty when there is no cursor. Each arm keeps
/// only rows strictly after the cursor in that sort's total order; `started_at`
/// is nullable and always sorts last, so the null block gets its own case.
fn keyset(sort: SortOrder, cursor: &Option<Cursor>) -> (String, Vec<Value>) {
    let Some(c) = cursor else {
        return (String::new(), Vec::new());
    };
    match sort {
        SortOrder::Relevance => match c.started_at {
            Some(sa) => (
                " WHERE rank > ?
                    OR (rank = ? AND sa < ?)
                    OR (rank = ? AND sa IS NULL)
                    OR (rank = ? AND sa = ? AND did > ?)"
                    .to_string(),
                vec![
                    Value::Real(c.rank),
                    Value::Real(c.rank),
                    Value::Integer(sa),
                    Value::Real(c.rank),
                    Value::Real(c.rank),
                    Value::Integer(sa),
                    Value::Integer(c.id),
                ],
            ),
            None => {
                // Already inside the trailing NULL-started_at block; no
                // real-timestamp row can follow it at the same rank.
                (
                    " WHERE rank > ?
                        OR (rank = ? AND sa IS NULL AND did > ?)"
                        .to_string(),
                    vec![
                        Value::Real(c.rank),
                        Value::Real(c.rank),
                        Value::Integer(c.id),
                    ],
                )
            }
        },
        SortOrder::Newest | SortOrder::Oldest => {
            // Non-null timestamps first (DESC for newest, ASC for oldest), then
            // the null-start block, tie-broken by id. `rank` is irrelevant here.
            let cmp = if sort == SortOrder::Newest { "<" } else { ">" };
            match c.started_at {
                Some(sa) => (
                    format!(
                        " WHERE sa IS NULL
                            OR (sa IS NOT NULL AND sa {cmp} ?)
                            OR (sa = ? AND did > ?)"
                    ),
                    vec![Value::Integer(sa), Value::Integer(sa), Value::Integer(c.id)],
                ),
                None => (
                    " WHERE sa IS NULL AND did > ?".to_string(),
                    vec![Value::Integer(c.id)],
                ),
            }
        }
    }
}

/// Opaque keyset cursor: the sort key of the last row on a page. Encoded so the
/// client can round-trip it without depending on its shape. `rank` is stored by
/// its exact bit pattern so the boundary comparison is not perturbed by decimal
/// rounding.
struct Cursor {
    rank: f64,
    started_at: Option<i64>,
    id: i64,
}

impl Cursor {
    fn from_row(hit: &SearchHit, id: i64) -> Self {
        Cursor {
            rank: hit.rank,
            started_at: hit.started_at,
            id,
        }
    }

    fn encode(&self) -> String {
        use std::fmt::Write;
        let mut out = String::with_capacity(48);
        let _ = write!(out, "{:x}:", self.rank.to_bits());
        if let Some(sa) = self.started_at {
            let _ = write!(out, "{sa}");
        } else {
            out.push('n');
        }
        let _ = write!(out, ":{}", self.id);
        out
    }

    /// Parse a cursor produced by [`Cursor::encode`]. Any malformed input yields
    /// `None`, which the caller treats as "no cursor" (first page) — never a
    /// panic on untrusted client input.
    fn decode(s: &str) -> Option<Cursor> {
        if s.len() > 2_048 {
            return None;
        }
        let mut parts = s.split(':');
        let rank = f64::from_bits(u64::from_str_radix(parts.next()?, 16).ok()?);
        if !rank.is_finite() {
            return None;
        }
        let started_at = match parts.next()? {
            "n" => None,
            v => Some(v.parse::<i64>().ok()?),
        };
        let id = parts.next()?.parse::<i64>().ok()?;
        if id <= 0 || parts.next().is_some() {
            return None;
        }
        Some(Cursor {
            rank,
            started_at,
            id,
        })
    }
}

fn truncate_to_char_boundary(s: &str, max_bytes: usize) -> &str {
    if s.len() <= max_bytes {
        s
    } else {
        let mut idx = max_bytes;
        while !s.is_char_boundary(idx) {
            idx -= 1;
        }
        &s[..idx]
    }
}

fn parse_query(raw: &str) -> ParsedQuery {
    let bounded = truncate_to_char_boundary(raw, MAX_QUERY_LEN);
    let mut terms = Vec::new();
    let mut agent = None;
    let mut path = None;
    let mut tool = None;
    let mut has_error = false;
    let mut git = GitFilters::default();
    for token in bounded.split_whitespace() {
        if let Some(value) = token.strip_prefix("agent:") {
            let clean: String = value
                .chars()
                .filter(|&c| !c.is_control() && !crate::is_zero_width(c))
                .collect();
            if !clean.is_empty() {
                agent = Some(clean);
            }
        } else if let Some(value) = token.strip_prefix("path:") {
            let clean: String = value
                .chars()
                .filter(|&c| !c.is_control() && !crate::is_zero_width(c))
                .collect();
            if !clean.is_empty() {
                path = Some(clean);
            }
        } else if let Some(value) = token.strip_prefix("tool:") {
            let clean: String = value
                .chars()
                .filter(|&c| !c.is_control() && !crate::is_zero_width(c))
                .collect();
            if !clean.is_empty() {
                tool = Some(clean);
            }
        } else if let Some(value) = token.strip_prefix("repo:") {
            git.repo = clean_filter(value);
        } else if let Some(value) = token.strip_prefix("worktree:") {
            git.worktree = clean_filter(value);
        } else if let Some(value) = token.strip_prefix("branch:") {
            git.branch = clean_filter(value);
        } else if let Some(value) = token.strip_prefix("commit:") {
            // Commit shas are matched by prefix, so normalise case here rather
            // than forcing every caller to type the full 40 hex characters.
            git.commit = clean_filter(value).map(|v| v.to_ascii_lowercase());
        } else if let Some(value) = token.strip_prefix("git-source:") {
            // An unknown class is dropped rather than applied, so a typo cannot
            // silently produce "no results" and read as "no such evidence".
            git.source_class =
                clean_filter(value).filter(|v| GIT_SOURCE_CLASSES.contains(&v.as_str()));
        } else if token == "has:error" {
            has_error = true;
        } else if terms.len() < MAX_TERMS {
            terms.push(token.to_string());
        }
    }
    ParsedQuery {
        terms,
        agent,
        path,
        tool,
        has_error,
        git,
    }
}

/// Escape LIKE wildcards so a user-supplied value cannot widen its own match.
fn like_escape(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_")
}

/// Strip control and zero-width characters from a filter value; `None` when
/// nothing printable survives, so `branch:` alone is ignored rather than
/// matching the empty string.
fn clean_filter(value: &str) -> Option<String> {
    let clean: String = value
        .chars()
        .filter(|&c| !c.is_control() && !crate::is_zero_width(c))
        .collect();
    (!clean.is_empty()).then_some(clean)
}

/// Build a safe FTS5 MATCH expression: each plain term becomes a quoted phrase
/// (internal quotes doubled), ANDed together. Quoting neutralizes FTS operators
/// so user input can never inject query syntax. `None` when there are no terms.
fn fts_match(terms: &[String]) -> Option<String> {
    if terms.is_empty() {
        return None;
    }
    let mut out = String::with_capacity(terms.len() * 16);
    let mut first = true;
    for term in terms {
        let clean: String = term
            .chars()
            .filter(|&c| !c.is_control() && !crate::is_zero_width(c))
            .collect();
        if clean.is_empty() {
            continue;
        }
        if !first {
            out.push(' ');
        }
        first = false;
        out.push('"');
        for ch in clean.chars() {
            if ch == '"' {
                out.push_str("\"\"");
            } else {
                out.push(ch);
            }
        }
        out.push('"');
    }
    if out.is_empty() {
        None
    } else {
        Some(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_terms_and_filters() {
        let q = parse_query("retry backoff agent:codex path:auth/ tool:Edit has:error");
        assert_eq!(q.terms, vec!["retry", "backoff"]);
        assert_eq!(q.agent.as_deref(), Some("codex"));
        assert_eq!(q.path.as_deref(), Some("auth/"));
        assert_eq!(q.tool.as_deref(), Some("Edit"));
        assert!(q.has_error);
    }

    #[test]
    fn parse_query_handles_empty_filters_and_unknown_prefixes() {
        // Empty filter values are ignored, unrecognized prefixes are treated as terms.
        let q = parse_query("agent: path: unknown:filter foo:bar");
        assert_eq!(q.agent, None);
        assert_eq!(q.path, None);
        assert!(!q.has_error);
        assert_eq!(q.terms, vec!["unknown:filter", "foo:bar"]);

        // Whitespace only
        let empty = parse_query("    \t\n  ");
        assert!(empty.terms.is_empty());
        assert_eq!(empty.agent, None);
        assert_eq!(empty.path, None);
        assert!(!empty.has_error);
    }

    #[test]
    fn fts_match_quotes_terms_and_neutralizes_operators() {
        // A hostile term with quotes/operators becomes a single safe phrase.
        let m = fts_match(&["foo\" OR bar".to_string()]).unwrap();
        assert_eq!(m, "\"foo\"\" OR bar\"");
        assert!(fts_match(&[]).is_none());

        // Null bytes and zero-width characters are stripped and empty terms are discarded.
        let m_clean = fts_match(&[
            "hello\0world".to_string(),
            "\0".to_string(),
            "\u{200b}\u{feff}".to_string(),
        ])
        .unwrap();
        assert_eq!(m_clean, "\"helloworld\"");
        assert!(fts_match(&["\0".to_string(), "\u{200b}".to_string()]).is_none());

        // Parentheses and boolean keywords become safe quoted phrases
        let m_bool = fts_match(&[
            "(auth".to_string(),
            "AND".to_string(),
            "session)".to_string(),
        ])
        .unwrap();
        assert_eq!(m_bool, "\"(auth\" \"AND\" \"session)\"");
    }

    #[test]
    fn term_cap_is_bounded() {
        let raw = (0..50)
            .map(|i| format!("t{i}"))
            .collect::<Vec<_>>()
            .join(" ");
        assert_eq!(parse_query(&raw).terms.len(), MAX_TERMS);
    }

    #[test]
    fn parse_query_truncates_multibyte_utf8_without_panicking() {
        // Construct a query of ASCII characters up to byte index 511, followed by
        // a 4-byte UTF-8 emoji spanning bytes 511..515. The slice must not split
        // the code point.
        let mut raw = "a".repeat(MAX_QUERY_LEN - 1);
        raw.push('🦀');
        let parsed = parse_query(&raw);
        assert_eq!(parsed.terms.len(), 1);
        assert_eq!(parsed.terms[0].len(), MAX_QUERY_LEN - 1);
    }

    #[test]
    fn cursor_round_trips_exactly() {
        for started_at in [Some(1_700_000_000_i64), None, Some(-5)] {
            let c = Cursor {
                rank: -3.5019287109375, // an exact f64 the bit pattern preserves
                started_at,
                id: 42,
            };
            let back = Cursor::decode(&c.encode()).expect("valid cursor decodes");
            assert_eq!(back.rank.to_bits(), c.rank.to_bits());
            assert_eq!(back.started_at, started_at);
            assert_eq!(back.id, 42);
        }
    }

    #[test]
    fn malformed_cursor_decodes_to_none() {
        for bad in [
            "",
            "garbage",
            "zz:1:2",
            "abc:notanint:2",
            "1:2",
            "1:2:3:4",
            "0:1:0",
            "0:1:-5",
            "7ff8000000000000:1:2", // NaN
            "7ff0000000000000:1:2", // +Infinity
            "fff0000000000000:1:2", // -Infinity
        ] {
            assert!(Cursor::decode(bad).is_none(), "{bad:?} should be rejected");
        }
        let oversized = "0:0:0".to_string() + &"x".repeat(3_000);
        assert!(
            Cursor::decode(&oversized).is_none(),
            "oversized cursor should be rejected"
        );
    }

    #[test]
    fn fts_match_filters_zero_width_and_null_terms() {
        let only_zero_width = vec!["\u{feff}\u{200b}\u{200c}\u{200d}\u{2060}".to_string()];
        assert_eq!(fts_match(&only_zero_width), None);

        let mixed = vec![
            "\u{200b}hello\u{200c}".to_string(),
            "\0\u{0007}\u{001f}".to_string(),
            "world".to_string(),
        ];
        assert_eq!(fts_match(&mixed), Some("\"hello\" \"world\"".to_string()));
    }

    #[test]
    fn sort_order_display_and_parse_roundtrip() {
        for order in [SortOrder::Relevance, SortOrder::Newest, SortOrder::Oldest] {
            assert_eq!(SortOrder::parse(Some(order.as_str())), order);
            assert_eq!(order.to_string(), order.as_str());
        }
        assert_eq!(SortOrder::parse(None), SortOrder::Relevance);
        assert_eq!(SortOrder::parse(Some("invalid")), SortOrder::Relevance);
    }

    #[test]
    fn parse_query_sanitizes_structured_filters() {
        let q1 = parse_query(
            "agent:\u{200b}codex\u{200c} path:\0src/lib.rs tool:\u{200b}Bash\u{200c} term",
        );
        assert_eq!(q1.agent.as_deref(), Some("codex"));
        assert_eq!(q1.path.as_deref(), Some("src/lib.rs"));
        assert_eq!(q1.tool.as_deref(), Some("Bash"));
        assert_eq!(q1.terms, vec!["term".to_string()]);

        let q2 = parse_query("agent:\u{200b} path:\0 has:error");
        assert_eq!(q2.agent, None);
        assert_eq!(q2.path, None);
        assert!(q2.has_error);
    }
}
