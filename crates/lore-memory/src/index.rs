//! The full-text index: a disposable cache of the vault at `.lore/index.db`.
//!
//! It is built only from parsed vault files, into a temporary file that is then
//! renamed over the old one, so it never holds anything the vault lacks and a
//! reader never sees half of it. Ranking is FTS5 BM25 with the title weighted
//! above the body, and the `## Timeline` section below both.
//!
//! The index also keeps a stamp (size and mtime) of every markdown file it
//! read, and a fingerprint of all of them. A person editing, renaming, or
//! deleting a note in Obsidian changes the fingerprint, and the vault then
//! re-reads only the files whose stamps changed.

use std::collections::hash_map::DefaultHasher;
use std::collections::HashMap;
use std::fs;
use std::hash::{Hash, Hasher};
use std::io;
use std::path::Path;

use rusqlite::{params, Connection, OpenFlags, OptionalExtension};
use thiserror::Error;

use crate::note::Note;

pub const INDEX_FILE: &str = "index.db";
const TEMP_FILE: &str = "index.db.tmp";
const SCHEMA_VERSION: i64 = 2;
const SCHEMA: &str = "CREATE VIRTUAL TABLE notes USING fts5(
    path UNINDEXED, id UNINDEXED, slug UNINDEXED, scope UNINDEXED,
    note_type UNINDEXED, status UNINDEXED, title, body, timeline,
    tokenize = 'porter unicode61'
);
CREATE TABLE files (
    path TEXT PRIMARY KEY, size INTEGER NOT NULL, mtime INTEGER NOT NULL
) WITHOUT ROWID;
CREATE TABLE meta (key TEXT PRIMARY KEY, value TEXT NOT NULL) WITHOUT ROWID;";

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
    /// The file name without `.md`.
    pub slug: String,
    pub scope: String,
    pub note_type: String,
    pub status: String,
    pub title: String,
    /// A one-line extract of the body above `## Timeline`, around the match.
    pub excerpt: String,
    /// FTS5 BM25. Lower is a better match.
    pub score: f64,
}

/// What the index remembers about one markdown file, to tell whether it
/// changed since it was read.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct FileStamp {
    /// Vault-relative path.
    pub path: String,
    pub size: u64,
    /// Modification time in nanoseconds since the Unix epoch.
    pub mtime_ns: i64,
}

/// One value for the whole set of stamps, compared before anything else so an
/// unchanged vault costs one directory walk and one row read.
#[must_use]
pub fn fingerprint(stamps: &[FileStamp]) -> String {
    let mut hasher = DefaultHasher::new();
    stamps.hash(&mut hasher);
    format!("{}:{:016x}", stamps.len(), hasher.finish())
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
        conn.busy_timeout(std::time::Duration::from_secs(5))?;
        let version: i64 = conn.pragma_query_value(None, "user_version", |row| row.get(0))?;
        Ok((version == SCHEMA_VERSION).then_some(Self { conn }))
    }

    /// Replace the index in `lore_dir` with one holding exactly `notes`, and
    /// `stamps` for every markdown file that was read, notes or not.
    pub fn build<'a>(
        lore_dir: &Path,
        notes: impl IntoIterator<Item = (&'a str, &'a Note)>,
        stamps: &[FileStamp],
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
            let mut stamp = tx.prepare(STAMP)?;
            for s in stamps {
                stamp.execute(stamp_row(s))?;
            }
            tx.execute(SET_FINGERPRINT, params![fingerprint(stamps)])?;
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

    /// The fingerprint stored with the stamps, if any.
    pub fn fingerprint(&self) -> Result<Option<String>, IndexError> {
        Ok(self
            .conn
            .query_row(
                "SELECT value FROM meta WHERE key = 'fingerprint'",
                [],
                |r| r.get(0),
            )
            .optional()?)
    }

    /// Every stored stamp, by path.
    pub fn stamps(&self) -> Result<HashMap<String, (u64, i64)>, IndexError> {
        let mut statement = self.conn.prepare("SELECT path, size, mtime FROM files")?;
        let stamps = statement
            .query_map([], |r| {
                let size: i64 = r.get(1)?;
                Ok((
                    r.get(0)?,
                    (u64::try_from(size).unwrap_or_default(), r.get(2)?),
                ))
            })?
            .collect::<Result<_, _>>()?;
        Ok(stamps)
    }

    /// Apply what changed on disk in one transaction: re-read files (with the
    /// note when the file is one), files that are gone, and the new
    /// fingerprint. A reader sees the index before or after, never between.
    pub fn refresh(
        &mut self,
        changed: &[(FileStamp, Option<Note>)],
        removed: &[String],
        fingerprint: &str,
    ) -> Result<(), IndexError> {
        let tx = self.conn.transaction()?;
        {
            let mut drop_note = tx.prepare("DELETE FROM notes WHERE path = ?1")?;
            let mut drop_file = tx.prepare("DELETE FROM files WHERE path = ?1")?;
            let mut insert = tx.prepare(INSERT)?;
            let mut stamp = tx.prepare(STAMP)?;
            for path in removed {
                drop_note.execute(params![path])?;
                drop_file.execute(params![path])?;
            }
            for (s, note) in changed {
                drop_note.execute(params![s.path])?;
                if let Some(note) = note {
                    insert.execute(row(&s.path, note))?;
                }
                stamp.execute(stamp_row(s))?;
            }
            tx.execute(SET_FINGERPRINT, params![fingerprint])?;
        }
        tx.commit()?;
        Ok(())
    }

    /// Notes matching every word of `query`, best first.
    pub fn search(&self, query: &str, limit: usize) -> Result<Vec<Hit>, IndexError> {
        let Some(fts) = fts_query(query) else {
            return Ok(Vec::new());
        };
        let statement = self.conn.prepare_cached(&format!(
            "{SELECT_HITS} WHERE notes MATCH ?1 ORDER BY score, path LIMIT ?2"
        ))?;
        let limit = i64::try_from(limit).unwrap_or(i64::MAX);
        collect_hits(statement, params![fts, limit])
    }

    /// Active notes that are global or in `repo` (written `repo:<key>`)
    /// matching every word of `query`, best first.
    pub fn search_active(
        &self,
        query: &str,
        repo: Option<&str>,
        limit: usize,
    ) -> Result<Vec<Hit>, IndexError> {
        let Some(fts) = fts_query(query) else {
            return Ok(Vec::new());
        };
        let statement = self.conn.prepare_cached(&format!(
            "{SELECT_HITS} WHERE notes MATCH ?1 AND status = 'active'
               AND scope IN ('global', ?3) ORDER BY score, path LIMIT ?2"
        ))?;
        let limit = i64::try_from(limit).unwrap_or(i64::MAX);
        collect_hits(statement, params![fts, limit, repo.unwrap_or("global")])
    }

    /// Active notes that are global or in `repo` (written `repo:<key>`), by
    /// path, with no query to match. The score is 0 and the excerpt is the
    /// start of the body above `## Timeline`.
    pub fn list_active(&self, repo: Option<&str>, limit: usize) -> Result<Vec<Hit>, IndexError> {
        let statement = self.conn.prepare_cached(
            "SELECT path, id, slug, scope, note_type, status, title, body, 0.0 FROM notes
               WHERE status = 'active' AND scope IN ('global', ?2) ORDER BY path LIMIT ?1",
        )?;
        let limit = i64::try_from(limit).unwrap_or(i64::MAX);
        let mut hits = collect_hits(statement, params![limit, repo.unwrap_or("global")])?;
        for hit in &mut hits {
            hit.excerpt = opening(&hit.excerpt);
        }
        Ok(hits)
    }

    /// Every active note, in any scope, by path.
    pub fn list_every_active(&self) -> Result<Vec<Hit>, IndexError> {
        let statement = self.conn.prepare_cached(
            "SELECT path, id, slug, scope, note_type, status, title, body, 0.0 FROM notes
               WHERE status = 'active' ORDER BY path",
        )?;
        let mut hits = collect_hits(statement, params![])?;
        for hit in &mut hits {
            hit.excerpt = opening(&hit.excerpt);
        }
        Ok(hits)
    }

    /// Paths of the notes whose id, slug (file name), or path is exactly `key`.
    pub fn lookup(&self, key: &str) -> Result<Vec<String>, IndexError> {
        let mut statement = self.conn.prepare_cached(
            "SELECT path FROM notes WHERE id = ?1 OR slug = ?1 OR path = ?1 ORDER BY path",
        )?;
        let paths = statement
            .query_map(params![key], |r| r.get(0))?
            .collect::<Result<_, _>>()?;
        Ok(paths)
    }

    /// Paths of the markdown files the index read but holds no note for,
    /// because they did not parse, sorted.
    pub fn unparsed(&self) -> Result<Vec<String>, IndexError> {
        let mut statement = self.conn.prepare_cached(
            "SELECT path FROM files WHERE path NOT IN (SELECT path FROM notes) ORDER BY path",
        )?;
        let paths = statement
            .query_map([], |r| r.get(0))?
            .collect::<Result<_, _>>()?;
        Ok(paths)
    }

    /// Paths of the notes with this id, sorted.
    pub fn paths_with_id(&self, id: &str) -> Result<Vec<String>, IndexError> {
        let mut statement = self
            .conn
            .prepare_cached("SELECT path FROM notes WHERE id = ?1 ORDER BY path")?;
        let paths = statement
            .query_map(params![id], |r| r.get(0))?
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

const SELECT_HITS: &str = "SELECT path, id, slug, scope, note_type, status, title,
        snippet(notes, 7, '', '', '...', 12),
        bm25(notes, 0, 0, 0, 0, 0, 0, 4.0, 1.0, 0.5) AS score
    FROM notes";

fn collect_hits(
    mut statement: rusqlite::CachedStatement<'_>,
    params: impl rusqlite::Params,
) -> Result<Vec<Hit>, IndexError> {
    let hits = statement
        .query_map(params, |r| {
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

const INSERT: &str = "INSERT INTO notes
    (path, id, slug, scope, note_type, status, title, body, timeline)
    VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)";
const STAMP: &str = "INSERT OR REPLACE INTO files (path, size, mtime) VALUES (?1, ?2, ?3)";
const SET_FINGERPRINT: &str = "INSERT OR REPLACE INTO meta (key, value) VALUES ('fingerprint', ?1)";

fn stamp_row(s: &FileStamp) -> impl rusqlite::Params + '_ {
    (
        s.path.as_str(),
        i64::try_from(s.size).unwrap_or(i64::MAX),
        s.mtime_ns,
    )
}

/// The slug is the file name stem, which is the title's slug unless the note
/// was accepted under another name with `mem accept --as`.
fn row<'a>(rel: &'a str, note: &'a Note) -> impl rusqlite::Params + 'a {
    let file = rel.rsplit('/').next().unwrap_or(rel);
    let slug = file.strip_suffix(".md").unwrap_or(file);
    let (statement, timeline) = split_timeline(&note.body);
    (
        rel,
        note.id.as_str(),
        slug,
        note.scope.to_string(),
        note.note_type.as_str(),
        note.status.as_str(),
        note.title.as_str(),
        statement,
        timeline,
    )
}

/// The body above the `## Timeline` heading, and the timeline itself.
#[must_use]
pub fn split_timeline(body: &str) -> (&str, &str) {
    const HEADING: &str = "## Timeline";
    let mut offset = 0;
    for line in body.split_inclusive('\n') {
        let trimmed = line.trim_end_matches(['\n', '\r']).trim_end();
        if trimmed == HEADING {
            return (
                body.get(..offset).unwrap_or(body),
                body.get(offset..).unwrap_or_default(),
            );
        }
        offset += line.len();
    }
    (body, "")
}

/// The first words of a body, cut the way `snippet` cuts an excerpt.
fn opening(body: &str) -> String {
    const WORDS: usize = 12;
    let mut words = body.split_whitespace();
    let start: Vec<&str> = words.by_ref().take(WORDS).collect();
    let more = if words.next().is_some() { "..." } else { "" };
    format!("{}{more}", start.join(" "))
}

/// Whether `query` has a word to search for. A query without one matches
/// nothing in [`Index::search_active`].
#[must_use]
pub fn has_words(query: &str) -> bool {
    fts_query(query).is_some()
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
