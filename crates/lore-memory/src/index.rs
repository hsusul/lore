//! The full-text index: a disposable cache of the vault at `.lore/index.db`.
//!
//! It is built only from parsed vault files, into a temporary file that is then
//! renamed over the old one, so it never holds anything the vault lacks and a
//! reader never sees half of it. Ranking is FTS5 BM25 with the title weighted
//! above the body.

use std::fs;
use std::io;
use std::path::Path;

use rusqlite::{params, Connection, OpenFlags};
use thiserror::Error;

use crate::note::Note;

pub const INDEX_FILE: &str = "index.db";
const TEMP_FILE: &str = "index.db.tmp";
const SCHEMA_VERSION: i64 = 1;
const SCHEMA: &str = "CREATE VIRTUAL TABLE notes USING fts5(
    path UNINDEXED, id UNINDEXED, slug UNINDEXED, scope UNINDEXED,
    note_type UNINDEXED, status UNINDEXED, title, body,
    tokenize = 'porter unicode61'
);";

#[derive(Debug, Error)]
pub enum IndexError {
    #[error("index: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("index: {0}")]
    Io(#[from] io::Error),
}

/// One search result.
#[derive(Debug, Clone, PartialEq)]
pub struct Hit {
    pub path: String,
    pub id: String,
    pub slug: String,
    pub scope: String,
    pub note_type: String,
    pub status: String,
    pub title: String,
    /// A one-line extract of the body around the match.
    pub excerpt: String,
    /// FTS5 BM25. Lower is a better match.
    pub score: f64,
}

#[derive(Debug)]
pub struct Index {
    conn: Connection,
}

impl Index {
    /// Open the index in `lore_dir`. `None` when there is no index, or it was
    /// written by a build with another schema; either way, rebuild it.
    pub fn open(lore_dir: &Path) -> Result<Option<Self>, IndexError> {
        let path = lore_dir.join(INDEX_FILE);
        if !path.exists() {
            return Ok(None);
        }
        let conn = Connection::open_with_flags(
            &path,
            OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;
        let version: i64 = conn.pragma_query_value(None, "user_version", |row| row.get(0))?;
        Ok((version == SCHEMA_VERSION).then_some(Self { conn }))
    }

    /// Replace the index in `lore_dir` with one holding exactly `notes`.
    pub fn build<'a>(
        lore_dir: &Path,
        notes: impl IntoIterator<Item = (&'a str, &'a Note)>,
    ) -> Result<usize, IndexError> {
        let temp = lore_dir.join(TEMP_FILE);
        match fs::remove_file(&temp) {
            Err(e) if e.kind() != io::ErrorKind::NotFound => return Err(e.into()),
            _ => {}
        }
        let mut conn = Connection::open(&temp)?;
        conn.execute_batch(SCHEMA)?;
        let tx = conn.transaction()?;
        let mut count = 0;
        {
            let mut insert = tx.prepare(INSERT)?;
            for (rel, note) in notes {
                insert.execute(row(rel, note))?;
                count += 1;
            }
        }
        tx.commit()?;
        conn.pragma_update(None, "user_version", SCHEMA_VERSION)?;
        drop(conn);
        fs::rename(&temp, lore_dir.join(INDEX_FILE))?;
        Ok(count)
    }

    /// Put one note in the index, replacing whatever it held for that path.
    pub fn upsert(&self, rel: &str, note: &Note) -> Result<(), IndexError> {
        self.conn
            .execute("DELETE FROM notes WHERE path = ?1", params![rel])?;
        self.conn.execute(INSERT, row(rel, note))?;
        Ok(())
    }

    /// Notes matching every word of `query`, best first.
    pub fn search(&self, query: &str, limit: usize) -> Result<Vec<Hit>, IndexError> {
        let Some(fts) = fts_query(query) else {
            return Ok(Vec::new());
        };
        let mut statement = self.conn.prepare_cached(
            "SELECT path, id, slug, scope, note_type, status, title,
                    snippet(notes, 7, '', '', '...', 12),
                    bm25(notes, 0, 0, 0, 0, 0, 0, 4.0, 1.0) AS score
             FROM notes WHERE notes MATCH ?1
             ORDER BY score, path LIMIT ?2",
        )?;
        let limit = i64::try_from(limit).unwrap_or(i64::MAX);
        let hits = statement
            .query_map(params![fts, limit], |r| {
                let excerpt: String = r.get(7)?;
                Ok(Hit {
                    path: r.get(0)?,
                    id: r.get(1)?,
                    slug: r.get(2)?,
                    scope: r.get(3)?,
                    note_type: r.get(4)?,
                    status: r.get(5)?,
                    title: r.get(6)?,
                    excerpt: excerpt.split_whitespace().collect::<Vec<_>>().join(" "),
                    score: r.get(8)?,
                })
            })?
            .collect::<Result<_, _>>()?;
        Ok(hits)
    }

    /// Paths of the notes whose id, slug, or path is exactly `key`.
    pub fn lookup(&self, key: &str) -> Result<Vec<String>, IndexError> {
        let mut statement = self.conn.prepare_cached(
            "SELECT path FROM notes WHERE id = ?1 OR slug = ?1 OR path = ?1 ORDER BY path",
        )?;
        let paths = statement
            .query_map(params![key], |r| r.get(0))?
            .collect::<Result<_, _>>()?;
        Ok(paths)
    }

    /// How many notes the index holds.
    pub fn len(&self) -> Result<usize, IndexError> {
        let count: i64 = self
            .conn
            .query_row("SELECT count(*) FROM notes", [], |r| r.get(0))?;
        Ok(usize::try_from(count).unwrap_or_default())
    }

    pub fn is_empty(&self) -> Result<bool, IndexError> {
        Ok(self.len()? == 0)
    }
}

const INSERT: &str = "INSERT INTO notes
    (path, id, slug, scope, note_type, status, title, body)
    VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)";

/// The slug is the file name stem, which is the title's slug unless the note
/// was accepted under another name with `mem accept --as`.
fn row<'a>(rel: &'a str, note: &'a Note) -> impl rusqlite::Params + 'a {
    let stem = rel.rsplit('/').next().and_then(|n| n.strip_suffix(".md"));
    (
        rel,
        note.id.as_str(),
        stem.map_or_else(|| note.slug(), str::to_string),
        note.scope.to_string(),
        note.note_type.as_str(),
        note.status.as_str(),
        note.title.as_str(),
        note.body.as_str(),
    )
}

/// Every word of the query as a quoted FTS5 term, so punctuation in a query is
/// never read as FTS5 syntax. `None` when the query has no words.
fn fts_query(query: &str) -> Option<String> {
    let terms: Vec<String> = query
        .split(|c: char| !c.is_alphanumeric())
        .filter(|t| !t.is_empty())
        .map(|t| format!("\"{t}\""))
        .collect();
    (!terms.is_empty()).then(|| terms.join(" "))
}
