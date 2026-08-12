//! FTS5 search over the redacted `SearchDocument` projections (`SEARCH.md`).
//!
//! The FTS index is external-content over `SearchDocument.redacted_text` only,
//! so results and snippets are inherently secret-safe — a flagged span was
//! masked before it was ever indexed. The user query is parsed into plain terms
//! plus a few structured filters; plain terms are wrapped as FTS5 phrases and
//! bound as a parameter, so a malformed or hostile query can never become raw
//! SQL or FTS syntax. Ranking is BM25; matches are wrapped in highlight markers.

use lore_ipc::SearchHit;
use rusqlite::types::Value;
use rusqlite::Connection;

use crate::storage::Result;

/// Marker inserted before a matched term in a snippet (private-use code point,
/// so it will not collide with indexed text). The UI splits on these.
pub const HIGHLIGHT_START: &str = "\u{e000}";
/// Marker inserted after a matched term in a snippet.
pub const HIGHLIGHT_END: &str = "\u{e001}";

/// Caps to keep pathological queries bounded.
const MAX_QUERY_LEN: usize = 512;
const MAX_TERMS: usize = 16;
const MAX_LIMIT: i64 = 200;

/// A parsed query: plain terms plus optional structured filters.
struct ParsedQuery {
    terms: Vec<String>,
    agent: Option<String>,
    path: Option<String>,
    has_error: bool,
}

/// Search the archive. `raw` is the user's query (plain terms plus optional
/// `agent:`, `path:`, and `has:error` filters); returns up to `limit` hits
/// ranked best-first. An empty term set yields no results.
pub fn search(conn: &Connection, raw: &str, limit: i64) -> Result<Vec<SearchHit>> {
    let query = parse_query(raw);
    let Some(match_expr) = fts_match(&query.terms) else {
        return Ok(Vec::new());
    };
    let limit = limit.clamp(1, MAX_LIMIT);

    // Params are appended in the same order they appear in the SQL text.
    let mut sql = String::from(
        "SELECT sd.session_id, sd.source_kind, sd.source_id, sd.field,
                snippet(search_fts, 0, ?, ?, '…', 12), bm25(search_fts),
                s.title, s.agent_id, s.started_at
         FROM search_fts
         JOIN search_document sd ON sd.id = search_fts.rowid
         JOIN agent_session s ON s.id = sd.session_id
         WHERE search_fts MATCH ?",
    );
    let mut params: Vec<Value> = vec![
        Value::Text(HIGHLIGHT_START.to_string()),
        Value::Text(HIGHLIGHT_END.to_string()),
        Value::Text(match_expr),
    ];

    if let Some(agent) = &query.agent {
        sql.push_str(" AND s.agent_id = ?");
        params.push(Value::Text(agent.clone()));
    }
    if query.has_error {
        sql.push_str(
            " AND EXISTS (SELECT 1 FROM tool_call tc
                          WHERE tc.session_id = s.id AND tc.is_error = 1)",
        );
    }
    if let Some(path) = &query.path {
        sql.push_str(
            " AND EXISTS (SELECT 1 FROM file_event fe
                          WHERE fe.session_id = s.id AND fe.path LIKE ?)",
        );
        params.push(Value::Text(format!("{path}%")));
    }
    // BM25 is ascending (lower is better); tie-break by recency then id so
    // pagination is stable.
    sql.push_str(" ORDER BY bm25(search_fts), s.started_at DESC, sd.id LIMIT ?");
    params.push(Value::Integer(limit));

    let mut stmt = conn.prepare(&sql)?;
    let hits = stmt
        .query_map(rusqlite::params_from_iter(params), |row| {
            Ok(SearchHit {
                session_id: row.get(0)?,
                source_kind: row.get(1)?,
                source_id: row.get(2)?,
                field: row.get(3)?,
                snippet: row.get(4)?,
                rank: row.get(5)?,
                title: row.get(6)?,
                agent_id: row.get(7)?,
                started_at: row.get(8)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(hits)
}

fn parse_query(raw: &str) -> ParsedQuery {
    let bounded = &raw[..raw.len().min(MAX_QUERY_LEN)];
    let mut terms = Vec::new();
    let mut agent = None;
    let mut path = None;
    let mut has_error = false;
    for token in bounded.split_whitespace() {
        if let Some(value) = token.strip_prefix("agent:") {
            if !value.is_empty() {
                agent = Some(value.to_string());
            }
        } else if let Some(value) = token.strip_prefix("path:") {
            if !value.is_empty() {
                path = Some(value.to_string());
            }
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
        has_error,
    }
}

/// Build a safe FTS5 MATCH expression: each plain term becomes a quoted phrase
/// (internal quotes doubled), ANDed together. Quoting neutralizes FTS operators
/// so user input can never inject query syntax. `None` when there are no terms.
fn fts_match(terms: &[String]) -> Option<String> {
    if terms.is_empty() {
        return None;
    }
    let quoted: Vec<String> = terms
        .iter()
        .map(|term| format!("\"{}\"", term.replace('"', "\"\"")))
        .collect();
    Some(quoted.join(" "))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_terms_and_filters() {
        let q = parse_query("retry backoff agent:codex path:auth/ has:error");
        assert_eq!(q.terms, vec!["retry", "backoff"]);
        assert_eq!(q.agent.as_deref(), Some("codex"));
        assert_eq!(q.path.as_deref(), Some("auth/"));
        assert!(q.has_error);
    }

    #[test]
    fn fts_match_quotes_terms_and_neutralizes_operators() {
        // A hostile term with quotes/operators becomes a single safe phrase.
        let m = fts_match(&["foo\" OR bar".to_string()]).unwrap();
        assert_eq!(m, "\"foo\"\" OR bar\"");
        assert!(fts_match(&[]).is_none());
    }

    #[test]
    fn term_cap_is_bounded() {
        let raw = (0..50)
            .map(|i| format!("t{i}"))
            .collect::<Vec<_>>()
            .join(" ");
        assert_eq!(parse_query(&raw).terms.len(), MAX_TERMS);
    }
}
