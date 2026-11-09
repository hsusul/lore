//! Helpers shared by the `lorectl` integration tests.
#![allow(dead_code, clippy::expect_used, clippy::unwrap_used)]

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

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

/// Run `command` the way a person does, with a terminal on its stdin, typing
/// `input` (whole lines) and then the end-of-input key. Stdout and stderr are
/// piped. `mem accept`, `mem retire`, and `init --review` refuse any other
/// stdin.
pub fn on_a_terminal(command: &mut Command, input: &str) -> Output {
    use rustix::fs::{Mode, OFlags};
    use rustix::pty::{self, OpenptFlags};
    assert!(input.is_empty() || input.ends_with('\n'), "{input:?}");
    let keyboard = pty::openpt(OpenptFlags::RDWR | OpenptFlags::NOCTTY).unwrap();
    // Close-on-exec, as std's own pipes are: tests run in parallel, and a
    // terminal every sibling's child inherited held those children open at
    // exit for seconds (openpt cannot set this flag on macOS).
    rustix::io::fcntl_setfd(&keyboard, rustix::io::FdFlags::CLOEXEC).unwrap();
    pty::grantpt(&keyboard).unwrap();
    pty::unlockpt(&keyboard).unwrap();
    let name = pty::ptsname(&keyboard, Vec::new()).unwrap();
    let terminal = rustix::fs::open(
        &name,
        OFlags::RDWR | OFlags::NOCTTY | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .unwrap();
    let child = command
        .stdin(terminal)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    // Ctrl-D at the start of a line, a terminal's end of input. The keyboard
    // stays open until the child exits, so no typed line is dropped.
    let mut keyboard = std::fs::File::from(keyboard);
    keyboard
        .write_all(format!("{input}\x04").as_bytes())
        .unwrap();
    child.wait_with_output().unwrap()
}
