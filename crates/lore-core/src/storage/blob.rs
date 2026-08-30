//! Content-addressed, crash-safe blob store for large canonical payloads.
//!
//! Large recorded content — patch/diff payloads today, oversized tool I/O and
//! opaque encrypted regions later — is offloaded from SQLite rows into files
//! under the archive's `blobs/` directory and referenced by `blob.id`
//! (`docs/architecture/DATA_MODEL.md` §6, §9).
//!
//! Writes are crash-safe and follow the ingest contract: a blob is streamed to
//! a temporary file, fully flushed, then atomically renamed into its
//! content-addressed final path **before** the write transaction that
//! references it opens. A staged file whose referencing row never commits is a
//! self-healing orphan — the identical content re-ingested resolves to the same
//! path, and unreferenced files can be swept later. A reference is only created
//! inside the caller's transaction.
//!
//! ## Why the address must be a cryptographic digest
//!
//! The address is not merely a filename. `stage` treats an existing path as
//! proof that the identical bytes are already stored and skips the write, and
//! the `blob` row keyed by that address carries the `scan_state` gating
//! search/export. Two distinct payloads sharing an address therefore means the
//! second one's bytes are never written *and* it inherits the first one's
//! completed secret scan. Under the previous 64-bit FNV-1a address that
//! collision was constructible, not merely improbable, and session content is
//! attacker-influenceable (fetched pages, pasted material, files in a cloned
//! repo). Addresses are BLAKE3 (`ADDRESS_ALGO`), still prefixed by the byte
//! length. Pre-existing `fnv1a` rows stay readable — reads resolve
//! `storage_relpath`, which never changes — and are re-addressed lazily the next
//! time their source artifact is re-ingested; `blob.hash_algo` records which
//! algorithm produced each address (migration 0010).

use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use rusqlite::{params, Connection, OptionalExtension};

use super::{Result, StorageError};

/// Monotonic per-process counter making concurrent temp-file names unique
/// without a random or clock dependency.
static TEMP_SEQ: AtomicU64 = AtomicU64::new(0);

/// A blob whose bytes are already durably written to their final
/// content-addressed path, awaiting a transactional reference.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StagedBlob {
    content_hash: String,
    relpath: String,
    byte_len: i64,
}

impl StagedBlob {
    #[must_use]
    pub fn content_hash(&self) -> &str {
        &self.content_hash
    }

    #[must_use]
    pub fn relpath(&self) -> &str {
        &self.relpath
    }
}

/// A directory-rooted store of content-addressed blob files.
#[derive(Debug, Clone)]
pub struct BlobStore {
    root: PathBuf,
}

impl BlobStore {
    /// Open (creating if absent) a blob store rooted at `root`, ensuring the
    /// staging directory exists.
    pub fn open(root: impl Into<PathBuf>) -> Result<Self> {
        let root = root.into();
        fs::create_dir_all(root.join("tmp")).map_err(|_| StorageError::Io)?;
        Ok(Self { root })
    }

    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Durably write `bytes` to their content-addressed final path (idempotent;
    /// identical content resolves to the same path and is written at most once).
    /// Does not touch the database — call [`BlobStore::reference`] inside the
    /// write transaction to create the row.
    pub fn stage(&self, bytes: &[u8]) -> Result<StagedBlob> {
        let content_hash = content_hash(bytes);
        let shard = &content_hash[..2];
        let dir = self.root.join(shard);
        let final_path = dir.join(&content_hash);
        let relpath = format!("{shard}/{content_hash}");

        if !final_path.exists() {
            fs::create_dir_all(&dir).map_err(|_| StorageError::Io)?;
            let seq = TEMP_SEQ.fetch_add(1, Ordering::Relaxed);
            let tmp = self
                .root
                .join("tmp")
                .join(format!("{content_hash}.{seq}.tmp"));
            write_all_synced(&tmp, bytes)?;
            // Atomic within a filesystem. A concurrent writer may have won the
            // race and already produced the final file — that is fine, the
            // content is identical; discard our temp copy.
            match fs::rename(&tmp, &final_path) {
                Ok(()) => {}
                Err(_) if final_path.exists() => {
                    let _ = fs::remove_file(&tmp);
                }
                Err(_) => return Err(StorageError::Io),
            }
        }

        Ok(StagedBlob {
            content_hash,
            relpath,
            byte_len: i64::try_from(bytes.len()).unwrap_or(i64::MAX),
        })
    }

    /// Create (or reuse) the `blob` row for a staged blob inside the caller's
    /// transaction and return its id. `scan_state` starts `pending`: the blob is
    /// canonical local storage but is not searchable/exportable until scanned.
    pub fn reference(tx: &Connection, staged: &StagedBlob, media_type: &str) -> Result<String> {
        let id = blob_id(&staged.content_hash);
        tx.execute(
            "INSERT INTO blob
                (id, content_hash, media_type, byte_len, storage_relpath, scan_state,
                 hash_algo, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, 'pending', ?6, unixepoch('now') * 1000)
             ON CONFLICT(content_hash) DO NOTHING",
            params![
                id,
                staged.content_hash,
                media_type,
                staged.byte_len,
                staged.relpath,
                ADDRESS_ALGO,
            ],
        )?;
        // The id is a deterministic function of the content hash, so the row now
        // present (freshly inserted or pre-existing) carries exactly this id.
        let stored: Option<String> = tx
            .query_row(
                "SELECT id FROM blob WHERE content_hash = ?1",
                [&staged.content_hash],
                |row| row.get(0),
            )
            .optional()?;
        stored.ok_or(StorageError::Io)
    }

    /// Read a blob's bytes back by its `storage_relpath`. Reads are bounded by
    /// the file's own length.
    pub fn read(&self, relpath: &str) -> Result<Vec<u8>> {
        if !safe_relpath(relpath) {
            return Err(StorageError::Io);
        }
        fs::read(self.root.join(relpath)).map_err(|_| StorageError::Io)
    }

    /// Delete a blob file by `storage_relpath` (used when garbage-collecting an
    /// unreferenced blob during "forget"). A missing file is not an error.
    pub fn remove(&self, relpath: &str) -> Result<()> {
        if !safe_relpath(relpath) {
            return Err(StorageError::Io);
        }
        match fs::remove_file(self.root.join(relpath)) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(_) => Err(StorageError::Io),
        }
    }
}

/// A relpath is safe when no segment escapes the store root.
fn safe_relpath(relpath: &str) -> bool {
    let segments: Vec<&str> = relpath.split(['/', '\\']).collect();
    !segments.is_empty()
        && !segments
            .iter()
            .any(|seg| seg.is_empty() || *seg == ".." || *seg == "." || seg.contains(':'))
}

/// Write `bytes` to `path` and flush to disk before returning.
fn write_all_synced(path: &Path, bytes: &[u8]) -> Result<()> {
    let mut file = File::create(path).map_err(|_| StorageError::Io)?;
    file.write_all(bytes).map_err(|_| StorageError::Io)?;
    file.sync_all().map_err(|_| StorageError::Io)?;
    Ok(())
}

/// Content address: byte length followed by a BLAKE3 digest. The length prefix
/// is redundant against a cryptographic digest but is kept so an address remains
/// self-describing and cheaply sanity-checkable against the file on disk.
fn content_hash(bytes: &[u8]) -> String {
    format!(
        "{:016x}{}",
        bytes.len() as u64,
        blake3::hash(bytes).to_hex()
    )
}

/// Deterministic blob id derived from the content hash (blob ids are opaque; a
/// stable derivation keeps `reference` idempotent).
fn blob_id(content_hash: &str) -> String {
    format!("blob_{content_hash}")
}

/// The algorithm every address written by this build uses. Recorded on the row
/// so pre-0010 `fnv1a` addresses remain identifiable (and sweepable) while
/// staying readable.
const ADDRESS_ALGO: &str = "blake3";

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> (tempfile::TempDir, BlobStore) {
        let dir = tempfile::tempdir().unwrap();
        let store = BlobStore::open(dir.path()).unwrap();
        (dir, store)
    }

    #[test]
    fn stage_writes_content_addressed_file_read_back_faithfully() {
        let (_dir, store) = store();
        let staged = store.stage(b"@@ -1 +1 @@\n-old\n+new\n").unwrap();
        assert!(staged.relpath().starts_with(&staged.content_hash()[..2]));
        assert_eq!(
            store.read(staged.relpath()).unwrap(),
            b"@@ -1 +1 @@\n-old\n+new\n"
        );
    }

    #[test]
    fn identical_content_dedupes_to_one_file() {
        let (_dir, store) = store();
        let a = store.stage(b"same bytes").unwrap();
        let b = store.stage(b"same bytes").unwrap();
        assert_eq!(a, b);
    }

    #[test]
    fn length_disambiguates_content_hash() {
        // Same digest input length must not alias distinct content.
        let (_dir, store) = store();
        let a = store.stage(b"abc").unwrap();
        let b = store.stage(b"abcd").unwrap();
        assert_ne!(a.content_hash(), b.content_hash());
    }

    #[test]
    fn reference_is_idempotent_within_a_transaction() {
        let (_dir, store) = store();
        let conn = crate::storage::open_in_memory().unwrap();
        let staged = store.stage(b"diff payload").unwrap();

        let first = BlobStore::reference(&conn, &staged, "text/x-patch").unwrap();
        let second = BlobStore::reference(&conn, &staged, "text/x-patch").unwrap();
        assert_eq!(first, second);

        let rows: i64 = conn
            .query_row("SELECT count(*) FROM blob", [], |r| r.get(0))
            .unwrap();
        assert_eq!(
            rows, 1,
            "referencing the same content twice inserts one row"
        );

        let (hash, state, relpath): (String, String, String) = conn
            .query_row(
                "SELECT content_hash, scan_state, storage_relpath FROM blob WHERE id = ?1",
                [&first],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!(hash, staged.content_hash());
        assert_eq!(state, "pending", "unscanned blobs are not yet searchable");
        assert_eq!(relpath, staged.relpath());
    }

    #[test]
    fn address_is_a_labeled_cryptographic_digest() {
        // Regression: the address gates dedupe *and* inherits `scan_state`, so a
        // forgeable one lets colliding content ride another blob's clean scan.
        let (_dir, store) = store();
        let conn = crate::storage::open_in_memory().unwrap();
        let staged = store.stage(b"diff payload").unwrap();
        // 16 hex chars of length prefix + a 256-bit digest.
        assert_eq!(staged.content_hash().len(), 16 + 64);

        let id = BlobStore::reference(&conn, &staged, "text/x-patch").unwrap();
        let algo: String = conn
            .query_row("SELECT hash_algo FROM blob WHERE id = ?1", [&id], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(algo, "blake3");
    }

    #[test]
    fn legacy_fnv1a_addressed_blob_stays_readable() {
        // Migration 0010 is lazy: pre-existing rows keep their weak address and
        // must keep resolving, because reads go through `storage_relpath`.
        let (_dir, store) = store();
        let conn = crate::storage::open_in_memory().unwrap();
        let legacy_hash = "0000000000000004deadbeefdeadbeef";
        let relpath = format!("{}/{legacy_hash}", &legacy_hash[..2]);
        fs::create_dir_all(store.root().join(&legacy_hash[..2])).unwrap();
        fs::write(store.root().join(&relpath), b"old!").unwrap();
        conn.execute(
            "INSERT INTO blob
                (id, content_hash, media_type, byte_len, storage_relpath, scan_state, created_at)
             VALUES ('blob_legacy', ?1, 'text/x-patch', 4, ?2, 'clean', 0)",
            params![legacy_hash, relpath],
        )
        .unwrap();

        let (stored_relpath, algo): (String, String) = conn
            .query_row(
                "SELECT storage_relpath, hash_algo FROM blob WHERE id = 'blob_legacy'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(algo, "fnv1a", "pre-0010 rows default to the old algorithm");
        assert_eq!(store.read(&stored_relpath).unwrap(), b"old!");
    }

    #[test]
    fn read_rejects_path_traversal() {
        let (_dir, store) = store();
        assert!(store.read("../escape").is_err());
        assert!(store.read("..\\escape").is_err());
        assert!(store.read("shard/../../escape").is_err());
        assert!(store.read("C:\\escape").is_err());
        assert!(store.read("/escape").is_err());
        assert!(store.read("").is_err());
        assert!(store.read("./escape").is_err());
    }

    #[test]
    fn remove_deletes_blob_and_rejects_traversal() {
        let (_dir, store) = store();
        let staged = store.stage(b"temporary blob to remove").unwrap();
        assert_eq!(
            store.read(staged.relpath()).unwrap(),
            b"temporary blob to remove"
        );

        // Removal succeeds
        assert!(store.remove(staged.relpath()).is_ok());
        assert!(store.read(staged.relpath()).is_err());

        // Removing non-existent file is idempotent (Ok)
        assert!(store.remove(staged.relpath()).is_ok());

        // Traversal path in remove is rejected
        assert!(store.remove("../escape").is_err());
        assert!(store.remove("shard/../../escape").is_err());
        assert!(store.remove("").is_err());
    }

    #[test]
    fn stage_and_read_empty_and_multibyte_blobs() {
        let (_dir, store) = store();

        // Empty blob
        let empty_staged = store.stage(b"").unwrap();
        assert_eq!(empty_staged.byte_len, 0);
        assert!(empty_staged.content_hash().starts_with("0000000000000000"));
        assert_eq!(store.read(empty_staged.relpath()).unwrap(), b"");

        // Multibyte & binary blob
        let multibyte: &[u8] = b"\xf0\x9f\xa6\x80 \xe6\xbc\xa2\xe5\xad\x97 \x00 \xff";
        let mb_staged = store.stage(multibyte).unwrap();
        assert_eq!(mb_staged.byte_len, multibyte.len() as i64);
        assert_eq!(store.read(mb_staged.relpath()).unwrap(), multibyte);
    }

    #[test]
    fn safe_relpath_and_path_resolution_boundaries() {
        assert!(safe_relpath("ab/0123456789abcdef"));
        assert!(safe_relpath("00/0000000000000000af12"));
        assert!(!safe_relpath(""));
        assert!(!safe_relpath("/root/abs"));
        assert!(!safe_relpath("ab//cd"));
        assert!(!safe_relpath("ab/./cd"));
        assert!(!safe_relpath("ab/../cd"));
        assert!(!safe_relpath("C:/windows"));
        assert!(!safe_relpath("ab/cd:stream"));
    }

    #[test]
    fn blob_store_supports_non_ascii_root_paths() {
        let parent = tempfile::tempdir().unwrap();
        let unicode_root = parent.path().join("🦀_üñîçødé_目录_テスト");
        let store = BlobStore::open(&unicode_root).unwrap();
        let content = b"patch in unicode path root: \xf0\x9f\x94\xa7";
        let staged = store.stage(content).unwrap();
        assert_eq!(store.read(staged.relpath()).unwrap(), content);
        assert!(store.remove(staged.relpath()).is_ok());
        assert!(store.read(staged.relpath()).is_err());
    }

    #[test]
    fn zero_byte_blob_db_reference_and_deduplication() {
        let (_dir, store) = store();
        let conn = crate::storage::open_in_memory().unwrap();
        let empty_staged = store.stage(b"").unwrap();
        assert_eq!(empty_staged.byte_len, 0);

        let id1 = BlobStore::reference(&conn, &empty_staged, "text/plain").unwrap();
        let id2 = BlobStore::reference(&conn, &empty_staged, "text/plain").unwrap();
        assert_eq!(id1, id2);

        let (stored_len, state): (i64, String) = conn
            .query_row(
                "SELECT byte_len, scan_state FROM blob WHERE id = ?1",
                [&id1],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(stored_len, 0);
        assert_eq!(state, "pending");
    }

    #[test]
    fn read_missing_or_corrupted_relpath_returns_io_error() {
        let (_dir, store) = store();
        // Valid relpath format but file does not exist
        assert!(store.read("00/0000000000000000nonexistent").is_err());
        // Non-existent shard directory
        assert!(store.read("ff/ffffffffffffffffnonexistent").is_err());
    }

    #[test]
    fn blob_store_edge_cases_and_nonexistent_paths() {
        let (_dir, store) = store();

        // Reading nonexistent valid relpath returns Io error
        assert!(store.read("00/0000000000000000dead").is_err());

        // Reading invalid/traversal paths returns Io error
        assert!(matches!(
            store.read(""),
            Err(crate::storage::StorageError::Io)
        ));
        assert!(matches!(
            store.read("/absolute/path"),
            Err(crate::storage::StorageError::Io)
        ));
        assert!(matches!(
            store.read("../escape"),
            Err(crate::storage::StorageError::Io)
        ));

        // Staging multiple distinct payloads produces distinct content hashes and shard directories
        let a = store.stage(b"payload a").unwrap();
        let b = store.stage(b"payload b").unwrap();
        assert_ne!(a.content_hash(), b.content_hash());
        assert_ne!(a.relpath(), b.relpath());
        assert_eq!(store.read(a.relpath()).unwrap(), b"payload a");
        assert_eq!(store.read(b.relpath()).unwrap(), b"payload b");
    }

    #[test]
    fn blob_store_remove_safety_and_idempotence() {
        let (_dir, store) = store();

        let staged = store.stage(b"temporary blob content").unwrap();
        assert!(store.read(staged.relpath()).is_ok());

        // Removing existing blob -> Ok(()) and file is deleted
        store.remove(staged.relpath()).unwrap();
        assert!(store.read(staged.relpath()).is_err());

        // Removing already-deleted blob -> Ok(()) (idempotent)
        store.remove(staged.relpath()).unwrap();

        // Traversal / invalid relpath -> Err(StorageError::Io)
        assert!(matches!(
            store.remove("../escape"),
            Err(crate::storage::StorageError::Io)
        ));
        assert!(matches!(
            store.remove(""),
            Err(crate::storage::StorageError::Io)
        ));
    }

    #[test]
    fn staged_blob_getters_and_root_accessor() {
        let (dir, store) = store();
        assert_eq!(store.root(), dir.path());

        let staged = store.stage(b"hello world").unwrap();
        assert_eq!(staged.byte_len, 11);
        assert!(!staged.content_hash().is_empty());
        assert!(staged.relpath().ends_with(staged.content_hash()));

        let bid = blob_id(staged.content_hash());
        assert!(bid.starts_with("blob_"));
    }
}
