//! The Claude Code and Codex plugins under `plugins/`, run the way each agent
//! runs them: the SessionStart command from `hooks/hooks.json` through a shell,
//! with the hook JSON on stdin, and its stdout taken as session context.
#![cfg(unix)]
#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::collections::BTreeSet;
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

/// Each plugin's MCP server entry: command, args, and env.
fn mcp_server(dir: &str) -> (String, Vec<String>, Vec<(String, String)>) {
    let mcp = read_json(&repo_root().join(dir).join(".mcp.json"));
    let lore = &mcp["mcpServers"]["lore"];
    let args = lore["args"].as_array().unwrap();
    let env = lore["env"].as_object().unwrap();
    (
        lore["command"].as_str().unwrap().to_owned(),
        args.iter()
            .map(|a| a.as_str().unwrap().to_owned())
            .collect(),
        env.iter()
            .map(|(k, v)| (k.clone(), v.as_str().unwrap().to_owned()))
            .collect(),
    )
}

#[test]
fn each_plugin_registers_lorectl_mcp_as_the_lore_server() {
    for (dir, agent) in [
        ("plugins/claude-code", "claude-code"),
        ("plugins/codex", "codex"),
    ] {
        let (command, args, env) = mcp_server(dir);
        assert_eq!(command, "lorectl", "{dir}");
        assert_eq!(args, ["mcp"], "{dir}");
        assert_eq!(env, [("LORE_AGENT".to_owned(), agent.to_owned())], "{dir}");
    }
    let codex = read_json(&repo_root().join("plugins/codex/.codex-plugin/plugin.json"));
    assert_eq!(codex["mcpServers"], "./.mcp.json");
    let mcp = read_json(&repo_root().join("plugins/codex/.mcp.json"));
    assert_eq!(
        mcp["mcpServers"]["lore"]["env_vars"],
        json!(["LORE_VAULT_DIR", "LORE_ARCHIVE_DIR"])
    );
}

#[test]
fn the_plugin_mcp_server_files_proposals_as_its_own_agent() {
    let home = tempfile::tempdir().unwrap();
    let vault = home.path().join("vault");
    lorectl(
        home.path(),
        &["mem", "init", "--vault", vault.to_str().unwrap()],
    );
    let bin_dir = Path::new(env!("CARGO_BIN_EXE_lorectl")).parent().unwrap();
    for (dir, agent) in [
        ("plugins/claude-code", "claude-code"),
        ("plugins/codex", "codex"),
    ] {
        let (command, args, env) = mcp_server(dir);
        let mut child = Command::new(bin_dir.join(&command))
            .args(&args)
            .envs(env)
            .env("HOME", home.path())
            .env("LORE_VAULT_DIR", &vault)
            .env("LORE_ARCHIVE_DIR", home.path().join("archive"))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let messages = [
            json!({"jsonrpc": "2.0", "id": 1, "method": "initialize",
                "params": {"protocolVersion": "2025-06-18", "clientInfo": {"name": "some-client"}}}),
            json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list"}),
            json!({"jsonrpc": "2.0", "id": 3, "method": "tools/call", "params": {
                "name": "propose_memory", "arguments": {
                    "title": "T", "body": "B", "type": "fact", "scope": "global"}}}),
        ];
        let mut pipe = child.stdin.take().unwrap();
        for m in &messages {
            writeln!(pipe, "{m}").unwrap();
        }
        drop(pipe);
        let out = child.wait_with_output().unwrap();
        let replies: Vec<Value> = String::from_utf8(out.stdout)
            .unwrap()
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        let tools: Vec<&str> = replies[1]["result"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["name"].as_str().unwrap())
            .collect();
        assert_eq!(
            tools,
            [
                "search_sessions",
                "read_session",
                "recall_memory",
                "read_note",
                "mine_candidates",
                "propose_memory"
            ],
            "{dir}"
        );
        let inbox = replies[2]["result"]["structuredContent"]["inbox"]
            .as_str()
            .unwrap();
        assert!(
            inbox.starts_with(&format!("_inbox/{agent}-mcp-")),
            "{dir}: {inbox}"
        );
    }
}

/// The `tools/list` reply of the real `lorectl mcp` server.
fn listed_tools(home: &Path) -> Vec<Value> {
    let mut child = Command::new(env!("CARGO_BIN_EXE_lorectl"))
        .arg("mcp")
        .env("HOME", home)
        .env("LORE_VAULT_DIR", home.join("vault"))
        .env("LORE_ARCHIVE_DIR", home.join("archive"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut pipe = child.stdin.take().unwrap();
    for m in [
        json!({"jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": {"protocolVersion": "2025-06-18", "clientInfo": {"name": "claude-code"}}}),
        json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list"}),
    ] {
        writeln!(pipe, "{m}").unwrap();
    }
    drop(pipe);
    let out = child.wait_with_output().unwrap();
    let stdout = String::from_utf8(out.stdout).unwrap();
    let reply: Value = serde_json::from_str(stdout.lines().nth(1).unwrap()).unwrap();
    reply["result"]["tools"].as_array().unwrap().clone()
}

/// Every identifier-like word in `value`, keys and strings alike.
fn words(value: &Value, out: &mut BTreeSet<String>) {
    fn add(text: &str, out: &mut BTreeSet<String>) {
        let parts = text.split(|c: char| !(c.is_ascii_alphanumeric() || c == '_'));
        out.extend(parts.filter(|w| !w.is_empty()).map(str::to_owned));
    }
    match value {
        Value::String(s) => add(s, out),
        Value::Array(items) => items.iter().for_each(|v| words(v, out)),
        Value::Object(map) => {
            for (k, v) in map {
                add(k, out);
                words(v, out);
            }
        }
        _ => {}
    }
}

/// A SKILL.md's frontmatter fields in order, and its body.
fn skill_parts(text: &str) -> (Vec<(String, String)>, String) {
    let rest = text
        .strip_prefix("---\n")
        .expect("frontmatter opens the file");
    let (front, body) = rest.split_once("\n---\n").expect("frontmatter closes");
    let fields = front
        .lines()
        .map(|line| {
            let (key, value) = line.split_once(':').expect("key: value");
            (key.to_owned(), value.trim().to_owned())
        })
        .collect();
    (fields, body.to_owned())
}

/// The names a skill body leans on: every word quoted in backticks that is
/// a lowercase identifier, and every snake_case word anywhere.
fn named_identifiers(body: &str) -> BTreeSet<String> {
    let ident = |w: &str| !w.is_empty() && w.chars().all(|c| c.is_ascii_lowercase() || c == '_');
    let quoted = body.split('`').skip(1).step_by(2).filter(|w| ident(w));
    let snake = body
        .split(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
        .filter(|w| w.contains('_') && ident(w));
    quoted.chain(snake).map(str::to_owned).collect()
}

#[test]
fn the_lore_lessons_skill_names_only_what_the_lore_server_lists() {
    let home = tempfile::tempdir().unwrap();
    let tools = listed_tools(home.path());
    let tool_names: BTreeSet<String> = tools
        .iter()
        .map(|t| t["name"].as_str().unwrap().to_owned())
        .collect();
    let mut vocabulary = BTreeSet::new();
    words(&Value::Array(tools), &mut vocabulary);

    let codex = read_json(&repo_root().join("plugins/codex/.codex-plugin/plugin.json"));
    assert_eq!(codex["skills"], "./skills/");

    for dir in ["plugins/claude-code", "plugins/codex"] {
        let path = repo_root().join(dir).join("skills/lore-lessons/SKILL.md");
        let text = std::fs::read_to_string(&path)
            .map_err(|e| format!("{}: {e}", path.display()))
            .unwrap();
        let (fields, body) = skill_parts(&text);
        let keys: Vec<&str> = fields.iter().map(|(k, _)| k.as_str()).collect();
        assert_eq!(keys, ["name", "description"], "{dir}");
        assert_eq!(fields[0].1, "lore-lessons", "{dir}");
        assert!(!fields[1].1.is_empty(), "{dir} description");

        let named = named_identifiers(&body);
        let tools_named: Vec<&str> = named
            .iter()
            .filter(|w| tool_names.contains(*w))
            .map(String::as_str)
            .collect();
        assert_eq!(
            tools_named,
            [
                "mine_candidates",
                "propose_memory",
                "recall_memory",
                "search_sessions"
            ],
            "{dir}"
        );
        let unknown: Vec<&String> = named.iter().filter(|w| !vocabulary.contains(*w)).collect();
        assert!(
            unknown.is_empty(),
            "{dir} names what lorectl mcp does not list: {unknown:?}"
        );
    }
    let skill = |dir: &str| {
        std::fs::read_to_string(repo_root().join(dir).join("skills/lore-lessons/SKILL.md")).unwrap()
    };
    assert_eq!(
        skill("plugins/claude-code"),
        skill("plugins/codex"),
        "both plugins ship one skill"
    );

}
