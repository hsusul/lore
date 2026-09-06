//! Helpers shared by the `lorectl` integration tests.
#![allow(dead_code, clippy::expect_used, clippy::unwrap_used)]

use std::path::{Path, PathBuf};

/// Every entry under `root`, relative and sorted, for asserting that a command
/// left an archive exactly as it found it.
///
/// SQLite's `-wal` / `-shm` sidecars are filtered out: they appear and vanish
/// with connections and say nothing about whether the archive's *contents*
/// changed, so including them would make the comparison flap.
#[must_use]
pub fn listing(root: &Path) -> Vec<String> {
    fn walk(dir: &Path, base: &Path, out: &mut Vec<String>) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            out.push(
                path.strip_prefix(base)
                    .unwrap()
                    .to_string_lossy()
                    .into_owned(),
            );
            if path.is_dir() {
                walk(&path, base, out);
            }
        }
    }
    let mut out = Vec::new();
    walk(root, root, &mut out);
    out.retain(|p| !p.ends_with("-wal") && !p.ends_with("-shm"));
    out.sort();
    out
}

/// Agent home directories that exist but are empty, so a scan finds nothing and
/// no real `~/.claude` history is ever read.
#[must_use]
pub fn empty_homes() -> tempfile::TempDir {
    let homes = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(homes.path().join("claude")).unwrap();
    std::fs::create_dir_all(homes.path().join("codex")).unwrap();
    homes
}

/// The archive database inside an archive directory.
#[must_use]
pub fn db_path(archive: &Path) -> PathBuf {
    archive.join(lore_core::paths::ARCHIVE_DB_FILENAME)
}
