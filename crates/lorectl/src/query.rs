//! `lorectl search` and `lorectl inspect` — the read-only query surface.
//!
//! Both open the archive with [`storage::open_read_only`], so they are
//! structurally incapable of changing it: a stray write fails at the SQLite
//! layer rather than depending on this module behaving. Neither takes the
//! writer lock — a query must not be blocked by a running scan, and cannot
//! corrupt one.
//!
//! Blobs are opened with [`BlobStore::open_existing`], not `open`. The writer
//! constructor creates `blobs/tmp/` as a side effect, which would have a command
//! that prints a patch leave a directory behind in an archive it was only asked
//! to read — and would silently succeed against a directory that is not an
//! archive, reporting every patch as missing rather than saying so once.
//!
//! These are the first commands that can meet an archive which does not exist:
//! `scan` creates one, so exit code 2 was unreachable until now.

use std::io::Write;

use lore_core::paths::{ARCHIVE_DB_FILENAME, BLOBS_DIRNAME};
use lore_core::search::SortOrder;
use lore_core::storage::blob::BlobStore;
use lore_core::{query, search, storage};
use rusqlite::Connection;

use crate::cli::Invocation;
use crate::exit::{self, CliError};

/// How many hits one `search` prints. Bounded so a broad query cannot page the
/// whole archive into a terminal; `SearchPage` carries a cursor for later.
const SEARCH_LIMIT: i64 = 20;
// A compile-time check rather than a test: the bound is a property of the
// constant, so it should fail the build, not a test run.
const _: () = assert!(SEARCH_LIMIT > 0 && SEARCH_LIMIT <= 100);

/// Open the archive this invocation targets, for reading only.
fn open_reader(invocation: &Invocation) -> Result<(Connection, std::path::PathBuf), CliError> {
    let archive_dir = invocation.archive_dir()?;
    let conn = storage::open_read_only(&archive_dir.join(ARCHIVE_DB_FILENAME))?;
    Ok((conn, archive_dir))
}

/// `lorectl search <QUERY>` — full-text search across archived sessions.
pub fn search(invocation: &Invocation) -> Result<u8, CliError> {
    let query_text = invocation
        .operand
        .as_deref()
        .ok_or_else(|| CliError::Usage("`lorectl search` needs a QUERY".into()))?;
    let (conn, _dir) = open_reader(invocation)?;

    let page = search::search_page(&conn, query_text, SEARCH_LIMIT, None, SortOrder::Relevance)?;

    let mut stdout = std::io::stdout().lock();
    if invocation.json {
        for hit in &page.hits {
            let line = serde_json::json!({
                "session_id": hit.session_id,
                "agent_id": hit.agent_id,
                "title": hit.title,
                "started_at_ms": hit.started_at,
                "source_kind": hit.source_kind,
                "field": hit.field,
                "snippet": hit.snippet,
            });
            let _ = writeln!(stdout, "{line}");
        }
    } else if page.hits.is_empty() {
        // Says what was searched, not what exists. "No matches" is a fact about
        // this query against what has been archived — not evidence that the work
        // never happened.
        let _ = writeln!(stdout, "no matches in the archive for {query_text:?}");
    } else {
        for hit in &page.hits {
            let title = hit.title.as_deref().unwrap_or("(untitled)");
            let _ = writeln!(stdout, "{}  {}  {}", hit.session_id, hit.agent_id, title);
            let _ = writeln!(stdout, "    {}", hit.snippet.replace('\n', " "));
        }
        if page.next_cursor.is_some() {
            let _ = writeln!(stdout, "(more matches not shown)");
        }
    }
    Ok(exit::OK)
}

/// `lorectl inspect <SESSION_ID>` — show one archived session.
pub fn inspect(invocation: &Invocation) -> Result<u8, CliError> {
    let session_id = invocation
        .operand
        .as_deref()
        .ok_or_else(|| CliError::Usage("`lorectl inspect` needs a SESSION_ID".into()))?;
    let (conn, archive_dir) = open_reader(invocation)?;

    let Some(detail) = query::get_session(&conn, session_id)? else {
        // Not an error about the archive: the archive is fine and simply holds
        // no such session. Exit 1, because the argument was wrong.
        return Err(CliError::Usage(format!(
            "no session `{session_id}` in this archive"
        )));
    };

    let mut stdout = std::io::stdout().lock();
    if invocation.json {
        let line = serde_json::json!({
            "session_id": detail.summary.id,
            "agent_id": detail.summary.agent_id,
            "title": detail.summary.title,
            "started_at_ms": detail.summary.started_at,
            "ended_at_ms": detail.summary.ended_at,
            "message_count": detail.summary.message_count,
            "tool_call_count": detail.summary.tool_call_count,
            "parse_status": detail.summary.parse_status,
            "parse_note": detail.parse_note,
            "file_events": detail.file_events.len(),
        });
        let _ = writeln!(stdout, "{line}");
    } else {
        let title = detail.summary.title.as_deref().unwrap_or("(untitled)");
        let _ = writeln!(stdout, "session  {}", detail.summary.id);
        let _ = writeln!(stdout, "agent    {}", detail.summary.agent_id);
        let _ = writeln!(stdout, "title    {title}");
        let _ = writeln!(
            stdout,
            "messages {}  tool calls {}  files {}",
            detail.summary.message_count,
            detail.summary.tool_call_count,
            detail.file_events.len()
        );
        // Surfaced because a partial parse means the session is shown
        // incompletely, and a reader acting on it should know that.
        if detail.summary.parse_status != "ok" {
            let _ = writeln!(stdout, "parse    {}", detail.summary.parse_status);
            if let Some(note) = &detail.parse_note {
                let _ = writeln!(stdout, "         {note}");
            }
        }
    }

    if invocation.patch {
        print_patches(&conn, &archive_dir, &detail, &mut stdout)?;
    }
    Ok(exit::OK)
}

/// Print every recorded patch for a session, in file-event order.
fn print_patches(
    conn: &Connection,
    archive_dir: &std::path::Path,
    detail: &lore_ipc::SessionDetail,
    stdout: &mut impl Write,
) -> Result<(), CliError> {
    let blobs = BlobStore::open_existing(archive_dir.join(BLOBS_DIRNAME))?;
    for event in &detail.file_events {
        // `None` covers three different things — no payload recorded, invalid
        // UTF-8, and a quarantined blob whose scan never completed. They are
        // reported as one "unavailable" rather than guessed between, because
        // this layer cannot tell them apart and inventing a reason would be a
        // claim Lore cannot support.
        match query::file_patch_text(conn, &blobs, &event.id)? {
            Some(text) => {
                let _ = writeln!(stdout, "\n--- {} ({})", event.path, event.change_kind);
                let _ = write!(stdout, "{text}");
                if !text.ends_with('\n') {
                    let _ = writeln!(stdout);
                }
            }
            None => {
                let _ = writeln!(
                    stdout,
                    "\n--- {} ({}) — no patch text available",
                    event.path, event.change_kind
                );
            }
        }
    }
    Ok(())
}
