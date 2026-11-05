//! What keeps a replay from seeing the future or touching the world.
//!
//! Each rule here exists because a replay without it failed:
//! - The agent read the real repository and found the fix already landed, so
//!   the sandbox hides `/Users` and `/Volumes` and the file tools are denied
//!   there too.
//! - Claude Code wrote the forked session despite `--no-session-persistence`
//!   and Lore ingested it, so the project folder is a symlink (see
//!   [`project_slug`]).
//! - The agent tried to open pull requests, so `gh` is a stub that refuses and
//!   logs what it was asked.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::{json, Value};

use crate::error::{Error, Result};
use crate::types::Action;

/// The folder name Claude Code gives a project: every character of the path
/// that is not alphanumeric becomes `-`.
///
/// The replay's folder under `projects/` is a symlink to the run directory.
/// Lore's discovery never descends a symlinked directory and its watcher sees
/// only resolved paths, so nothing written there reaches the archive.
#[must_use]
pub fn project_slug(path: &Path) -> String {
    path.to_string_lossy()
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '-' })
        .collect()
}

/// The `--settings` JSON: an OS sandbox with no network and no reads under
/// the folders that hold people's files, and the same denial for the file
/// tools, which the sandbox does not cover.
#[must_use]
pub fn settings() -> Value {
    let deny: Vec<String> = ["WebFetch", "WebSearch"]
        .into_iter()
        .map(str::to_owned)
        .chain(["Read", "Edit", "Write"].into_iter().flat_map(|tool| {
            ["Users", "Volumes"]
                .into_iter()
                .map(move |root| format!("{tool}(//{root}/**)"))
        }))
        .collect();
    json!({
        "sandbox": {
            "enabled": true,
            "allowUnsandboxedCommands": false,
            "failIfUnavailable": true,
            "filesystem": {"denyRead": ["/Users", "/Volumes"]},
            "network": {"allowedDomains": []},
        },
        "permissions": {
            "defaultMode": "acceptEdits",
            "deny": deny,
        },
    })
}

/// Absolute paths under `/Users` or `/Volumes` that the corrected turn's tool
/// calls named outside the repository (`toplevel`), sorted. Claude Code's own
/// folders (`/.claude/`) are left out. The sandbox hides these paths, so a
/// case that needs them cannot replay faithfully; the report says so.
#[must_use]
pub fn foreign_paths(actions: &[Action], toplevel: Option<&Path>) -> Vec<String> {
    let mut found = BTreeSet::new();
    for action in actions {
        if let Action::Tool { input, .. } = action {
            each_string(input, &mut |s| {
                for path in absolute_paths(s) {
                    let inside = toplevel.is_some_and(|top| Path::new(path).starts_with(top));
                    if !inside && !path.contains("/.claude/") {
                        found.insert(path.to_owned());
                    }
                }
            });
        }
    }
    found.into_iter().collect()
}

fn each_string(value: &Value, f: &mut dyn FnMut(&str)) {
    match value {
        Value::String(s) => f(s),
        Value::Array(items) => items.iter().for_each(|v| each_string(v, f)),
        Value::Object(map) => map.values().for_each(|v| each_string(v, f)),
        _ => {}
    }
}

/// Each `/Users/...` or `/Volumes/...` path in `text` that starts a word, up
/// to the first quote, space, or shell operator.
fn absolute_paths(text: &str) -> impl Iterator<Item = &str> {
    const ROOTS: [&str; 2] = ["/Users/", "/Volumes/"];
    let is_path_char = |c: char| c.is_alphanumeric() || matches!(c, '/' | '.' | '_' | '-' | '~');
    let ends = |c: char| c.is_whitespace() || matches!(c, '"' | '\'' | ';' | '&' | '|' | '`');
    text.char_indices().filter_map(move |(i, _)| {
        let rest = &text[i..];
        let starts_word = text[..i]
            .chars()
            .next_back()
            .is_none_or(|c| !is_path_char(c));
        if !starts_word || !ROOTS.iter().any(|root| rest.starts_with(root)) {
            return None;
        }
        let len = rest.find(ends).unwrap_or(rest.len());
        Some(&rest[..len])
    })
}

/// The variable that tells the `gh` stub where to log. The stub reads it at
/// call time, so the stub itself is the same file in every run of a case.
pub const OUTWARD_LOG_ENV: &str = "LORE_REPLAY_OUTWARD_LOG";

/// The environment additions for the agent and the shell it runs.
#[derive(Debug, Clone)]
pub struct ToolEnv {
    pub vars: Vec<(String, std::ffi::OsString)>,
    /// Where the `gh` stub logs each call.
    pub outward_log: PathBuf,
}

/// Build `home`: a git config with the person's name and email (the real one
/// sits under the hidden home folder), a private XDG config folder, and a
/// `gh` stub first on `PATH` that logs to `outward_log`.
pub fn tool_env(home: &Path, outward_log: &Path) -> Result<ToolEnv> {
    let bin = home.join("bin");
    let xdg = home.join("xdg");
    for dir in [&bin, &xdg] {
        std::fs::create_dir_all(dir).map_err(|e| Error::io("create", dir, e))?;
    }
    let gitconfig = home.join("gitconfig");
    let config = format!(
        "[user]\n\tname = {}\n\temail = {}\n[init]\n\tdefaultBranch = main\n",
        git_config_value(&global_git("user.name")),
        git_config_value(&global_git("user.email")),
    );
    std::fs::write(&gitconfig, config).map_err(|e| Error::io("write", &gitconfig, e))?;

    let gh = bin.join("gh");
    let stub = format!(
        "#!/bin/sh\nprintf 'gh %s\\n' \"$*\" >> \"${OUTWARD_LOG_ENV}\"\necho 'gh: no network in this environment' >&2\nexit 1\n"
    );
    std::fs::write(&gh, stub).map_err(|e| Error::io("write", &gh, e))?;
    make_executable(&gh)?;

    let mut path = std::ffi::OsString::from(bin.as_os_str());
    if let Some(old) = std::env::var_os("PATH") {
        path.push(":");
        path.push(old);
    }
    Ok(ToolEnv {
        vars: vec![
            ("GIT_CONFIG_GLOBAL".into(), gitconfig.into_os_string()),
            ("XDG_CONFIG_HOME".into(), xdg.into_os_string()),
            ("PATH".into(), path),
            (OUTWARD_LOG_ENV.into(), outward_log.as_os_str().to_owned()),
        ],
        outward_log: outward_log.to_owned(),
    })
}

#[cfg(unix)]
fn make_executable(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755))
        .map_err(|e| Error::io("make executable", path, e))
}

#[cfg(not(unix))]
fn make_executable(_path: &Path) -> Result<()> {
    Ok(())
}

fn global_git(key: &str) -> String {
    Command::new("git")
        .args(["config", "--global", key])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned())
        .unwrap_or_default()
}

/// A git config value, quoted so a name with `"` or `\` stays one value.
fn git_config_value(value: &str) -> String {
    let escaped = value.replace('\\', "\\\\").replace('"', "\\\"");
    format!("\"{escaped}\"")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slug_replaces_every_non_alphanumeric_character() {
        assert_eq!(
            project_slug(Path::new("/private/tmp/run_1/wd.x")),
            "-private-tmp-run-1-wd-x"
        );
        assert_eq!(
            project_slug(Path::new("/Volumes/990 Pro/a")),
            "-Volumes-990-Pro-a"
        );
    }

    #[test]
    fn settings_deny_the_network_and_the_home_folders() {
        let s = settings();
        assert_eq!(s["sandbox"]["enabled"], true);
        assert_eq!(s["sandbox"]["allowUnsandboxedCommands"], false);
        assert_eq!(s["sandbox"]["failIfUnavailable"], true);
        assert_eq!(
            s["sandbox"]["filesystem"]["denyRead"],
            json!(["/Users", "/Volumes"])
        );
        assert_eq!(s["sandbox"]["network"]["allowedDomains"], json!([]));
        assert_eq!(s["permissions"]["defaultMode"], "acceptEdits");
        assert_eq!(
            s["permissions"]["deny"],
            json!([
                "WebFetch",
                "WebSearch",
                "Read(//Users/**)",
                "Read(//Volumes/**)",
                "Edit(//Users/**)",
                "Edit(//Volumes/**)",
                "Write(//Users/**)",
                "Write(//Volumes/**)"
            ])
        );
    }

    #[test]
    fn foreign_paths_are_outside_the_repo_and_not_claude_folders() {
        let actions = vec![
            Action::Text {
                text: "/Users/me/elsewhere/ignored-in-prose".into(),
            },
            Action::Tool {
                name: "Bash".into(),
                input: json!({"command":
                    "cat /Users/me/notes.md && ls '/Volumes/990 Pro' ; cd /Users/me/repo/src|x"}),
            },
            Action::Tool {
                name: "Read".into(),
                input: json!({"file_path": "/Users/me/.claude/CLAUDE.md",
                              "other": ["/Users/me/repo-two/a", "/opt/Users/x"]}),
            },
        ];
        let top = Path::new("/Users/me/repo");
        assert_eq!(
            foreign_paths(&actions, Some(top)),
            vec!["/Users/me/notes.md", "/Users/me/repo-two/a", "/Volumes/990"]
        );
        assert!(foreign_paths(&actions, None).contains(&"/Users/me/repo/src".to_owned()));
    }

    #[test]
    fn quoting_keeps_odd_values_whole() {
        assert_eq!(git_config_value("A \"B\" \\"), "\"A \\\"B\\\" \\\\\"");
    }
}
