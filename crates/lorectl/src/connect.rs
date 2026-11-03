//! `lorectl connect`: whether Claude Code and Codex run Lore's MCP server and
//! session-start hook, and the exact command or config to add each one missing.
//!
//! Read only. It never writes an agent's config and never runs `claude` or
//! `codex`. Locations follow each agent's documentation. Claude Code keeps MCP
//! servers in `.claude.json` (top level, and per project) and hooks in
//! `settings.json`. With `CLAUDE_CONFIG_DIR` set, both live in that folder;
//! otherwise `.claude.json` sits beside `~/.claude`. Codex keeps MCP servers in
//! `$CODEX_HOME/config.toml` and hooks there or in `hooks.json`. A broken file
//! is reported on its line, and the command always exits 0.

use std::path::{Path, PathBuf};

use serde_json::Value;

/// A piece's status, and how to add it when it is not connected.
type Piece = (String, Option<String>);
/// Reads one agent's config folder into its two pieces.
type Pieces<'a> = &'a dyn Fn(&Path) -> [Piece; 2];

/// Print the report for the running binary. Always exit 0.
pub fn run() -> u8 {
    let exe = std::env::current_exe().unwrap_or_else(|_| "lorectl".into());
    print!("{}", report(&exe.to_string_lossy()));
    crate::exit::OK
}

// How to add each piece. `{exe}` and `{hook}` are JSON strings, which are also
// TOML strings; `{exe_sh}` is quoted for a shell.
const CLAUDE_MCP: &str = "run:\n  claude mcp add --scope user lore -- {exe_sh} mcp";
const CLAUDE_HOOK: &str = r#"merge this into {file}:
  {"hooks": {"SessionStart": [{"hooks": [{"type": "command", "command": {hook}}]}]}}"#;
const CODEX_MCP: &str = r#"add this to {file}:
  [mcp_servers.lore]
  command = {exe}
  args = ["mcp"]"#;
const CODEX_HOOK: &str = r#"add this to {file}:
  [[hooks.SessionStart]]
  [[hooks.SessionStart.hooks]]
  type = "command"
  command = {hook}"#;

fn report(exe: &str) -> String {
    let fill = |template: &str, file: &Path| {
        let hook = format!("{} hook session-start", sh(exe));
        template
            .replace("{exe_sh}", &sh(exe))
            .replace("{exe}", &json(exe))
            .replace("{hook}", &json(&hook))
            .replace("{file}", &file.to_string_lossy())
    };
    let claude = |dir: &Path| {
        let mcp = match std::env::var_os("CLAUDE_CONFIG_DIR") {
            Some(_) => dir.join(".claude.json"),
            None => dir.with_file_name(".claude.json"),
        };
        let settings = dir.join("settings.json");
        [
            check(&[(mcp.clone(), false)], claude_mcp, fill(CLAUDE_MCP, &mcp)),
            check(
                &[(settings.clone(), false)],
                session_hook,
                fill(CLAUDE_HOOK, &settings),
            ),
        ]
    };
    let codex = |dir: &Path| {
        let config = dir.join("config.toml");
        let files = [(config.clone(), true), (dir.join("hooks.json"), false)];
        [
            check(&files[..1], codex_mcp, fill(CODEX_MCP, &config)),
            check(&files, session_hook, fill(CODEX_HOOK, &config)),
        ]
    };
    let agents: [(&str, &str, &str, Pieces<'_>); 2] = [
        ("Claude Code", "CLAUDE_CONFIG_DIR", ".claude", &claude),
        ("Codex", "CODEX_HOME", ".codex", &codex),
    ];

    let (mut out, mut fixes, mut found) = (String::new(), String::new(), false);
    for (name, var, home_sub, pieces) in agents {
        // `$VAR`, else `$HOME/<home_sub>`, the way the scanner resolves it.
        let dir = std::env::var_os(var)
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(home_sub)));
        out.push_str(name);
        if let Some(dir) = &dir {
            out.push_str(&format!("  {}", dir.display()));
        }
        out.push('\n');
        let pieces = dir.as_deref().filter(|dir| dir.is_dir()).map(pieces);
        found |= pieces.is_some();
        let missing = || ("agent not found".to_string(), None);
        let pieces = pieces.unwrap_or_else(|| [missing(), missing()]);
        for (label, (status, fix)) in ["MCP server", "Session hook"].into_iter().zip(pieces) {
            out.push_str(&format!("  {label:<12}  {status}\n"));
            if let Some(fix) = fix {
                let noun = label.replace("Session", "session");
                fixes.push_str(&format!("\nTo connect {name}'s {noun}, {fix}\n"));
            }
        }
    }
    if !found {
        fixes.push_str(
            "\nNeither agent was found. Install one, then run `lorectl connect` again.\n",
        );
    }
    out + &fixes
}

/// The first file (JSON, or TOML when flagged) where `find` names an entry.
fn check(files: &[(PathBuf, bool)], find: fn(&Value) -> Option<String>, fix: String) -> Piece {
    let mut status = "not connected".to_string();
    for (path, is_toml) in files {
        let parsed = match std::fs::read_to_string(path) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => Err(e.to_string()),
            Ok(text) if *is_toml => toml::from_str(&text).map_err(|e| e.message().to_string()),
            Ok(text) => serde_json::from_str(&text).map_err(|e| e.to_string()),
        };
        match parsed.map(|root| find(&root)) {
            Ok(Some(entry)) => return (format!("connected ({}, {entry})", path.display()), None),
            Ok(None) => {}
            Err(why) => status = format!("unreadable ({}, {why})", path.display()),
        }
    }
    (status, Some(fix))
}

fn claude_mcp(root: &Value) -> Option<String> {
    mcp_entry(root.get("mcpServers"), "mcpServers").or_else(|| {
        let mut projects = root["projects"].as_object()?.iter();
        projects.find_map(|(path, project)| {
            let prefix = format!("projects[{}].mcpServers", json(path));
            mcp_entry(project.get("mcpServers"), &prefix)
        })
    })
}

fn codex_mcp(root: &Value) -> Option<String> {
    mcp_entry(root.get("mcp_servers"), "mcp_servers")
}

/// The name of the first server in `servers` that runs `lorectl ... mcp`.
fn mcp_entry(servers: Option<&Value>, prefix: &str) -> Option<String> {
    let (name, _) = servers?.as_object()?.iter().find(|(_, server)| {
        let args = server["args"].as_array().into_iter().flatten();
        let args: Vec<&str> = args.filter_map(Value::as_str).collect();
        let command = server["command"].as_str().unwrap_or_default();
        runs_lorectl(&format!("{command} {}", args.join(" ")), &["mcp"])
    })?;
    Some(format!("{prefix}.{name}"))
}

/// `hooks.SessionStart[].hooks[].command`, the shape both agents share.
fn session_hook(root: &Value) -> Option<String> {
    let groups = root["hooks"]["SessionStart"].as_array()?.iter();
    let hooks = groups
        .filter_map(|group| group["hooks"].as_array())
        .flatten();
    let mut commands = hooks.filter_map(|hook| hook["command"].as_str());
    commands
        .any(|command| runs_lorectl(command, &["hook", "session-start"]))
        .then(|| "hooks.SessionStart".to_string())
}

/// Whether the command line runs `lorectl`, by basename or path, quoted or
/// not, with the words `sub` after it.
fn runs_lorectl(line: &str, sub: &[&str]) -> bool {
    let boundary = |c: char| matches!(c, '/' | '\'' | '"') || c.is_whitespace();
    line.match_indices("lorectl").any(|(at, word)| {
        let rest = line[at + word.len()..].trim_start_matches(['\'', '"']);
        let words: Vec<&str> = rest.split_whitespace().collect();
        (at == 0 || line[..at].ends_with(boundary))
            && rest.starts_with(char::is_whitespace)
            && words.windows(sub.len()).any(|window| window == sub)
    })
}

/// `word` quoted for a POSIX shell when it needs it.
fn sh(word: &str) -> String {
    let plain = |b: u8| b.is_ascii_alphanumeric() || b"/._-+=:@,".contains(&b);
    if !word.is_empty() && word.bytes().all(plain) {
        word.to_string()
    } else {
        format!("'{}'", word.replace('\'', r"'\''"))
    }
}

/// A JSON string literal, which is also a valid TOML basic string.
fn json(text: &str) -> String {
    Value::from(text).to_string()
}
