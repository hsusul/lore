//! `lorectl connect` end to end: what it reports for each agent's config, and
//! the exact commands it prints. The binary path and the temp root are masked.
#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::fs;
use std::path::Path;
use std::process::Command;

/// Run `lorectl connect` with every agent location under `tmp`, and return its
/// stdout with the binary path shown as `<lorectl>`, bare or quoted for a
/// shell, and `tmp` as `<tmp>`.
fn connect(tmp: &Path, configure: impl FnOnce(&mut Command)) -> String {
    let exe = env!("CARGO_BIN_EXE_lorectl");
    let canonical = fs::canonicalize(exe).unwrap();
    let canonical = canonical.to_string_lossy();
    let mut out = run_connect(Path::new(exe), tmp, configure);
    for path in [&*canonical, exe] {
        // connect quotes a path with a space for the shell; see
        // `a_lorectl_path_with_a_space_is_quoted_for_the_shell`.
        out = out
            .replace(&format!("'{path}'"), "<lorectl>")
            .replace(path, "<lorectl>");
    }
    out
}

/// Run the `lorectl` at `exe` as `lorectl connect` with every agent location
/// under `tmp`, and return its stdout with `tmp` shown as `<tmp>`.
fn run_connect(exe: &Path, tmp: &Path, configure: impl FnOnce(&mut Command)) -> String {
    let mut command = Command::new(exe);
    command
        .arg("connect")
        .env("HOME", tmp.join("home"))
        .env("CLAUDE_CONFIG_DIR", tmp.join("claude"))
        .env("CODEX_HOME", tmp.join("codex"))
        .env_remove("LORE_ARCHIVE_DIR");
    configure(&mut command);
    let output = command.output().expect("spawn lorectl");
    assert_eq!(output.status.code(), Some(0));
    assert_eq!(String::from_utf8(output.stderr).unwrap(), "");
    String::from_utf8(output.stdout)
        .unwrap()
        .replace(&tmp.to_string_lossy().into_owned(), "<tmp>")
}

#[test]
fn a_lorectl_path_with_a_space_is_quoted_for_the_shell() {
    let tmp = tempfile::tempdir().unwrap();
    let tmp = fs::canonicalize(tmp.path()).unwrap();
    fs::create_dir_all(tmp.join("claude")).unwrap();
    fs::create_dir_all(tmp.join("codex")).unwrap();
    let bin = tmp.join("Lore Tools");
    fs::create_dir_all(&bin).unwrap();
    let exe = bin.join("lorectl");
    fs::copy(env!("CARGO_BIN_EXE_lorectl"), &exe).unwrap();
    let out = run_connect(&exe, &tmp, |_| {});
    for line in [
        "  claude mcp add --scope user lore -- '<tmp>/Lore Tools/lorectl' mcp\n",
        "\"command\": \"'<tmp>/Lore Tools/lorectl' hook session-start\"",
        "  command = \"<tmp>/Lore Tools/lorectl\"\n",
        "  command = \"'<tmp>/Lore Tools/lorectl' hook session-start\"\n",
    ] {
        assert!(out.contains(line), "missing {line:?} in:\n{out}");
    }
}

const CLAUDE_MCP_FIX: &str = "
To connect Claude Code's MCP server, run:
  claude mcp add --scope user lore -- <lorectl> mcp
";

/// Claude Code's prompt and tool hook fixes, for settings in `<tmp>/<dir>`.
fn claude_hook_fixes(dir: &str) -> String {
    format!(
        "
To connect Claude Code's prompt hook, merge this into <tmp>/{dir}/settings.json:
  {{\"hooks\": {{\"UserPromptSubmit\": [{{\"hooks\": [{{\"type\": \"command\", \"command\": \"<lorectl> hook prompt\", \"timeout\": 5}}]}}]}}}}

To connect Claude Code's tool hook, merge this into <tmp>/{dir}/settings.json:
  {{\"hooks\": {{\"PreToolUse\": [{{\"matcher\": \"Bash|Write|Edit|MultiEdit|NotebookEdit\", \"hooks\": [{{\"type\": \"command\", \"command\": \"<lorectl> hook pre-tool\", \"timeout\": 5}}]}}]}}}}
"
    )
}

const CODEX_FIXES: &str = "
To connect Codex's MCP server, add this to <tmp>/codex/config.toml:
  [mcp_servers.lore]
  command = \"<lorectl>\"
  args = [\"mcp\"]

To connect Codex's session hook, add this to <tmp>/codex/config.toml:
  [[hooks.SessionStart]]
  [[hooks.SessionStart.hooks]]
  type = \"command\"
  command = \"<lorectl> hook session-start\"

To connect Codex's prompt hook, add this to <tmp>/codex/config.toml:
  [[hooks.UserPromptSubmit]]
  [[hooks.UserPromptSubmit.hooks]]
  type = \"command\"
  command = \"<lorectl> hook prompt\"
  timeout = 5

To connect Codex's tool hook, add this to <tmp>/codex/config.toml:
  [[hooks.PreToolUse]]
  matcher = \"Bash|apply_patch\"
  [[hooks.PreToolUse.hooks]]
  type = \"command\"
  command = \"<lorectl> hook pre-tool\"
  timeout = 5
";

/// One agent's four rows, all with `status`.
fn rows(status: &str) -> String {
    format!(
        "  MCP server    {status}\n  Session hook  {status}\n  Prompt hook   {status}\n  Tool hook     {status}\n"
    )
}

#[test]
fn help_connect_prints_the_connect_page() {
    let output = Command::new(env!("CARGO_BIN_EXE_lorectl"))
        .args(["help", "connect"])
        .output()
        .expect("spawn lorectl");
    assert_eq!(output.status.code(), Some(0));
    let page = String::from_utf8(output.stdout).unwrap();
    assert_eq!(page.lines().next(), Some("Connect (lorectl connect):"));
    for name in ["CLAUDE_CONFIG_DIR", "CODEX_HOME", "Never writes"] {
        assert!(
            page.contains(name),
            "`help connect` omits `{name}`:\n{page}"
        );
    }
}

#[test]
fn nothing_installed_reports_both_agents_missing() {
    let tmp = tempfile::tempdir().unwrap();
    let missing = rows("agent not found");
    assert_eq!(
        connect(tmp.path(), |_| {}),
        format!(
            "\
Claude Code  <tmp>/claude
{missing}\
Codex  <tmp>/codex
{missing}
Neither agent was found. Install one, then run `lorectl connect` again.
"
        )
    );
}

#[test]
fn both_agents_present_with_nothing_connected() {
    let tmp = tempfile::tempdir().unwrap();
    fs::create_dir_all(tmp.path().join("claude")).unwrap();
    fs::create_dir_all(tmp.path().join("codex")).unwrap();
    // A hook and a server that are not Lore's must not count.
    fs::write(
        tmp.path().join("claude/settings.json"),
        r#"{"hooks": {"SessionStart": [{"hooks": [{"type": "command", "command": "lorectl status"}]}]}}"#,
    )
    .unwrap();
    fs::write(
        tmp.path().join("codex/config.toml"),
        "model = \"o3\"\n[mcp_servers.other]\ncommand = \"node\"\nargs = [\"mcp\"]\n",
    )
    .unwrap();
    assert_eq!(
        connect(tmp.path(), |_| {}),
        format!(
            "\
Claude Code  <tmp>/claude
{none}\
Codex  <tmp>/codex
{none}\
{CLAUDE_MCP_FIX}
To connect Claude Code's session hook, merge this into <tmp>/claude/settings.json:
  {{\"hooks\": {{\"SessionStart\": [{{\"hooks\": [{{\"type\": \"command\", \"command\": \"<lorectl> hook session-start\"}}]}}]}}}}
{claude_hooks}{CODEX_FIXES}",
            none = rows("not connected"),
            claude_hooks = claude_hook_fixes("claude"),
        )
    );
}

#[test]
fn the_printed_snippets_connect_once_pasted() {
    let tmp = tempfile::tempdir().unwrap();
    fs::create_dir_all(tmp.path().join("claude")).unwrap();
    fs::create_dir_all(tmp.path().join("codex")).unwrap();
    let out = connect(tmp.path(), |_| {}).replace("<lorectl>", "/opt/bin/lorectl");
    // The indented lines after each "To connect ..." heading, unindented.
    let snippet = |heading: &str| -> String {
        let start = out.find(heading).unwrap();
        out[start..]
            .lines()
            .skip(1)
            .take_while(|line| line.starts_with("  "))
            .map(|line| format!("{}\n", &line[2..]))
            .collect()
    };
    // Each Claude Code snippet is a whole settings object; merge their hooks.
    let mut settings = serde_json::json!({ "hooks": {} });
    for hook in ["session", "prompt", "tool"] {
        let text = snippet(&format!("To connect Claude Code's {hook} hook"));
        let piece: serde_json::Value = serde_json::from_str(&text).unwrap();
        for (event, groups) in piece["hooks"].as_object().unwrap() {
            settings["hooks"][event] = groups.clone();
        }
    }
    fs::write(
        tmp.path().join("claude/settings.json"),
        settings.to_string(),
    )
    .unwrap();
    let codex: String = ["MCP server", "session hook", "prompt hook", "tool hook"]
        .iter()
        .map(|piece| snippet(&format!("To connect Codex's {piece}")))
        .collect();
    fs::write(tmp.path().join("codex/config.toml"), codex).unwrap();
    let again = connect(tmp.path(), |_| {});
    assert_eq!(
        again.lines().take(10).collect::<Vec<_>>(),
        [
            "Claude Code  <tmp>/claude",
            "  MCP server    not connected",
            "  Session hook  connected (<tmp>/claude/settings.json, hooks.SessionStart)",
            "  Prompt hook   connected (<tmp>/claude/settings.json, hooks.UserPromptSubmit)",
            "  Tool hook     connected (<tmp>/claude/settings.json, hooks.PreToolUse)",
            "Codex  <tmp>/codex",
            "  MCP server    connected (<tmp>/codex/config.toml, mcp_servers.lore)",
            // Pasted but not yet approved in Codex, which skips it until then.
            "  Session hook  configured but not approved (Codex skips it; <tmp>/codex/config.toml, hooks.SessionStart)",
            "  Prompt hook   configured but not approved (Codex skips it; <tmp>/codex/config.toml, hooks.UserPromptSubmit)",
            "  Tool hook     configured but not approved (Codex skips it; <tmp>/codex/config.toml, hooks.PreToolUse)",
        ]
    );
}

#[test]
fn claude_code_connected_through_user_mcp_servers_and_a_session_start_hook() {
    let tmp = tempfile::tempdir().unwrap();
    fs::create_dir_all(tmp.path().join("claude")).unwrap();
    fs::write(
        tmp.path().join("claude/.claude.json"),
        r#"{"mcpServers": {"lore": {"type": "stdio", "command": "/opt/lore bin/lorectl", "args": ["mcp"]}}}"#,
    )
    .unwrap();
    fs::write(
        tmp.path().join("claude/settings.json"),
        r#"{"hooks": {
            "SessionStart": [{"matcher": "startup", "hooks": [{"type": "command", "command": "'/opt/lore bin/lorectl' hook session-start"}]}],
            "UserPromptSubmit": [{"hooks": [{"type": "command", "command": "lorectl hook prompt"}]}],
            "PreToolUse": [{"matcher": "Bash", "hooks": [{"type": "command", "command": "other"}]},
                {"matcher": "Bash|Edit|Write", "hooks": [{"type": "command", "command": "lorectl hook pre-tool"}]}]
        }}"#,
    )
    .unwrap();
    assert_eq!(
        connect(tmp.path(), |_| {}),
        format!(
            "\
Claude Code  <tmp>/claude
  MCP server    connected (<tmp>/claude/.claude.json, mcpServers.lore)
  Session hook  connected (<tmp>/claude/settings.json, hooks.SessionStart)
  Prompt hook   connected (<tmp>/claude/settings.json, hooks.UserPromptSubmit)
  Tool hook     connected (<tmp>/claude/settings.json, hooks.PreToolUse)
Codex  <tmp>/codex
{}",
            rows("agent not found")
        )
    );
}

#[test]
fn claude_code_project_mcp_servers_count_too() {
    let tmp = tempfile::tempdir().unwrap();
    fs::create_dir_all(tmp.path().join("claude")).unwrap();
    fs::write(
        tmp.path().join("claude/.claude.json"),
        r#"{"projects": {"/src/app": {"mcpServers": {"archive": {"command": "lorectl", "args": ["mcp"]}}}}}"#,
    )
    .unwrap();
    let out = connect(tmp.path(), |_| {});
    assert!(
        out.contains(
            "  MCP server    connected (<tmp>/claude/.claude.json, projects[\"/src/app\"].mcpServers.archive)\n"
        ),
        "{out}"
    );
}

#[test]
fn codex_connected_through_config_toml_once_approved() {
    let tmp = tempfile::tempdir().unwrap();
    fs::create_dir_all(tmp.path().join("codex")).unwrap();
    let config = tmp.path().join("codex/config.toml");
    // Codex's own approvals for these three hooks: the hashes it records,
    // computed independently of lorectl (and checked against real Codex
    // approvals) by the scheme in `codex_hook_hash`.
    let approvals = [
        (
            "session_start",
            "ddf274727b327e42383cf897feaede99e2d5eafe353a82873fe293759a795852",
        ),
        (
            "user_prompt_submit",
            "972630c620ab59207c40254e0a8bad0280d6aaf5e60c5b984d8ea538c92ded08",
        ),
        (
            "pre_tool_use",
            "da7d8020b5cef74096679d984b0d22e605fdd6140bb99d0a65f569211696237e",
        ),
    ]
    .map(|(event, hash)| {
        format!(
            "[hooks.state.\"{}:{event}:0:0\"]\ntrusted_hash = \"sha256:{hash}\"\n\n",
            config.display()
        )
    })
    .concat();
    fs::write(
        &config,
        approvals
            + "\
[mcp_servers.other]
command = \"node\"

[mcp_servers.memory]
command = \"lorectl\"
args = [\"--archive\", \"/data/lore\", \"mcp\"]

[[hooks.SessionStart]]
matcher = \"startup|resume\"

[[hooks.SessionStart.hooks]]
type = \"command\"
command = \"/usr/local/bin/lorectl hook session-start\"

[[hooks.UserPromptSubmit]]
[[hooks.UserPromptSubmit.hooks]]
type = \"command\"
command = \"lorectl hook prompt\"

[[hooks.PreToolUse]]
matcher = \"Bash|apply_patch\"
[[hooks.PreToolUse.hooks]]
type = \"command\"
command = \"lorectl hook pre-tool\"
",
    )
    .unwrap();
    assert_eq!(
        connect(tmp.path(), |_| {}),
        format!(
            "\
Claude Code  <tmp>/claude
{}\
Codex  <tmp>/codex
  MCP server    connected (<tmp>/codex/config.toml, mcp_servers.memory)
  Session hook  connected (<tmp>/codex/config.toml, hooks.SessionStart)
  Prompt hook   connected (<tmp>/codex/config.toml, hooks.UserPromptSubmit)
  Tool hook     connected (<tmp>/codex/config.toml, hooks.PreToolUse)
",
            rows("agent not found")
        )
    );
}

#[test]
fn a_pre_edit_hook_is_reported_as_outdated() {
    // A config from before `pre-tool` shows notes before an edit but
    // enforces none, so it is not connected, and the fix replaces it.
    let tmp = tempfile::tempdir().unwrap();
    fs::create_dir_all(tmp.path().join("claude")).unwrap();
    fs::write(
        tmp.path().join("claude/settings.json"),
        r#"{"hooks": {"PreToolUse": [{"matcher": "Edit|Write", "hooks": [{"type": "command", "command": "lorectl hook pre-edit"}]}]}}"#,
    )
    .unwrap();
    let out = connect(tmp.path(), |_| {});
    assert!(
        out.contains(
            "  Tool hook     outdated (<tmp>/claude/settings.json, hooks.PreToolUse; runs `hook pre-edit`, replace it with the entry below)\n"
        ),
        "{out}"
    );
    assert!(out.contains("To connect Claude Code's tool hook"), "{out}");
}

#[test]
fn a_codex_hooks_json_without_approval_is_not_connected() {
    let tmp = tempfile::tempdir().unwrap();
    fs::create_dir_all(tmp.path().join("codex")).unwrap();
    fs::write(
        tmp.path().join("codex/hooks.json"),
        r#"{"hooks": {"SessionStart": [{"hooks": [{"type": "command", "command": "lorectl hook session-start"}]}]}}"#,
    )
    .unwrap();
    let out = connect(tmp.path(), |_| {});
    assert!(
        out.contains("  Session hook  configured but not approved (Codex skips it; <tmp>/codex/hooks.json, hooks.SessionStart)\n"),
        "{out}"
    );
    assert!(out.contains(CODEX_APPROVE_FIX), "{out}");
}

const CODEX_APPROVE_FIX: &str = "
To connect Codex's session hook, approve it in Codex: start `codex` in a terminal and, at \"Hooks need review\", choose \"Trust all and continue\" (or \"Review hooks\" to trust each one); in a running session, open `/hooks`. Codex records the approval in <tmp>/codex/config.toml; lorectl never writes it.
";

/// The hashes Codex records for the two hooks in
/// [`codex_session_hook_line`], computed independently of lorectl.
const OTHER_HASH: &str = "4d94f384be3cc0ea88ea38fb55a3f90184a7803dd3d96bf04f4e7f43df4fe111";
const LORE_HASH: &str = "7bbb39e07ddb4f9a42712b3bf26f5bafac476e185201cac2df0b5a7ddbe68e68";

/// Codex's session hook line, for a `hooks.json` with another tool's session
/// hook at index 0 and Lore's at index 1, and a `config.toml` holding
/// `approvals` as (`<group>:<handler>`, hash).
fn codex_session_hook_line(tmp: &Path, approvals: &[(&str, &str)]) -> String {
    fs::create_dir_all(tmp.join("codex")).unwrap();
    let hooks = tmp.join("codex/hooks.json");
    fs::write(
        &hooks,
        r#"{"hooks": {"SessionStart": [
            {"hooks": [{"type": "command", "command": "other-tool session-start", "timeout": 2}]},
            {"hooks": [{"type": "command", "command": "lorectl hook session-start", "timeout": 10}]}
        ]}}"#,
    )
    .unwrap();
    let config: String = approvals
        .iter()
        .map(|(index, hash)| {
            format!(
                "[hooks.state.\"{}:session_start:{index}\"]\ntrusted_hash = \"sha256:{hash}\"\n",
                hooks.display()
            )
        })
        .collect();
    fs::write(tmp.join("codex/config.toml"), config).unwrap();
    let out = connect(tmp, |_| {});
    let codex = out.split("\nCodex  ").nth(1).unwrap();
    let line = codex
        .lines()
        .find(|line| line.starts_with("  Session hook"));
    line.unwrap().to_string()
}

#[test]
fn another_tools_approval_at_index_0_does_not_approve_lores_hook_at_index_1() {
    let tmp = tempfile::tempdir().unwrap();
    let line = codex_session_hook_line(tmp.path(), &[("0:0", OTHER_HASH)]);
    assert_eq!(
        line,
        "  Session hook  configured but not approved (Codex skips it; <tmp>/codex/hooks.json, hooks.SessionStart)"
    );
    // Nor does the other hook's hash recorded at Lore's index.
    let line = codex_session_hook_line(tmp.path(), &[("0:0", OTHER_HASH), ("1:0", OTHER_HASH)]);
    assert_eq!(
        line,
        "  Session hook  configured but not approved (changed since approved; Codex skips it; <tmp>/codex/hooks.json, hooks.SessionStart)"
    );
}

#[test]
fn a_matching_approval_at_lores_index_connects_the_codex_hook() {
    let tmp = tempfile::tempdir().unwrap();
    let line = codex_session_hook_line(tmp.path(), &[("0:0", OTHER_HASH), ("1:0", LORE_HASH)]);
    assert_eq!(
        line,
        "  Session hook  connected (<tmp>/codex/hooks.json, hooks.SessionStart)"
    );
}

#[test]
fn a_malformed_claude_json_is_reported_on_its_line() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");
    fs::create_dir_all(home.join(".claude")).unwrap();
    fs::write(home.join(".claude.json"), "{").unwrap();
    let out = connect(tmp.path(), |command| {
        command
            .env_remove("CLAUDE_CONFIG_DIR")
            .env_remove("CODEX_HOME");
    });
    assert_eq!(
        out,
        format!(
            "\
Claude Code  <tmp>/home/.claude
  MCP server    unreadable (<tmp>/home/.claude.json, EOF while parsing an object at line 1 column 1)
  Session hook  not connected
  Prompt hook   not connected
  Tool hook     not connected
Codex  <tmp>/home/.codex
{}\
{CLAUDE_MCP_FIX}
To connect Claude Code's session hook, merge this into <tmp>/home/.claude/settings.json:
  {{\"hooks\": {{\"SessionStart\": [{{\"hooks\": [{{\"type\": \"command\", \"command\": \"<lorectl> hook session-start\"}}]}}]}}}}
{}",
            rows("agent not found"),
            claude_hook_fixes("home/.claude")
        )
    );
}

/// Copy the repository's `plugins/<name>` into `to`, as an agent installs it.
fn install_plugin(name: &str, to: &Path) {
    let from = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../plugins")
        .join(name);
    for file in [
        "hooks/hooks.json",
        ".mcp.json",
        ".claude-plugin/plugin.json",
        ".codex-plugin/plugin.json",
    ] {
        if from.join(file).exists() {
            fs::create_dir_all(to.join(file).parent().unwrap()).unwrap();
            fs::copy(from.join(file), to.join(file)).unwrap();
        }
    }
}

/// Install and enable the Lore plugin the way each agent records it: Claude
/// Code in `plugins/installed_plugins.json` and `settings.json`
/// `enabledPlugins`, Codex under `plugins/cache/<marketplace>/<name>/<version>`
/// and `[plugins."<name>@<marketplace>"]` in `config.toml`.
fn install_both_plugins(tmp: &Path, enabled: bool) {
    let claude_install = tmp.join("claude/plugins/cache/lore/lore/0.1.0");
    install_plugin("claude-code", &claude_install);
    fs::write(
        tmp.join("claude/plugins/installed_plugins.json"),
        serde_json::json!({
            "version": 2,
            "plugins": {"lore@lore": [{
                "scope": "user",
                "installPath": claude_install,
                "version": "0.1.0",
                "installedAt": "2026-11-01T00:00:00.000Z",
                "lastUpdated": "2026-11-01T00:00:00.000Z"
            }]}
        })
        .to_string(),
    )
    .unwrap();
    fs::write(
        tmp.join("claude/settings.json"),
        serde_json::json!({
            "enabledPlugins": {"lore@lore": enabled},
            "extraKnownMarketplaces": {"lore": {"source": {"source": "github", "repo": "hsusul/lore"}}}
        })
        .to_string(),
    )
    .unwrap();
    install_plugin("codex", &tmp.join("codex/plugins/cache/lore/lore/0.1.0"));
    fs::write(
        tmp.join("codex/config.toml"),
        format!("model = \"o3\"\n\n[plugins.\"lore@lore\"]\nenabled = {enabled}\n"),
    )
    .unwrap();
}

#[test]
fn an_enabled_lore_plugin_connects_every_piece_without_a_fix() {
    let tmp = tempfile::tempdir().unwrap();
    install_both_plugins(tmp.path(), true);
    assert_eq!(
        connect(tmp.path(), |_| {}),
        format!(
            "Claude Code  <tmp>/claude\n{all}Codex  <tmp>/codex\n{all}",
            all = rows("connected (plugin lore)")
        )
    );
}

#[test]
fn a_disabled_lore_plugin_connects_nothing() {
    let tmp = tempfile::tempdir().unwrap();
    install_both_plugins(tmp.path(), false);
    let out = connect(tmp.path(), |_| {});
    assert!(!out.contains("plugin"), "{out}");
    assert_eq!(out.matches("not connected").count(), 8, "{out}");
}

#[test]
fn empty_env_config_roots_fall_back_to_home_directories() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");
    fs::create_dir_all(home.join(".claude")).unwrap();
    fs::create_dir_all(home.join(".codex")).unwrap();
    fs::write(home.join(".claude.json"), "{}").unwrap();
    let out = connect(tmp.path(), |command| {
        command.env("CLAUDE_CONFIG_DIR", "").env("CODEX_HOME", "");
    });
    assert!(
        out.contains("Claude Code  <tmp>/home/.claude\n"),
        "did not fall back to home/.claude:\n{out}"
    );
    assert!(
        out.contains("Codex  <tmp>/home/.codex\n"),
        "did not fall back to home/.codex:\n{out}"
    );
    assert!(
        !out.contains("agent not found"),
        "reported agents missing when installed in home:\n{out}"
    );
}
