//! End-to-end assertions for actionable advice printed on stderr.
#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::path::Path;
use std::process::{Command, Output};

fn lorectl(home: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_lorectl"))
        .args(args)
        .env("HOME", home)
        .env("LORE_ARCHIVE_DIR", home.join("archive"))
        .output()
        .unwrap()
}

#[test]
fn relative_archive_override_advice() {
    let dir = tempfile::tempdir().unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_lorectl"))
        .args(["--archive", "relative/path", "scan"])
        .env("HOME", dir.path())
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("lorectl: archive path override must be absolute\n  give an absolute path starting with `/`"),
        "stderr was: {stderr}"
    );
}

#[test]
fn relative_env_override_advice() {
    let dir = tempfile::tempdir().unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_lorectl"))
        .args(["scan"])
        .env("HOME", dir.path())
        .env("LORE_ARCHIVE_DIR", "relative/dir")
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("lorectl: LORE_ARCHIVE_DIR must be an absolute path\n  set LORE_ARCHIVE_DIR to an absolute path starting with `/`"),
        "stderr was: {stderr}"
    );
}

#[test]
fn note_not_found_advice_ascii_and_non_ascii() {
    let dir = tempfile::tempdir().unwrap();
    let init = lorectl(dir.path(), &["mem", "init"]);
    assert_eq!(init.status.code(), Some(0));

    let ascii = lorectl(dir.path(), &["mem", "show", "nonexistent-note"]);
    assert_eq!(ascii.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&ascii.stderr);
    assert!(
        stderr.contains("lorectl: no note matches `nonexistent-note`\n  run `lorectl mem search <QUERY>` to find notes"),
        "stderr was: {stderr}"
    );

    let non_ascii = lorectl(dir.path(), &["mem", "show", "日本語ノート"]);
    assert_eq!(non_ascii.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&non_ascii.stderr);
    assert!(
        stderr.contains("lorectl: no note matches `日本語ノート`\n  run `lorectl mem search <QUERY>` to find notes"),
        "stderr was: {stderr}"
    );
}

#[test]
fn proposal_not_found_and_bad_slug_advice() {
    let dir = tempfile::tempdir().unwrap();
    let init = lorectl(dir.path(), &["mem", "init"]);
    assert_eq!(init.status.code(), Some(0));

    let not_found = lorectl(dir.path(), &["mem", "accept", "01JC0000000000000000000000"]);
    assert_eq!(not_found.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&not_found.stderr);
    assert!(
        stderr.contains("lorectl: no proposal `01JC0000000000000000000000` in the inbox\n  run `lorectl mem inbox` to see what is waiting"),
        "stderr was: {stderr}"
    );

    let bad_slug = lorectl(
        dir.path(),
        &[
            "mem",
            "accept",
            "01JC0000000000000000000000",
            "--as",
            "Bad/Slug",
        ],
    );
    assert_eq!(bad_slug.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&bad_slug.stderr);
    assert!(
        stderr.contains("lorectl: `Bad/Slug` is not a valid slug: use lowercase letters, digits, and single `-`\n  give a slug of lowercase letters, digits, and single `-`"),
        "stderr was: {stderr}"
    );
}

#[test]
fn relative_vault_override_advice() {
    let dir = tempfile::tempdir().unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_lorectl"))
        .args(["--vault", "relative/vault", "mem", "show", "test"])
        .env("HOME", dir.path())
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("lorectl: the vault location must be an absolute path, got `relative/vault`\n  give an absolute path starting with `/`"),
        "stderr was: {stderr}"
    );
}

#[test]
fn relative_vault_env_advice() {
    let dir = tempfile::tempdir().unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_lorectl"))
        .args(["mem", "show", "test"])
        .env("HOME", dir.path())
        .env("LORE_VAULT_DIR", "relative/vault")
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("lorectl: the vault location must be an absolute path, got `relative/vault`\n  give an absolute path starting with `/`"),
        "stderr was: {stderr}"
    );
}

#[test]
fn bad_note_path_advice_ascii_and_non_ascii() {
    let dir = tempfile::tempdir().unwrap();
    let init = lorectl(dir.path(), &["mem", "init"]);
    assert_eq!(init.status.code(), Some(0));

    let dotdot = lorectl(dir.path(), &["mem", "show", "../note"]);
    assert_eq!(dotdot.status.code(), Some(7));
    let stderr = String::from_utf8_lossy(&dotdot.stderr);
    assert!(
        stderr.contains("lorectl: `../note` is not a valid note path: `..` is not allowed, got `../note`\n  give a relative path inside the vault without `..`"),
        "stderr was: {stderr}"
    );

    let non_ascii_abs = lorectl(dir.path(), &["mem", "show", "/etc/日本語"]);
    assert_eq!(non_ascii_abs.status.code(), Some(7));
    let stderr = String::from_utf8_lossy(&non_ascii_abs.stderr);
    assert!(
        stderr.contains("lorectl: `/etc/日本語` is not a valid note path: absolute paths outside the vault are not allowed, got `/etc/日本語`\n  give a relative path inside the vault without `..`"),
        "stderr was: {stderr}"
    );
}
