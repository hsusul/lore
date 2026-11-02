//! A non-UTF-8 `.md` file in the vault must not break `lorectl mem search`
//! through the incremental index refresh, before or after a reindex.
#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::path::Path;
use std::process::{Command, Output};

fn lorectl(home: &Path, vault: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_lorectl"))
        .arg("mem")
        .args(args)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("HOME", home)
        .env("LORE_VAULT_DIR", vault)
        .output()
        .expect("spawn lorectl")
}

fn code(o: &Output) -> i32 {
    o.status.code().expect("exited with a code")
}
fn stdout(o: &Output) -> String {
    String::from_utf8_lossy(&o.stdout).into_owned()
}
fn stderr(o: &Output) -> String {
    String::from_utf8_lossy(&o.stderr).into_owned()
}

const NOTE: &str =
    "---\ntype: fact\ntitle: Kingfisher rule\nscope: global\n---\nThe kingfisher dives at dawn.\n";
const HIT: &str = "kingfisher-rule  global  The kingfisher dives at dawn.\n";

/// A vault with one good note added through the CLI, so the index exists.
fn vault_with_note() -> (tempfile::TempDir, std::path::PathBuf, std::path::PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    std::fs::create_dir_all(&home).unwrap();
    let vault = dir.path().join("vault");
    let out = lorectl(&home, &vault, &["init"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    let file = dir.path().join("n.md");
    std::fs::write(&file, NOTE).unwrap();
    let out = lorectl(&home, &vault, &["add", file.to_str().unwrap()]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    (dir, home, vault)
}

fn write_bad(vault: &Path, bytes: &[u8]) {
    let global = vault.join("global");
    std::fs::create_dir_all(&global).unwrap();
    std::fs::write(global.join("bad.md"), bytes).unwrap();
}

#[test]
fn mem_search_skips_a_non_utf8_file_without_reindex() {
    let (_dir, home, vault) = vault_with_note();
    write_bad(&vault, b"\xff\xfe");
    let out = lorectl(&home, &vault, &["search", "kingfisher"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    assert_eq!(stdout(&out), HIT);
}

#[test]
fn mem_search_skips_a_non_utf8_file_that_changed_after_reindex() {
    let (_dir, home, vault) = vault_with_note();
    write_bad(&vault, b"\xff\xfe");
    let out = lorectl(&home, &vault, &["reindex"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    write_bad(&vault, b"\xff\xfe\xfd\xfc");
    let out = lorectl(&home, &vault, &["search", "kingfisher"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    assert_eq!(stdout(&out), HIT);
}
