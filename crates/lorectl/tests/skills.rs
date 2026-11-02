//! `lorectl skills` as a process, against the lore-skills fixture home. HOME is
//! always pointed at a fixture or a temp dir, never the real one, and the
//! agent root overrides are cleared so a developer's own never leak in.
#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::SystemTime;

fn fixture_home() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../lore-skills/fixtures/home")
}

fn command(home: &Path, args: &[&str]) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_lorectl"));
    command
        .args(args)
        .env("HOME", home)
        .env_remove("CLAUDE_CONFIG_DIR")
        .env_remove("CODEX_HOME");
    command
}

fn lorectl(home: &Path, args: &[&str]) -> Output {
    command(home, args).output().expect("lorectl runs")
}

fn stdout(output: &Output) -> String {
    String::from_utf8(output.stdout.clone()).expect("utf-8")
}

fn json(output: &Output) -> Vec<serde_json::Value> {
    let value: serde_json::Value = serde_json::from_str(&stdout(output)).unwrap();
    value.as_array().unwrap().clone()
}

fn write(path: &Path, text: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, text).unwrap();
}

#[test]
fn claude_config_dir_and_codex_home_move_the_agent_roots() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    let claude = dir.path().join("claude-cfg");
    let codex = dir.path().join("codex-home");
    fs::create_dir_all(&home).unwrap();
    let skill = |name: &str| format!("---\nname: {name}\ndescription: d\n---\nBody.\n");
    write(&claude.join("skills/alpha/SKILL.md"), &skill("alpha"));
    let install = claude.join("plugins/cache/mkt/p/1.0");
    write(&install.join("skills/beta/SKILL.md"), &skill("beta"));
    write(
        &claude.join("plugins/installed_plugins.json"),
        &serde_json::json!({
            "version": 2,
            "plugins": {"p@mkt": [{"installPath": install, "version": "1.0"}]}
        })
        .to_string(),
    );
    write(&codex.join("skills/gamma/SKILL.md"), &skill("gamma"));
    // Ignored: these sit where the defaults would look, not where the
    // overrides point.
    write(&home.join(".claude/skills/stale/SKILL.md"), &skill("stale"));
    write(
        &home.join(".codex/skills/stale2/SKILL.md"),
        &skill("stale2"),
    );

    let output = command(&home, &["--json", "skills", "ls"])
        .env("CLAUDE_CONFIG_DIR", &claude)
        .env("CODEX_HOME", &codex)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(0));
    let got: Vec<(String, String, String)> = json(&output)
        .iter()
        .map(|r| {
            (
                r["name"].as_str().unwrap().to_string(),
                r["installs"][0]["location"].as_str().unwrap().to_string(),
                r["source"]["kind"].as_str().unwrap().to_string(),
            )
        })
        .collect();
    let row = |n: &str, l: &str, s: &str| (n.to_string(), l.to_string(), s.to_string());
    assert_eq!(
        got,
        vec![
            row("alpha", "claude", "unknown"),
            row("beta", "claude-plugins", "plugin"),
            row("gamma", "codex", "unknown"),
        ]
    );
}

/// The content hash `skills ls --json` reports for the one skill in `home`.
fn only_hash(home: &Path) -> String {
    let records = json(&lorectl(home, &["--json", "skills", "ls"]));
    assert_eq!(records.len(), 1);
    records[0]["hash"].as_str().unwrap().to_string()
}

#[cfg(unix)]
#[test]
fn hashing_stays_inside_the_skill_folder() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    let skill = home.join(".claude/skills/s");
    write(
        &skill.join("SKILL.md"),
        "---\nname: s\ndescription: d\n---\n",
    );
    write(&dir.path().join("outside/secret.txt"), "one");
    std::os::unix::fs::symlink(dir.path().join("outside"), skill.join("out")).unwrap();
    let before = only_hash(&home);
    write(&dir.path().join("outside/secret.txt"), "two, longer");
    assert_eq!(
        only_hash(&home),
        before,
        "a link out of the folder was read"
    );
}

#[test]
fn hashing_reads_a_bounded_prefix_of_large_files() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    let skill = home.join(".claude/skills/s");
    write(
        &skill.join("SKILL.md"),
        "---\nname: s\ndescription: d\n---\n",
    );
    let mut bytes = vec![0_u8; 4 << 20];
    fs::write(skill.join("big.bin"), &bytes).unwrap();
    let before = only_hash(&home);
    // Same length, one byte changed past the cap: not read, so not seen.
    *bytes.last_mut().unwrap() = 1;
    fs::write(skill.join("big.bin"), &bytes).unwrap();
    assert_eq!(only_hash(&home), before);
    // A different length is seen.
    bytes.push(0);
    fs::write(skill.join("big.bin"), &bytes).unwrap();
    assert_ne!(only_hash(&home), before);
}

#[test]
fn ls_json_lists_one_record_per_distinct_content() {
    let output = lorectl(&fixture_home(), &["--json", "skills", "ls"]);
    assert_eq!(output.status.code(), Some(0));
    assert_eq!(json(&output).len(), 13);
}

/// Every path below `root` with its modification time and length.
fn snapshot(root: &Path) -> Vec<(PathBuf, SystemTime, u64)> {
    fn walk(dir: &Path, out: &mut Vec<(PathBuf, SystemTime, u64)>) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            let meta = std::fs::symlink_metadata(&path).unwrap();
            out.push((path.clone(), meta.modified().unwrap(), meta.len()));
            if meta.is_dir() {
                walk(&path, out);
            }
        }
    }
    let mut out = Vec::new();
    walk(root, &mut out);
    out.sort();
    out
}

#[test]
fn skills_commands_write_nothing_under_home() {
    let home = fixture_home();
    let before = snapshot(&home);
    for args in [&["skills", "ls"][..], &["--json", "skills", "ls"]] {
        assert_eq!(lorectl(&home, args).status.code(), Some(0));
    }
    assert_eq!(snapshot(&home), before);
}

#[test]
fn a_missing_or_unknown_subcommand_is_a_usage_error() {
    let home = fixture_home();
    for args in [&["skills"][..], &["skills", "fix"]] {
        let output = lorectl(&home, args);
        assert_eq!(output.status.code(), Some(1), "{args:?}");
    }
}

#[test]
fn a_relative_home_is_a_usage_error() {
    let output = lorectl(Path::new("relative/home"), &["skills", "ls"]);
    assert_eq!(output.status.code(), Some(1));
}
