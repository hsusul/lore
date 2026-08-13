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

/// Search the archive, returning the first page only. `raw` is the user's query
/// (plain terms plus optional `agent:`, `path:`, and `has:error` filters);
/// returns up to `limit` hits ranked best-first. An empty term set yields no
/// results. Convenience wrapper over [`search_page`] for callers that do not
/// paginate.
pub fn search(conn: &Connection, raw: &str, limit: i64) -> Result<Vec<SearchHit>> {
    Ok(search_page(conn, raw, limit, None)?.hits)
}

/// Search the archive with stable keyset pagination (`SEARCH.md` §6). Ordering
/// is `bm25` ascending (best first), then `started_at` descending, then the
/// `search_document` id — a total order, so paging never drops or repeats a row.
/// Pass `cursor = None` for the first page; on each result, if `next_cursor` is
/// `Some`, pass it back verbatim to fetch the next page. A cursor is only
/// meaningful for the identical query that produced it; a malformed cursor
/// degrades to the first page rather than erroring.
pub fn search_page(
    conn: &Connection,
    raw: &str,
    limit: i64,
    cursor: Option<&str>,
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

    // Inner query computes the ranked, filtered candidate set; the FTS auxiliary
    // functions (bm25/snippet) are only valid alongside the MATCH, so they live
    // here. The outer query applies the keyset predicate and ordering on the
    // exposed `rank`/`sa`/`did` aliases. Params are appended in SQL text order.
    let mut sql = String::from(
        "SELECT session_id, source_kind, source_id, field, snip, rank,
                title, agent_id, sa, did
         FROM (
           SELECT sd.session_id AS session_id, sd.source_kind AS source_kind,
                  sd.source_id AS source_id, sd.field AS field,
                  snippet(search_fts, 0, ?, ?, '…', 12) AS snip,
                  bm25(search_fts) AS rank,
                  s.title AS title, s.agent_id AS agent_id,
                  s.started_at AS sa, sd.id AS did
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
    sql.push(')');

    // Keyset predicate: keep only rows strictly after the cursor in the total
    // order (rank ASC, sa DESC with NULLs last, did ASC). `started_at` is
    // nullable, so the NULL block (which sorts after every real timestamp) needs
    // its own case.
    if let Some(c) = &cursor {
        match c.started_at {
            Some(sa) => {
                sql.push_str(
                    " WHERE rank > ?
                        OR (rank = ? AND sa < ?)
                        OR (rank = ? AND sa IS NULL)
                        OR (rank = ? AND sa = ? AND did > ?)",
                );
                let r = Value::Real(c.rank);
                params.extend([
                    r.clone(),
                    r.clone(),
                    Value::Integer(sa),
                    r.clone(),
                    r,
                    Value::Integer(sa),
                    Value::Integer(c.id),
                ]);
            }
            None => {
                // The cursor is already inside the trailing NULL-started_at
                // block; no real-timestamp row can follow it at the same rank.
                sql.push_str(
                    " WHERE rank > ?
                        OR (rank = ? AND sa IS NULL AND did > ?)",
                );
                let r = Value::Real(c.rank);
                params.extend([r.clone(), r, Value::Integer(c.id)]);
            }
        }
    }

    sql.push_str(" ORDER BY rank, sa DESC, did LIMIT ?");
    params.push(Value::Integer(limit));

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
        let sa = match self.started_at {
            Some(v) => v.to_string(),
            None => "n".to_string(),
        };
        format!("{:x}:{}:{}", self.rank.to_bits(), sa, self.id)
    }

    /// Parse a cursor produced by [`Cursor::encode`]. Any malformed input yields
    /// `None`, which the caller treats as "no cursor" (first page) — never a
    /// panic on untrusted client input.
    fn decode(s: &str) -> Option<Cursor> {
        let mut parts = s.split(':');
        let rank = f64::from_bits(u64::from_str_radix(parts.next()?, 16).ok()?);
        let started_at = match parts.next()? {
            "n" => None,
            v => Some(v.parse::<i64>().ok()?),
        };
        let id = parts.next()?.parse::<i64>().ok()?;
        if parts.next().is_some() {
            return None;
        }
        Some(Cursor {
            rank,
            started_at,
            id,
        })
    }
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
        for bad in ["", "garbage", "zz:1:2", "abc:notanint:2", "1:2", "1:2:3:4"] {
            assert!(Cursor::decode(bad).is_none(), "{bad:?} should be rejected");
        }
    }
}
