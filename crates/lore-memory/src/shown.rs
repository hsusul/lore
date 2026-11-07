//! Which notes an agent session has already been shown, so the hooks do not
//! repeat one within a session.
//!
//! One file per session at `<vault>/.lore/shown/<hash>`, where `<hash>` is
//! the BLAKE3 of the agent's session id cut to 32 hex digits, so the id
//! itself is not kept. The file holds one note id per line, appended by
//! each hook that shows notes. It is derived and gitignored with the rest of
//! `.lore/`, and only ever read by the hooks. Starting a new file deletes
//! the files no hook has written to for [`MAX_AGE`].
//!
//! Every operation is best effort: a record that cannot be read reads as
//! empty, and one that cannot be written is skipped. A missed record only
//! means a note may be shown twice.

use std::collections::HashSet;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::time::{Duration, SystemTime};

use crate::vault::Vault;

/// The folder under `.lore/` that holds the records.
pub const DIR: &str = "shown";
/// A record untouched this long belongs to a session that is over.
pub const MAX_AGE: Duration = Duration::from_secs(7 * 24 * 60 * 60);

/// The notes one session has been shown.
#[derive(Debug, Default)]
pub struct Shown {
    /// `None` when there is no session to record for.
    path: Option<PathBuf>,
    ids: HashSet<String>,
}

impl Shown {
    /// The record for `session_id` in `vault`. No session id gives an empty
    /// record that [`Shown::record`] never writes.
    #[must_use]
    pub fn open(vault: &Vault, session_id: Option<&str>) -> Self {
        let Some(id) = session_id.filter(|id| !id.is_empty()) else {
            return Self::default();
        };
        let hash = blake3::hash(id.as_bytes()).to_hex();
        let path = vault.lore_dir().join(DIR).join(&hash.as_str()[..32]);
        let ids = fs::read_to_string(&path)
            .map(|text| text.lines().map(str::to_owned).collect())
            .unwrap_or_default();
        Self {
            path: Some(path),
            ids,
        }
    }

    /// Whether the session was shown the note with this id.
    #[must_use]
    pub fn contains(&self, id: &str) -> bool {
        self.ids.contains(id)
    }

    /// Add these note ids to the record.
    pub fn record<'a>(&mut self, ids: impl IntoIterator<Item = &'a str>) {
        let new: Vec<&str> = ids
            .into_iter()
            .filter(|id| !id.contains('\n') && self.ids.insert((*id).to_owned()))
            .collect();
        let Some(path) = self.path.as_ref().filter(|_| !new.is_empty()) else {
            return;
        };
        let Some(dir) = path.parent() else {
            return;
        };
        let started = !path.exists();
        if fs::create_dir_all(dir).is_err() {
            return;
        }
        if started {
            prune(dir);
        }
        let text: String = new.iter().map(|id| format!("{id}\n")).collect();
        // One short write per hook, so concurrent hooks append whole lines.
        if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(path) {
            let _ = file.write_all(text.as_bytes());
        }
    }
}

/// Every session's record in `vault`: when a hook last wrote to it, and the
/// note ids in it. Read only; a record that cannot be read is left out. The
/// records hold a hash of the session id, so they cannot be joined to an
/// archived session, and records older than [`MAX_AGE`] have been pruned.
#[must_use]
pub fn records(vault: &Vault) -> Vec<(SystemTime, Vec<String>)> {
    let Ok(entries) = fs::read_dir(vault.lore_dir().join(DIR)) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for entry in entries.flatten() {
        let Ok(modified) = entry.metadata().and_then(|meta| meta.modified()) else {
            continue;
        };
        let Ok(text) = fs::read_to_string(entry.path()) else {
            continue;
        };
        let mut ids: Vec<String> = Vec::new();
        for line in text.lines().map(str::trim).filter(|l| !l.is_empty()) {
            if !ids.iter().any(|id| id == line) {
                ids.push(line.to_owned());
            }
        }
        out.push((modified, ids));
    }
    out
}

/// Delete the records in `dir` older than [`MAX_AGE`].
fn prune(dir: &std::path::Path) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    let now = SystemTime::now();
    for entry in entries.flatten() {
        let old = entry
            .metadata()
            .and_then(|meta| meta.modified())
            .ok()
            .and_then(|modified| now.duration_since(modified).ok())
            .is_some_and(|age| age > MAX_AGE);
        if old {
            let _ = fs::remove_file(entry.path());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vault() -> (tempfile::TempDir, Vault) {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join(".git")).unwrap();
        let vault = Vault::open(dir.path()).unwrap();
        (dir, vault)
    }

    #[test]
    fn a_session_remembers_what_it_was_shown_and_another_does_not() {
        let (_dir, vault) = vault();
        Shown::open(&vault, Some("s-1")).record(["01A", "01B"]);
        Shown::open(&vault, Some("s-1")).record(["01B", "01C"]);
        let again = Shown::open(&vault, Some("s-1"));
        assert!(again.contains("01A") && again.contains("01C"));
        assert!(!Shown::open(&vault, Some("s-2")).contains("01A"));
        let files: Vec<_> = fs::read_dir(vault.lore_dir().join(DIR)).unwrap().collect();
        assert_eq!(files.len(), 1);
        let text = fs::read_to_string(files[0].as_ref().unwrap().path()).unwrap();
        assert_eq!(text, "01A\n01B\n01C\n");
    }

    #[test]
    fn no_session_id_records_nothing() {
        let (_dir, vault) = vault();
        let mut shown = Shown::open(&vault, None);
        shown.record(["01A"]);
        assert!(!vault.lore_dir().join(DIR).exists());
        assert!(!Shown::open(&vault, Some("")).contains("01A"));
    }

    #[test]
    fn a_new_record_deletes_old_ones() {
        let (_dir, vault) = vault();
        let dir = vault.lore_dir().join(DIR);
        fs::create_dir_all(&dir).unwrap();
        let stale = dir.join("stale");
        fs::write(&stale, "01A\n").unwrap();
        let eight_days = SystemTime::now() - Duration::from_secs(8 * 24 * 60 * 60);
        fs::File::options()
            .write(true)
            .open(&stale)
            .unwrap()
            .set_modified(eight_days)
            .unwrap();
        let recent = dir.join("recent");
        fs::write(&recent, "01B\n").unwrap();
        Shown::open(&vault, Some("s-1")).record(["01C"]);
        assert!(!stale.exists());
        assert!(recent.exists());
    }
}
