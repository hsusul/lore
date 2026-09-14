//! Read-only file access inside a workspace root (a git repository top-level
//! or a task worktree), for the explorer and file tabs.
//!
//! Every path is resolved against the root and canonicalized; anything that
//! escapes the root (via `..` or symlinks) is refused.

use std::fs;
use std::io::Read;
use std::path::{Component, Path, PathBuf};

use lore_ipc::{DirEntryDto, FileContentDto};

use crate::{git, Error, Result};

const MAX_ENTRIES: usize = 5_000;
const MAX_FILE_BYTES: u64 = 1024 * 1024;
/// Never shown in the explorer.
const HIDDEN: &[&str] = &[".git", ".DS_Store"];

/// Validate that `root` is an existing repository or worktree top-level.
pub fn workspace_root(root: &str) -> Result<PathBuf> {
    let root = PathBuf::from(root);
    if !root.is_absolute() || !root.is_dir() {
        return Err(Error::Invalid(
            "workspace must be an existing absolute directory".into(),
        ));
    }
    let top = git::toplevel(&root).map_err(not_a_git_repository)?;
    let (a, b) = (fs::canonicalize(&root)?, fs::canonicalize(&top)?);
    if a != b {
        return Err(Error::Invalid(
            "workspace must be the top-level of a git repository".into(),
        ));
    }
    Ok(a)
}

/// The repository top-level containing any directory `path`.
pub fn repository_root(path: &str) -> Result<PathBuf> {
    let path = PathBuf::from(path);
    if !path.is_absolute() || !path.is_dir() {
        return Err(Error::Invalid(
            "choose an existing folder inside a git repository".into(),
        ));
    }
    let top = git::toplevel(&path).map_err(not_a_git_repository)?;
    Ok(fs::canonicalize(top)?)
}

pub(crate) fn not_a_git_repository(err: Error) -> Error {
    match err {
        Error::Git(msg) if msg.to_ascii_lowercase().contains("not a git repository") => {
            Error::Invalid("That folder isn't inside a git repository".into())
        }
        other => other,
    }
}

fn resolve(root: &Path, rel: &str) -> Result<PathBuf> {
    let rel = Path::new(rel);
    if rel.is_absolute()
        || rel.components().any(|c| {
            !matches!(c, Component::Normal(_) | Component::CurDir) || c.as_os_str() == ".git"
        })
    {
        return Err(Error::Invalid(
            "path must be relative to the workspace".into(),
        ));
    }
    let full = fs::canonicalize(root.join(rel))?;
    if !full.starts_with(root) {
        return Err(Error::Invalid("path escapes the workspace".into()));
    }
    Ok(full)
}

fn rel_string(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .components()
        .map(|c| c.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

/// List one directory: folders first, then files, each alphabetical.
pub fn list_dir(root: &str, rel: &str) -> Result<Vec<DirEntryDto>> {
    let root = workspace_root(root)?;
    let dir = resolve(&root, rel)?;
    if !dir.is_dir() {
        return Err(Error::Invalid("not a directory".into()));
    }
    let mut out = Vec::new();
    let entries = fs::read_dir(&dir)?
        .flatten()
        .filter(|e| !HIDDEN.contains(&e.file_name().to_string_lossy().as_ref()))
        .take(MAX_ENTRIES);
    for entry in entries {
        let name = entry.file_name().to_string_lossy().to_string();
        // Follow symlinks for the folder/file distinction; opening one that
        // leaves the workspace is still refused by `resolve`.
        let is_dir = fs::metadata(entry.path())
            .map(|m| m.is_dir())
            .unwrap_or(false);
        out.push(DirEntryDto {
            rel_path: rel_string(&root, &dir.join(&name)),
            name,
            is_dir,
        });
    }
    out.sort_by(|a, b| {
        b.is_dir
            .cmp(&a.is_dir)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    Ok(out)
}

/// Read a file for display; binary files return no text.
pub fn read_file(root: &str, rel: &str) -> Result<FileContentDto> {
    let root = workspace_root(root)?;
    let path = resolve(&root, rel)?;
    if !path.is_file() {
        return Err(Error::Invalid("not a file".into()));
    }
    let size = fs::metadata(&path)?.len();
    let mut buf = Vec::new();
    fs::File::open(&path)?
        .take(MAX_FILE_BYTES)
        .read_to_end(&mut buf)?;
    let truncated = size > MAX_FILE_BYTES;
    let text = if buf.contains(&0) {
        None
    } else {
        match String::from_utf8(buf) {
            Ok(s) => Some(s),
            // A cut can split a multi-byte character; keep the valid prefix.
            Err(e) if truncated => {
                let valid = e.utf8_error().valid_up_to();
                let mut bytes = e.into_bytes();
                bytes.truncate(valid);
                String::from_utf8(bytes).ok()
            }
            Err(_) => None,
        }
    };
    Ok(FileContentDto {
        rel_path: rel_string(&root, &path),
        text,
        size: i64::try_from(size).unwrap_or(i64::MAX),
        truncated,
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use std::process::Command;

    fn repo() -> tempfile::TempDir {
        let tmp = tempfile::tempdir().unwrap();
        Command::new("git")
            .arg("-C")
            .arg(tmp.path())
            .args(["init", "-q"])
            .status()
            .unwrap();
        fs::create_dir_all(tmp.path().join("src")).unwrap();
        fs::write(tmp.path().join("src/main.rs"), "fn main() {}\n").unwrap();
        fs::write(tmp.path().join("README.md"), "hi\n").unwrap();
        fs::write(tmp.path().join("logo.bin"), [0u8, 1, 2]).unwrap();
        tmp
    }

    #[test]
    fn lists_folders_first_and_hides_git() {
        let tmp = repo();
        let root = tmp.path().to_str().unwrap();
        let names: Vec<_> = list_dir(root, "")
            .unwrap()
            .into_iter()
            .map(|e| e.name)
            .collect();
        assert_eq!(names, ["src", "logo.bin", "README.md"]);
        assert_eq!(list_dir(root, "src").unwrap()[0].rel_path, "src/main.rs");
    }

    #[test]
    fn reads_text_and_flags_binary() {
        let tmp = repo();
        let root = tmp.path().to_str().unwrap();
        assert_eq!(
            read_file(root, "src/main.rs").unwrap().text.as_deref(),
            Some("fn main() {}\n")
        );
        assert_eq!(read_file(root, "logo.bin").unwrap().text, None);
    }

    #[test]
    fn refuses_escapes() {
        let tmp = repo();
        let outside = tempfile::tempdir().unwrap();
        fs::write(outside.path().join("secret"), "x").unwrap();
        std::os::unix::fs::symlink(outside.path(), tmp.path().join("link")).unwrap();
        let root = tmp.path().to_str().unwrap();
        assert!(read_file(root, "../secret").is_err());
        assert!(read_file(root, ".git/config").is_err());
        assert!(list_dir(root, ".git").is_err());
        assert!(read_file(root, "/etc/hosts").is_err());
        assert!(read_file(root, "link/secret").is_err());
        assert!(list_dir(root, "link").is_err());
        assert!(list_dir(tmp.path().join("src").to_str().unwrap(), "").is_err());
    }
}
