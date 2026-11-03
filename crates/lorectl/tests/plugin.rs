//! The Claude Code and Codex plugins under `plugins/`, run the way each agent
//! runs them: the SessionStart command from `hooks/hooks.json` through a shell,
//! with the hook JSON on stdin, and its stdout taken as session context.
#![cfg(unix)]
#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde_json::{json, Value};

const NOTE: &str = "---\nid: 01JD0000000000000000000003\ntype: fact\ntitle: Migrations are global\n---\nEvery migration is logged.\n";

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn read_json(path: &Path) -> Value {
    let where_ = path.display();
    let text = std::fs::read_to_string(path)
        .map_err(|e| format!("{where_}: {e}"))
        .unwrap();
    serde_json::from_str(&text)
        .map_err(|e| format!("{where_}: {e}"))
        .unwrap()
}

/// Every SessionStart command in a hooks file.
fn session_start_commands(hooks: &Value) -> Vec<String> {
    hooks["hooks"]["SessionStart"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|group| group["hooks"].as_array().unwrap().iter())
        .map(|hook| {
            assert_eq!(hook["type"], "command", "{hook}");
            hook["command"].as_str().unwrap().to_owned()
        })
        .collect()
}

fn lorectl(home: &Path, args: &[&str]) {
    let out = Command::new(env!("CARGO_BIN_EXE_lorectl"))
        .args(args)
        .env("HOME", home)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env_remove("LORE_VAULT_DIR")
        .output()
        .unwrap();
    assert!(out.status.success(), "{args:?}: {out:?}");
}

/// Run `command` as an agent runs a hook command: through `sh -c`, with
/// `lorectl` on `PATH` and the hook JSON on stdin. Returns stdout.
fn run_hook(command: &str, home: &Path, vault: &Path, cwd: &Path) -> String {
    let bin_dir = Path::new(env!("CARGO_BIN_EXE_lorectl")).parent().unwrap();
    let path = format!("{}:/usr/bin:/bin", bin_dir.display());
    let stdin = json!({
        "session_id": "11111111-2222-3333-4444-555555555555",
        "hook_event_name": "SessionStart",
        "source": "startup",
        "cwd": cwd,
    });
    let mut child = Command::new("/bin/sh")
        .arg("-c")
        .arg(command)
        .current_dir(home)
        .env("PATH", path)
        .env("HOME", home)
        .env("LORE_VAULT_DIR", vault)
        .env("LORE_ARCHIVE_DIR", home.join("archive"))
        .env("CLAUDE_CONFIG_DIR", home.join("claude"))
        .env("CODEX_HOME", home.join("codex"))
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut pipe = child.stdin.take().unwrap();
    pipe.write_all(stdin.to_string().as_bytes()).unwrap();
    drop(pipe);
    let out = child.wait_with_output().unwrap();
    assert_eq!(out.status.code(), Some(0), "{command}: {out:?}");
    String::from_utf8(out.stdout).unwrap()
}

#[test]
fn each_plugin_runs_the_session_start_hook() {
    for dir in ["plugins/claude-code", "plugins/codex"] {
        let hooks = read_json(&repo_root().join(dir).join("hooks/hooks.json"));
        assert_eq!(
            session_start_commands(&hooks),
            ["lorectl hook session-start"],
            "{dir}"
        );
        assert_eq!(
            hooks["hooks"]
                .as_object()
                .unwrap()
                .keys()
                .collect::<Vec<_>>(),
            ["SessionStart"],
            "{dir} hooks only session start"
        );
    }
}

#[test]
fn each_manifest_names_the_plugin_and_finds_its_hooks() {
    let claude = read_json(&repo_root().join("plugins/claude-code/.claude-plugin/plugin.json"));
    assert_eq!(claude["name"], "lore");
    let codex = read_json(&repo_root().join("plugins/codex/.codex-plugin/plugin.json"));
    assert_eq!(codex["name"], "lore");
    assert_eq!(codex["hooks"], "./hooks/hooks.json");
    let market = read_json(&repo_root().join(".claude-plugin/marketplace.json"));
    assert_eq!(market["plugins"][0]["name"], "lore");
    assert_eq!(market["plugins"][0]["source"], "./plugins/claude-code");
}

#[test]
fn the_plugin_hook_puts_the_notes_on_stdout_for_the_session() {
    let home = tempfile::tempdir().unwrap();
    let vault = home.path().join("vault");
    lorectl(
        home.path(),
        &["mem", "init", "--vault", vault.to_str().unwrap()],
    );
    let note = home.path().join("note.md");
    std::fs::write(&note, NOTE).unwrap();
    lorectl(
        home.path(),
        &[
            "mem",
            "add",
            note.to_str().unwrap(),
            "--vault",
            vault.to_str().unwrap(),
        ],
    );
    let work = home.path().join("work");
    std::fs::create_dir_all(&work).unwrap();

    for dir in ["plugins/claude-code", "plugins/codex"] {
        let hooks = read_json(&repo_root().join(dir).join("hooks/hooks.json"));
        for command in session_start_commands(&hooks) {
            assert_eq!(
                run_hook(&command, home.path(), &vault, &work),
                "Lore memory for this repo:\n\
                 - [unanchored] Migrations are global (01JD0000000000000000000003)\n",
                "{dir}"
            );
        }
    }
}
