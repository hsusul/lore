//! `lorectl connect`: whether Claude Code and Codex run Lore's MCP server and
//! its three hooks (session start, prompt, and pre-tool), and the exact
//! command or config to add each one missing. A `pre-edit` hook from before
//! `pre-tool` existed is reported as outdated: it shows notes before an edit
//! but enforces none.
//!
//! Read only. It never writes an agent's config and never runs `claude` or
//! `codex`. Locations follow each agent's documentation. Claude Code keeps MCP
//! servers in `.claude.json` (top level, and per project) and hooks in
//! `settings.json`. With `CLAUDE_CONFIG_DIR` set, both live in that folder;
//! otherwise `.claude.json` sits beside `~/.claude`. Codex keeps MCP servers in
//! `$CODEX_HOME/config.toml` and hooks there or in `hooks.json`. A broken file
//! is reported on its line, and the command always exits 0.
//!
//! An enabled plugin that runs a piece counts too, reported as
//! `connected (plugin <name>)` with no fix, since adding the piece by hand as
//! well would run it twice. Claude Code records enabled plugins in
//! `settings.json` `enabledPlugins` and where each is installed in
//! `plugins/installed_plugins.json`; Codex records them as
//! `[plugins."<name>@<marketplace>"]` in `config.toml`, installed under
//! `plugins/cache/<marketplace>/<name>/<version>`. A plugin's hooks are in
//! `hooks/hooks.json` and its MCP servers in `.mcp.json`, unless its manifest
//! names other files.
//!
//! Codex runs a hook from `config.toml` or `hooks.json` only once the person
//! has approved it: `config.toml` must hold
//! `[hooks.state."<file>:<event>:<group>:<handler>"]` with a `trusted_hash`
//! equal to the hook's current hash (see [`codex_hook_hash`]). A Lore hook
//! without one is reported as configured but not approved, with how to approve
//! it in Codex, never as connected.

use std::path::{Path, PathBuf};

use lore_core::paths::env_dir;
use serde_json::Value;

/// A piece's status, and how to add it when it is not connected.
type Piece = (String, Option<String>);
/// Reads one agent's config folder into its pieces, in [`LABELS`] order.
type Pieces<'a> = &'a dyn Fn(&Path) -> [Piece; 4];

/// Each piece's label in the report, and its name in a fix.
const LABELS: [(&str, &str); 4] = [
    ("MCP server", "MCP server"),
    ("Session hook", "session hook"),
    ("Prompt hook", "prompt hook"),
    ("Tool hook", "tool hook"),
];
/// Each hook's agent event and `lorectl hook` event, in [`LABELS`] order.
const HOOKS: [(&str, &str); 3] = [
    ("SessionStart", "session-start"),
    ("UserPromptSubmit", "prompt"),
    ("PreToolUse", "pre-tool"),
];
/// The `lorectl hook` event the tool hook ran before `pre-tool`.
const OLD_TOOL_HOOK: &str = "pre-edit";
/// Each hook's event as Codex names it in a trust key, in [`HOOKS`] order.
const CODEX_EVENT_KEYS: [&str; 3] = ["session_start", "user_prompt_submit", "pre_tool_use"];
/// Codex's default `additionalContextLimit`, left out of a hook's hash.
const CODEX_DEFAULT_CONTEXT_LIMIT: u64 = 2_500;

/// Print the report for the running binary. Always exit 0.
pub fn run() -> u8 {
    let exe = std::env::current_exe().unwrap_or_else(|_| "lorectl".into());
    print!("{}", report(&exe.to_string_lossy()));
    crate::exit::OK
}

// How to add each piece. `{exe}` and `{hook}` are JSON strings, which are also
// TOML strings; `{exe_sh}` is quoted for a shell.
const CLAUDE_MCP: &str = "run:\n  claude mcp add --scope user lore -- {exe_sh} mcp";
const CLAUDE_HOOKS: [&str; 3] = [
    r#"merge this into {file}:
  {"hooks": {"SessionStart": [{"hooks": [{"type": "command", "command": {hook}}]}]}}"#,
    r#"merge this into {file}:
  {"hooks": {"UserPromptSubmit": [{"hooks": [{"type": "command", "command": {hook}, "timeout": 5}]}]}}"#,
    r#"merge this into {file}:
  {"hooks": {"PreToolUse": [{"matcher": "Bash|Write|Edit|MultiEdit|NotebookEdit", "hooks": [{"type": "command", "command": {hook}, "timeout": 5}]}]}}"#,
];
const CODEX_MCP: &str = r#"add this to {file}:
  [mcp_servers.lore]
  command = {exe}
  args = ["mcp"]"#;
const CODEX_HOOKS: [&str; 3] = [
    r#"add this to {file}:
  [[hooks.SessionStart]]
  [[hooks.SessionStart.hooks]]
  type = "command"
  command = {hook}"#,
    r#"add this to {file}:
  [[hooks.UserPromptSubmit]]
  [[hooks.UserPromptSubmit.hooks]]
  type = "command"
  command = {hook}
  timeout = 5"#,
    r#"add this to {file}:
  [[hooks.PreToolUse]]
  matcher = "Bash|apply_patch"
  [[hooks.PreToolUse.hooks]]
  type = "command"
  command = {hook}
  timeout = 5"#,
];
const CODEX_APPROVE: &str = "approve it in Codex: start `codex` in a terminal and, at \"Hooks need review\", choose \"Trust all and continue\" (or \"Review hooks\" to trust each one); in a running session, open `/hooks`. Codex records the approval in {file}; lorectl never writes it.";

fn report(exe: &str) -> String {
    let fill = |template: &str, file: &Path, event: &str| {
        let hook = format!("{} hook {event}", sh(exe));
        template
            .replace("{exe_sh}", &sh(exe))
            .replace("{exe}", &json(exe))
            .replace("{hook}", &json(&hook))
            .replace("{file}", &file.to_string_lossy())
    };
    let claude = |dir: &Path| {
        let mcp = match env_dir("CLAUDE_CONFIG_DIR") {
            Some(_) => dir.join(".claude.json"),
            None => dir.with_file_name(".claude.json"),
        };
        let settings = [(dir.join("settings.json"), false)];
        let plugins = claude_plugins(dir);
        let hook = |i: usize| {
            let (event, sub) = HOOKS[i];
            let fix = fill(CLAUDE_HOOKS[i], &settings[0].0, sub);
            let find = |root: &Value| hook_entry(root, event, sub);
            let piece = check(&settings, &find, fix);
            outdated(i, &settings, by_plugin(piece, &plugins, false, &find))
        };
        let mcp_fix = fill(CLAUDE_MCP, &mcp, "");
        let piece = check(&[(mcp, false)], &claude_mcp, mcp_fix);
        [
            by_plugin(piece, &plugins, true, &plugin_mcp),
            hook(0),
            hook(1),
            hook(2),
        ]
    };
    let codex = |dir: &Path| {
        let config = dir.join("config.toml");
        let files = [(config.clone(), true), (dir.join("hooks.json"), false)];
        let plugins = codex_plugins(dir);
        let hook = |i: usize| {
            let (event, sub) = HOOKS[i];
            let fix = fill(CODEX_HOOKS[i], &config, sub);
            let find = |root: &Value| hook_entry(root, event, sub);
            let approve = fill(CODEX_APPROVE, &config, sub);
            let piece = codex_trust(check(&files, &find, fix), &files, i, approve);
            outdated(i, &files, by_plugin(piece, &plugins, false, &find))
        };
        let piece = check(&files[..1], &codex_mcp, fill(CODEX_MCP, &config, ""));
        [
            by_plugin(piece, &plugins, true, &plugin_mcp),
            hook(0),
            hook(1),
            hook(2),
        ]
    };
    let agents: [(&str, &str, &str, Pieces<'_>); 2] = [
        ("Claude Code", "CLAUDE_CONFIG_DIR", ".claude", &claude),
        ("Codex", "CODEX_HOME", ".codex", &codex),
    ];

    let (mut out, mut fixes, mut found) = (String::new(), String::new(), false);
    for (name, var, home_sub, pieces) in agents {
        // `$VAR`, else `$HOME/<home_sub>`, the way the scanner resolves it.
        let dir = env_dir(var).or_else(|| env_dir("HOME").map(|home| home.join(home_sub)));
        out.push_str(name);
        if let Some(dir) = &dir {
            out.push_str(&format!("  {}", dir.display()));
        }
        out.push('\n');
        let pieces = dir.as_deref().filter(|dir| dir.is_dir()).map(pieces);
        found |= pieces.is_some();
        let missing = || ("agent not found".to_string(), None);
        let pieces = pieces.unwrap_or_else(|| [missing(), missing(), missing(), missing()]);
        for ((label, noun), (status, fix)) in LABELS.into_iter().zip(pieces) {
            out.push_str(&format!("  {label:<12}  {status}\n"));
            if let Some(fix) = fix {
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

/// `piece` for hook `i`, reported as outdated when it is the tool hook, it is
/// not connected, and one of `files` still runs the `pre-edit` hook.
fn outdated(i: usize, files: &[(PathBuf, bool)], piece: Piece) -> Piece {
    let (event, _) = HOOKS[2];
    if i != 2 || piece.1.is_none() || piece.0.starts_with("configured") {
        return piece;
    }
    let old = check(
        files,
        &|root| hook_entry(root, event, OLD_TOOL_HOOK),
        String::new(),
    );
    match old.0.strip_prefix("connected (") {
        Some(at) => (
            format!(
                "outdated ({}; runs `hook pre-edit`, replace it with the entry below)",
                at.trim_end_matches(')')
            ),
            piece.1,
        ),
        None => piece,
    }
}

/// The first file (JSON, or TOML when flagged) where `find` names an entry.
fn check(files: &[(PathBuf, bool)], find: &dyn Fn(&Value) -> Option<String>, fix: String) -> Piece {
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

/// `piece`, a Codex hook found in `files`, unless Codex would skip it: no
/// `lorectl hook` entry for hook `i` has an approval in `config.toml`
/// (`files[0]`) whose `trusted_hash` matches the entry, or the approved one is
/// disabled. Then it is reported as configured but not approved, with
/// `approve` as the fix. Another hook's approval, such as another tool's at the
/// same event's index 0, never counts: the key names the entry's own indices.
fn codex_trust(piece: Piece, files: &[(PathBuf, bool)], i: usize, approve: String) -> Piece {
    if piece.1.is_some() {
        return piece;
    }
    let (event, sub) = HOOKS[i];
    let config = read_config(&files[0].0, true).unwrap_or_default();
    let states = &config["hooks"]["state"];
    let (mut changed, mut disabled) = (false, false);
    for (path, is_toml) in files {
        let Some(root) = read_config(path, *is_toml) else {
            continue;
        };
        let groups = root["hooks"][event].as_array().into_iter().flatten();
        for (g, group) in groups.enumerate() {
            let hooks = group["hooks"].as_array().into_iter().flatten();
            for (h, hook) in hooks.enumerate() {
                let command = hook["command"].as_str().unwrap_or_default();
                if !runs_lorectl(command, &["hook", sub]) {
                    continue;
                }
                let key = format!("{}:{}:{g}:{h}", path.display(), CODEX_EVENT_KEYS[i]);
                let state = &states[key.as_str()];
                match state["trusted_hash"].as_str() {
                    Some(hash) if hash == codex_hook_hash(i, group, hook) => {
                        if state["enabled"].as_bool() == Some(false) {
                            disabled = true;
                        } else {
                            return piece;
                        }
                    }
                    Some(_) => changed = true,
                    None => {}
                }
            }
        }
    }
    let at = piece.0.strip_prefix("connected (").unwrap_or(&piece.0);
    let at = at.strip_suffix(')').unwrap_or(at);
    let status = if disabled {
        format!("configured but disabled in Codex (Codex skips it; {at})")
    } else if changed {
        format!("configured but not approved (changed since approved; Codex skips it; {at})")
    } else {
        format!("configured but not approved (Codex skips it; {at})")
    };
    (status, Some(approve))
}

/// The hash Codex trusts a hook by: SHA-256 over the compact, key-sorted JSON
/// of `{event_name, matcher, hooks: [handler]}`, the handler normalized the way
/// Codex normalizes it (`timeout` defaulting to 600, `async` to false, an
/// `additionalContextLimit` of the default left out, a prompt hook's matcher
/// ignored). Mirrors `hook_hash` in openai/codex
/// `codex-rs/hooks/src/engine/discovery.rs` (codex-cli 0.162).
fn codex_hook_hash(i: usize, group: &Value, hook: &Value) -> String {
    use sha2::{Digest, Sha256};
    // Keys go in sorted order, so the JSON is sorted with or without
    // serde_json's `preserve_order`.
    let mut handler = serde_json::Map::new();
    let limit = hook["additionalContextLimit"].as_u64();
    if let Some(limit) = limit.filter(|limit| *limit != CODEX_DEFAULT_CONTEXT_LIMIT) {
        handler.insert("additionalContextLimit".into(), limit.into());
    }
    let is_async = hook["async"].as_bool().unwrap_or(false);
    handler.insert("async".into(), is_async.into());
    handler.insert("command".into(), hook["command"].clone());
    if let Some(message) = hook["statusMessage"].as_str() {
        handler.insert("statusMessage".into(), message.into());
    }
    let timeout = hook["timeout"].as_u64().unwrap_or(600).max(1);
    handler.insert("timeout".into(), timeout.into());
    handler.insert("type".into(), "command".into());
    let mut identity = serde_json::Map::new();
    identity.insert("event_name".into(), CODEX_EVENT_KEYS[i].into());
    identity.insert("hooks".into(), Value::Array(vec![handler.into()]));
    let (event, _) = HOOKS[i];
    if let Some(matcher) = group["matcher"].as_str() {
        if event != "UserPromptSubmit" {
            identity.insert("matcher".into(), matcher.into());
        }
    }
    let digest = Sha256::digest(Value::Object(identity).to_string());
    let hex: String = digest.iter().map(|byte| format!("{byte:02x}")).collect();
    format!("sha256:{hex}")
}

/// A JSON file, or a TOML one when `is_toml`, if it reads and parses.
fn read_config(path: &Path, is_toml: bool) -> Option<Value> {
    let text = std::fs::read_to_string(path).ok()?;
    if is_toml {
        toml::from_str(&text).ok()
    } else {
        serde_json::from_str(&text).ok()
    }
}

/// An enabled plugin: its name, and its MCP server and hooks files.
struct Plugin {
    name: String,
    mcp: PathBuf,
    hooks: PathBuf,
}

/// `piece`, or connected through the first plugin whose MCP servers (when
/// `mcp`) or hooks file `find` names an entry in, when `piece` is not
/// connected itself.
fn by_plugin(
    piece: Piece,
    plugins: &[Plugin],
    mcp: bool,
    find: &dyn Fn(&Value) -> Option<String>,
) -> Piece {
    if piece.1.is_none() {
        return piece;
    }
    let provides = |plugin: &&Plugin| {
        let file = if mcp { &plugin.mcp } else { &plugin.hooks };
        read_json(file).is_some_and(|root| find(&root).is_some())
    };
    match plugins.iter().find(provides) {
        Some(plugin) => (format!("connected (plugin {})", plugin.name), None),
        None => piece,
    }
}

fn read_json(path: &Path) -> Option<Value> {
    serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()
}

/// A plugin's `.mcp.json`: servers under `mcpServers`, or the servers alone.
fn plugin_mcp(root: &Value) -> Option<String> {
    let servers = root.get("mcpServers").unwrap_or(root);
    mcp_entry(Some(servers), "mcpServers")
}

/// The plugin `key` (`<name>@<marketplace>`) installed at `dir`, with its
/// files named by the manifest in `dir/<manifest>/plugin.json`, if any.
fn plugin(key: &str, dir: &Path, manifest: &str) -> Plugin {
    let manifest = read_json(&dir.join(manifest).join("plugin.json"));
    let file = |field: &str, default: &str| {
        let named = manifest.as_ref().and_then(|m| m[field].as_str());
        dir.join(named.unwrap_or(default))
    };
    Plugin {
        name: key.split('@').next().unwrap_or(key).to_string(),
        mcp: file("mcpServers", ".mcp.json"),
        hooks: file("hooks", "hooks/hooks.json"),
    }
}

/// Claude Code's plugins enabled in `settings.json`, where
/// `plugins/installed_plugins.json` says each is installed.
fn claude_plugins(dir: &Path) -> Vec<Plugin> {
    let (Some(settings), Some(installed)) = (
        read_json(&dir.join("settings.json")),
        read_json(&dir.join("plugins/installed_plugins.json")),
    ) else {
        return Vec::new();
    };
    let Some(enabled) = settings["enabledPlugins"].as_object() else {
        return Vec::new();
    };
    let enabled = enabled.iter().filter(|(_, on)| on.as_bool() == Some(true));
    enabled
        .flat_map(|(key, _)| {
            // Version 2 keeps a list of installs per plugin; version 1 one.
            let installs = &installed["plugins"][key];
            let installs = installs
                .as_array()
                .cloned()
                .unwrap_or_else(|| vec![installs.clone()]);
            installs.into_iter().filter_map(move |install| {
                let path = install["installPath"].as_str()?;
                Some(plugin(key, Path::new(path), ".claude-plugin"))
            })
        })
        .collect()
}

/// Codex's plugins enabled in `config.toml`, each at its most recently
/// written version under `plugins/cache/<marketplace>/<name>`.
fn codex_plugins(dir: &Path) -> Vec<Plugin> {
    let Some(config) = std::fs::read_to_string(dir.join("config.toml"))
        .ok()
        .and_then(|text| toml::from_str::<Value>(&text).ok())
    else {
        return Vec::new();
    };
    let Some(plugins) = config["plugins"].as_object() else {
        return Vec::new();
    };
    let enabled = plugins
        .iter()
        .filter(|(_, p)| p["enabled"].as_bool() != Some(false));
    enabled
        .filter_map(|(key, _)| {
            let (name, market) = key.split_once('@')?;
            let versions = std::fs::read_dir(dir.join("plugins/cache").join(market).join(name));
            let newest = versions
                .ok()?
                .filter_map(Result::ok)
                .filter(|entry| entry.path().is_dir())
                .max_by_key(|entry| entry.metadata().and_then(|m| m.modified()).ok())?;
            Some(plugin(key, &newest.path(), ".codex-plugin"))
        })
        .collect()
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

/// `hooks.<event>[].hooks[].command` running `lorectl hook <sub>`, the shape
/// both agents share.
fn hook_entry(root: &Value, event: &str, sub: &str) -> Option<String> {
    let groups = root["hooks"][event].as_array()?.iter();
    let hooks = groups
        .filter_map(|group| group["hooks"].as_array())
        .flatten();
    let mut commands = hooks.filter_map(|hook| hook["command"].as_str());
    commands
        .any(|command| runs_lorectl(command, &["hook", sub]))
        .then(|| format!("hooks.{event}"))
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn runs_lorectl_matches_subcommands_and_boundaries() {
        assert!(runs_lorectl("lorectl mcp", &["mcp"]));
        assert!(runs_lorectl("/usr/bin/lorectl mcp", &["mcp"]));
        assert!(runs_lorectl("'/path/to/lorectl' mcp", &["mcp"]));
        assert!(runs_lorectl(
            "lorectl hook session-start",
            &["hook", "session-start"]
        ));
        assert!(runs_lorectl(
            "/bin/lorectl --flag hook session-start",
            &["hook", "session-start"]
        ));

        // Negative cases
        assert!(!runs_lorectl("lorectl", &["mcp"]));
        assert!(!runs_lorectl("lorectl query", &["mcp"]));
        assert!(!runs_lorectl("not_lorectl mcp", &["mcp"]));
        assert!(!runs_lorectl("lorectl.bak mcp", &["mcp"]));
    }

    #[test]
    fn sh_quotes_only_when_necessary() {
        assert_eq!(sh("/usr/local/bin/lorectl"), "/usr/local/bin/lorectl");
        assert_eq!(
            sh("foo-bar_1.0+v1:test@dir,sub"),
            "foo-bar_1.0+v1:test@dir,sub"
        );
        assert_eq!(sh(""), "''");
        assert_eq!(sh("path with space"), "'path with space'");
        assert_eq!(sh("can't"), r"'can'\''t'");
        assert_eq!(sh("$(cmd)"), "'$(cmd)'");
    }

    #[test]
    fn json_string_escaping() {
        assert_eq!(json("lorectl"), "\"lorectl\"");
        assert_eq!(json("a \"quoted\" word"), "\"a \\\"quoted\\\" word\"");
    }

    #[test]
    fn claude_mcp_finds_top_level_and_project_entries() {
        let top = json!({
            "mcpServers": {
                "lore": { "command": "lorectl", "args": ["mcp"] }
            }
        });
        assert_eq!(claude_mcp(&top), Some("mcpServers.lore".into()));

        let project = json!({
            "projects": {
                "/workspace/repo": {
                    "mcpServers": {
                        "lore": { "command": "/usr/local/bin/lorectl", "args": ["mcp"] }
                    }
                }
            }
        });
        assert_eq!(
            claude_mcp(&project),
            Some("projects[\"/workspace/repo\"].mcpServers.lore".into())
        );

        let none = json!({ "mcpServers": { "other": { "command": "other", "args": [] } } });
        assert_eq!(claude_mcp(&none), None);
    }

    #[test]
    fn codex_mcp_finds_mcp_servers_entry() {
        let root = json!({
            "mcp_servers": {
                "lore": { "command": "lorectl", "args": ["mcp"] }
            }
        });
        assert_eq!(codex_mcp(&root), Some("mcp_servers.lore".into()));

        let missing = json!({ "mcp_servers": {} });
        assert_eq!(codex_mcp(&missing), None);
    }

    /// Vectors computed independently of this code by Codex's scheme, which
    /// reproduced every approval in a real Codex 0.162 `config.toml`.
    #[test]
    fn codex_hook_hash_matches_codexs_scheme() {
        let tool = json!({"type": "command", "command": "lorectl hook pre-tool", "timeout": 5});
        assert_eq!(
            codex_hook_hash(2, &json!({"matcher": "Bash|apply_patch"}), &tool),
            "sha256:9f99b1781c13b94d7902c915cb9373d0aa1482fae23cd333c3d37614ddf5c69b"
        );
        // A prompt hook's matcher is ignored.
        let prompt = json!({"type": "command", "command": "lorectl hook prompt", "timeout": 5});
        assert_eq!(
            codex_hook_hash(1, &json!({"matcher": "ignored"}), &prompt),
            "sha256:7e59890e2962dd2649673688e1fe5d03700df447aabcf360696dc44dfdf5c29e"
        );
        // The default timeout and context limit hash as if left out.
        let session = json!({
            "type": "command",
            "command": "lorectl hook session-start",
            "statusMessage": "Lore",
            "additionalContextLimit": 2500,
        });
        assert_eq!(
            codex_hook_hash(0, &json!({}), &session),
            "sha256:67a75affecc9a4d66d6f75080acd637eade4d2e5167cf5cc3ec047a017cc8ae6"
        );
    }

    #[test]
    fn session_hook_finds_hook_command() {
        let root = json!({
            "hooks": {
                "SessionStart": [
                    {
                        "hooks": [
                            { "type": "command", "command": "lorectl hook session-start" }
                        ]
                    }
                ]
            }
        });
        assert_eq!(
            hook_entry(&root, "SessionStart", "session-start"),
            Some("hooks.SessionStart".into())
        );
        assert_eq!(hook_entry(&root, "UserPromptSubmit", "prompt"), None);

        let unrelated = json!({
            "hooks": {
                "SessionStart": [
                    {
                        "hooks": [
                            { "type": "command", "command": "echo hello" }
                        ]
                    }
                ]
            }
        });
        assert_eq!(
            hook_entry(&unrelated, "SessionStart", "session-start"),
            None
        );
    }
}
