//! The full-text index: a disposable cache of the vault at `.lore/index.db`.
//!
//! It is built only from parsed vault files, into a temporary file that is then
//! renamed over the old one, so it never holds anything the vault lacks and a
//! reader never sees half of it. Ranking is FTS5 BM25 with the title weighted
//! above the body, and the `## Timeline` section below both. A `lesson`
//! column holds the title and the body above its Receipts list again, with
//! no weight, so a search can ask that a word be in the lesson itself rather
//! than only in the person's quoted words.
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
const SCHEMA_VERSION: i64 = 3;
const SCHEMA: &str = "CREATE VIRTUAL TABLE notes USING fts5(
    path UNINDEXED, id UNINDEXED, slug UNINDEXED, scope UNINDEXED,
    note_type UNINDEXED, status UNINDEXED, title, body, timeline, lesson,
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
        conn.execute_batch(
            "PRAGMA journal_mode = OFF;
             PRAGMA temp_store = MEMORY;
             PRAGMA cache_size = -65536;",
        )?;
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

    /// An index held in memory with exactly `notes`, for a reader that must
    /// not write to the vault. It has no stamps and no fingerprint.
    pub fn in_memory<'a>(
        notes: impl IntoIterator<Item = (&'a str, &'a Note)>,
    ) -> Result<Self, IndexError> {
        let mut conn = Connection::open_in_memory()?;
        conn.execute_batch(SCHEMA)?;
        let tx = conn.transaction()?;
        {
            let mut insert = tx.prepare(INSERT)?;
            for (rel, note) in notes {
                insert.execute(row(rel, note))?;
            }
        }
        tx.commit()?;
        Ok(Self { conn })
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

    /// Active notes that are global or in `repo` (written `repo:<key>`)
    /// sharing at least two content words with `text`, or its only one, best
    /// first. See [`related_words`] for what counts as a content word.
    pub fn search_active_related(
        &self,
        text: &str,
        repo: Option<&str>,
        limit: usize,
    ) -> Result<Vec<Hit>, IndexError> {
        let words = related_words(text);
        let specific = self.specific(&words)?;
        let Some(fts) = fts_related(&words, &specific) else {
            return Ok(Vec::new());
        };
        let statement = self.conn.prepare_cached(&format!(
            "{SELECT_HITS} WHERE notes MATCH ?1 AND status = 'active'
               AND scope IN ('global', ?3) ORDER BY score, path LIMIT ?2"
        ))?;
        let limit = i64::try_from(limit).unwrap_or(i64::MAX);
        collect_hits(statement, params![fts, limit, repo.unwrap_or("global")])
    }

    /// The words of `words` that are not generic. A word is generic when
    /// more than half of the active notes hold it, in any scope, and more
    /// than [`SPECIFIC_NOTES`] do: a word most lessons share says nothing
    /// about which one a text is about.
    fn specific<'w>(&self, words: &'w [String]) -> Result<Vec<&'w str>, IndexError> {
        let total = self.conn.query_row(
            "SELECT count(*) FROM notes WHERE status = 'active'",
            [],
            |r| r.get::<_, i64>(0),
        )?;
        let mut held = self.conn.prepare_cached(
            "SELECT count(*) FROM notes WHERE notes MATCH ?1 AND status = 'active'",
        )?;
        let mut out = Vec::new();
        for word in words {
            let notes: i64 = held.query_row(params![format!("\"{word}\"")], |r| r.get(0))?;
            if notes <= SPECIFIC_NOTES || notes * 2 <= total {
                out.push(word.as_str());
            }
        }
        Ok(out)
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

    /// How many active notes apply in `repo` (or globally if `None`).
    pub fn count_active(&self, repo: Option<&str>) -> Result<usize, IndexError> {
        let mut statement = self.conn.prepare_cached(
            "SELECT count(*) FROM notes WHERE status = 'active' AND scope IN ('global', ?1)",
        )?;
        let count: i64 = statement.query_row(params![repo.unwrap_or("global")], |r| r.get(0))?;
        Ok(usize::try_from(count).unwrap_or_default())
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
        bm25(notes, 0, 0, 0, 0, 0, 0, 4.0, 1.0, 0.5, 0.0) AS score
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
    (path, id, slug, scope, note_type, status, title, body, timeline, lesson)
    VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)";
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
    let lesson = format!("{}\n{}", note.title, without_receipts(statement));
    (
        rel,
        note.id.as_str(),
        slug,
        note.scope.as_str(),
        note.note_type.as_str(),
        note.status.as_str(),
        note.title.as_str(),
        statement,
        timeline,
        lesson,
    )
}

/// `statement` above its `Receipts:` line, the list of the person's quoted
/// words the lore-lessons skill writes under a lesson. All of it when there
/// is no such line.
#[must_use]
pub fn without_receipts(statement: &str) -> &str {
    let mut offset = 0;
    for line in statement.split_inclusive('\n') {
        if line.trim().eq_ignore_ascii_case("receipts:") {
            return statement.get(..offset).unwrap_or(statement);
        }
        offset += line.len();
    }
    statement
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

/// Words that carry no topic of their own: function words, and the short
/// replies a person gives an agent mid-task.
const STOPWORDS: &[&str] = &[
    "a", "about", "after", "again", "all", "also", "am", "an", "and", "any", "are", "as", "at",
    "be", "been", "before", "but", "by", "can", "continue", "could", "did", "do", "does", "done",
    "for", "from", "go", "good", "great", "had", "has", "have", "he", "her", "here", "him", "his",
    "how", "i", "if", "in", "into", "is", "it", "its", "just", "let", "lets", "like", "me", "more",
    "my", "no", "nope", "not", "now", "of", "ok", "okay", "on", "one", "or", "our", "out",
    "please", "proceed", "same", "she", "should", "so", "some", "sure", "thank", "thanks", "that",
    "the", "their", "them", "then", "there", "these", "they", "this", "those", "to", "too", "up",
    "us", "very", "was", "we", "were", "what", "when", "where", "which", "who", "why", "will",
    "with", "would", "yeah", "yes", "yep", "you", "your",
];
/// At most this many content words of a text are searched for.
const RELATED_WORDS: usize = 12;

/// Endings that make a word a contraction such as "don't" or "we'll". A
/// contraction carries no topic, so it is not a content word.
const CONTRACTION_ENDINGS: &[&str] = &["n't", "'re", "'ve", "'ll", "'d", "'m"];

/// Contractions typed without their apostrophe, as people often do in chat
/// ("i dont know"). Like "don't", they carry no topic. Ones that are also
/// words of their own, such as "well", "ill", "id" and "wed", are left out.
const BARE_CONTRACTIONS: &[&str] = &[
    "aint", "arent", "cant", "couldnt", "didnt", "doesnt", "dont", "hadnt", "hasnt", "havent",
    "hes", "heres", "hows", "im", "isnt", "itll", "ive", "mustnt", "shes", "shouldnt", "thats",
    "theres", "theyll", "theyre", "theyve", "wasnt", "werent", "weve", "whats", "wheres", "whos",
    "wont", "wouldnt", "youd", "youll", "youre", "youve",
];

/// The content words of `text`, lowercased, each once, in order, at most
/// [`RELATED_WORDS`], after dropping a leading slash command such as
/// `/review`. Curly and straight apostrophes are the same. A contraction
/// such as "don't" is dropped whole, as is one typed without its apostrophe
/// ([`BARE_CONTRACTIONS`]), a possessive such as "repo's" counts as "repo",
/// and single letters and [`STOPWORDS`] are dropped. This is the one
/// rule for which words two texts share.
#[must_use]
pub fn related_words(text: &str) -> Vec<String> {
    let text = text.trim_start();
    let text = match text.strip_prefix('/') {
        Some(rest) => {
            let (cmd, after) = match rest.find(char::is_whitespace) {
                Some(i) => (&rest[..i], &rest[i..]),
                None => (rest, ""),
            };
            if !cmd.is_empty() && !cmd.contains('/') {
                after
            } else {
                text
            }
        }
        None => text,
    };
    let text = text
        .to_lowercase()
        .replace(['\u{2018}', '\u{2019}', '\u{02bc}'], "'");
    let mut words: Vec<String> = Vec::new();
    for token in text.split(|c: char| !(c.is_alphanumeric() || c == '\'')) {
        let token = token.trim_matches('\'');
        if CONTRACTION_ENDINGS.iter().any(|e| token.ends_with(e)) {
            continue;
        }
        let token = token.strip_suffix("'s").unwrap_or(token);
        for word in token.split('\'') {
            if word.chars().count() < 2
                || STOPWORDS.contains(&word)
                || BARE_CONTRACTIONS.contains(&word)
                || words.iter().any(|w| w == word)
            {
                continue;
            }
            words.push(word.to_owned());
            if words.len() == RELATED_WORDS {
                return words;
            }
        }
    }
    words
}

/// `word` with a common English plural or verb ending folded, so the forms of
/// one word compare equal: "filing" and "file" are "fil", "reverted" and
/// "revert" are "revert". A stem is kept at three or more letters.
#[must_use]
pub fn stem(word: &str) -> String {
    for ending in ["ing", "ed", "es", "s", "e"] {
        if let Some(stem) = word.strip_suffix(ending) {
            if stem.chars().count() >= 3 {
                return stem.to_owned();
            }
        }
    }
    word.to_owned()
}

/// A word held by at most this many notes is never generic, so a small vault
/// keeps every word.
const SPECIFIC_NOTES: i64 = 3;

/// An FTS5 query matching a note that holds any two of `words`, or the only
/// one, with at least one of `specific` in the lesson (the title, or the
/// body above its Receipts list). The person's quoted words can supply the
/// second word but never the only one: they are what was said in some other
/// session, often about something else. A generic word, one most notes
/// hold, can be the second word but never the only one. `None` when
/// nothing can match.
fn fts_related(words: &[String], specific: &[&str]) -> Option<String> {
    if specific.is_empty() {
        return None;
    }
    let quoted = |w: &str| format!("\"{w}\"");
    let in_lesson = format!(
        "lesson : ({})",
        specific
            .iter()
            .map(|w| quoted(w))
            .collect::<Vec<_>>()
            .join(" OR ")
    );
    let words: Vec<String> = words.iter().map(|w| quoted(w)).collect();
    if words.len() < 2 {
        return Some(in_lesson);
    }
    let mut pairs = Vec::new();
    for (i, a) in words.iter().enumerate() {
        for b in &words[i + 1..] {
            pairs.push(format!("({a} AND {b})"));
        }
    }
    Some(format!("({}) AND {in_lesson}", pairs.join(" OR ")))
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn related_words_drop_filler_and_a_leading_slash_command() {
        assert_eq!(
            related_words("Please fix THE flaky retry test, then fix it again"),
            ["fix", "flaky", "retry", "test"]
        );
        assert!(related_words("yes").is_empty());
        assert!(related_words("ok, continue!").is_empty());
        assert!(related_words("/compact").is_empty());
        assert_eq!(related_words("/review the migration"), ["migration"]);
        assert_eq!(
            related_words("/src/lib.rs needs a fix"),
            ["src", "lib", "rs", "needs", "fix"]
        );
        assert_eq!(related_words("/src/lib.rs"), ["src", "lib", "rs"]);
        let words = |text: &str| related_words(text);
        assert_eq!(
            fts_related(&words("retry"), &["retry"]).unwrap(),
            "lesson : (\"retry\")"
        );
        assert_eq!(
            fts_related(&words("retry flaky test"), &["retry", "flaky", "test"]).unwrap(),
            "((\"retry\" AND \"flaky\") OR (\"retry\" AND \"test\") OR (\"flaky\" AND \"test\")) \
             AND lesson : (\"retry\" OR \"flaky\" OR \"test\")"
        );
        // A generic word can be the second word, never the one in the lesson.
        assert_eq!(
            fts_related(&words("retry the test"), &["retry"]).unwrap(),
            "((\"retry\" AND \"test\")) AND lesson : (\"retry\")"
        );
        assert!(fts_related(&words("the test"), &[]).is_none());
    }

    #[test]
    fn related_words_drop_contractions_and_single_letters() {
        assert_eq!(related_words("don't add a changelog"), ["add", "changelog"]);
        assert_eq!(
            related_words("don\u{2019}t add a changelog"),
            ["add", "changelog"]
        );
        assert_eq!(
            related_words("we'll ship it, can't wait, I'm sure you've seen it"),
            ["ship", "wait", "seen"]
        );
        assert_eq!(related_words("it's the repo's config"), ["repo", "config"]);
        assert_eq!(
            related_words("the repo\u{2019}s config"),
            related_words("the repo's config")
        );
        assert_eq!(related_words("x y z build"), ["build"]);
        assert!(related_words("don't").is_empty());
        assert!(related_words("don't won't can't").is_empty());
        // Typed without the apostrophe, they are still not content words.
        assert_eq!(
            related_words("i dont know, thats the wrong branch im on"),
            ["know", "wrong", "branch"]
        );
        assert!(related_words("dont wont cant youre ive").is_empty());
        // Words that only look like a bare contraction are kept.
        assert_eq!(related_words("well, ill id"), ["well", "ill", "id"]);
    }
}
