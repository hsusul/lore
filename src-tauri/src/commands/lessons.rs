//! Find lessons: run the person's own Claude Code, headless, on the
//! lore-lessons skill, with Lore's MCP server, so their agent reads each mined
//! correction with the turns around it and files the readable ones as
//! proposals. Lore opens no connection itself; Claude Code does, as the person
//! signed it in, and only when they press the button.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use tauri::State;

use crate::state::AppState;

/// The skill both plugins ship, so the app needs no plugin installed.
const SKILL: &str = include_str!("../../../plugins/claude-code/skills/lore-lessons/SKILL.md");

/// What the headless run adds to the skill: it starts in no repository.
const FROM_THE_APP: &str = "\n\n## Running from the Lore app\n\n\
You were started by the Lore app, not in a repository, so `cwd_repo` is null. \
For a rule whose receipts all share one non-null `repo.scope`, file it with `scope` set to repo \
and `cwd` set to that receipt's `repo_path`. File every other rule as global. \
Do not ask questions; nobody is there to answer. End with the report.";

/// The Lore tools, as Claude Code names a server called `lore`.
const TOOLS: [&str; 3] = [
    "mcp__lore__recall_memory",
    "mcp__lore__mine_candidates",
    "mcp__lore__propose_memory",
];

/// A run reads every candidate and may take a while; past this it is stopped.
const LIMIT: Duration = Duration::from_secs(20 * 60);

/// Folders an app opened from Finder does not have on its PATH.
fn search_dirs(home: &Path, path: Option<&str>) -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = path
        .map(|p| std::env::split_paths(p).collect())
        .unwrap_or_default();
    for dir in [
        home.join(".local/bin"),
        home.join(".claude/local"),
        home.join(".cargo/bin"),
        PathBuf::from("/opt/homebrew/bin"),
        PathBuf::from("/usr/local/bin"),
    ] {
        if !dirs.contains(&dir) {
            dirs.push(dir);
        }
    }
    dirs
}

/// The first executable called `name` in `dirs`.
fn find(name: &str, dirs: &[PathBuf]) -> Option<PathBuf> {
    dirs.iter().map(|d| d.join(name)).find(|p| p.is_file())
}

/// The MCP config that gives the run Lore's tools over `archive`.
fn mcp_config(lorectl: &Path, archive: &Path) -> serde_json::Value {
    serde_json::json!({ "mcpServers": { "lore": {
        "command": lorectl,
        "args": ["mcp", "--archive", archive],
        "env": { "LORE_AGENT": "claude-code" },
    }}})
}

/// What the run is asked to do. It goes on stdin: the skill opens with `---`,
/// which `claude` would read as an option if it were an argument.
fn prompt() -> String {
    format!("{SKILL}{FROM_THE_APP}")
}

/// `claude`'s arguments for one headless run of the skill.
fn arguments(config: &Path) -> Vec<String> {
    let mut args = vec![
        "-p".to_owned(),
        "--mcp-config".to_owned(),
        config.display().to_string(),
        "--strict-mcp-config".to_owned(),
        "--allowedTools".to_owned(),
    ];
    args.extend(TOOLS.iter().map(|t| (*t).to_owned()));
    args.extend(["--output-format".to_owned(), "json".to_owned()]);
    args
}

/// The agent's closing report from `claude -p --output-format json`, or why
/// there is none, from what it printed to stderr.
fn report(stdout: &str, stderr: &str) -> Result<String, String> {
    let value: serde_json::Value = serde_json::from_str(stdout.trim()).map_err(|_| {
        let said: String = stderr.trim().chars().take(300).collect();
        if said.is_empty() {
            "Claude Code did not finish its report".to_owned()
        } else {
            format!("Claude Code did not finish its report: {said}")
        }
    })?;
    let text = value["result"]
        .as_str()
        .unwrap_or_default()
        .trim()
        .to_owned();
    if value["is_error"].as_bool().unwrap_or(false) {
        return Err(if text.is_empty() {
            "Claude Code stopped with an error".to_owned()
        } else {
            text
        });
    }
    Ok(text)
}

/// Read `pipe` to its end on its own thread.
fn drain(pipe: Option<impl Read + Send + 'static>) -> JoinHandle<String> {
    std::thread::spawn(move || {
        let mut text = String::new();
        if let Some(mut pipe) = pipe {
            let _ = pipe.read_to_string(&mut text);
        }
        text
    })
}

/// Run the person's Claude Code on the lore-lessons skill and return its
/// report. Proposals it files land in the vault's inbox for review.
#[tauri::command]
pub async fn find_lessons(state: State<'_, AppState>) -> Result<String, String> {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .ok_or("HOME is not set")?;
    let path = std::env::var("PATH").ok();
    let dirs = search_dirs(&home, path.as_deref());
    let claude = find("claude", &dirs)
        .ok_or("Claude Code is not installed. Install it, sign in, then try again.")?;
    let own = std::env::current_exe()
        .ok()
        .and_then(|e| e.parent().map(Path::to_path_buf));
    let lorectl = own
        .and_then(|d| find("lorectl", &[d]))
        .or_else(|| find("lorectl", &dirs))
        .ok_or("lorectl is not installed, so Claude Code cannot reach Lore's tools.")?;
    let config = state.archive_dir.join("find-lessons.mcp.json");
    std::fs::write(
        &config,
        mcp_config(&lorectl, &state.archive_dir).to_string(),
    )
    .map_err(|e| e.to_string())?;
    let joined = std::env::join_paths(&dirs).map_err(|e| e.to_string())?;

    tauri::async_runtime::spawn_blocking(move || {
        let mut child = Command::new(&claude)
            .args(arguments(&config))
            .current_dir(&home)
            .env("PATH", joined)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| format!("could not start Claude Code: {e}"))?;
        if let Some(mut stdin) = child.stdin.take() {
            stdin
                .write_all(prompt().as_bytes())
                .map_err(|e| e.to_string())?;
        }
        // Read both pipes while it runs, so a long report cannot fill one and stall it.
        let stdout = drain(child.stdout.take());
        let stderr = drain(child.stderr.take());
        let started = Instant::now();
        loop {
            if child.try_wait().map_err(|e| e.to_string())?.is_some() {
                break;
            }
            if started.elapsed() > LIMIT {
                let _ = child.kill();
                return Err("Claude Code took more than 20 minutes and was stopped.".to_owned());
            }
            std::thread::sleep(Duration::from_millis(500));
        }
        let stdout = stdout.join().unwrap_or_default();
        let stderr = stderr.join().unwrap_or_default();
        report(&stdout, &stderr)
    })
    .await
    .map_err(|e| e.to_string())?
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    #[test]
    fn looks_where_finder_apps_cannot_see() {
        let home = Path::new("/Users/me");
        let dirs = search_dirs(home, Some("/usr/bin:/opt/homebrew/bin"));
        assert_eq!(
            dirs,
            [
                "/usr/bin",
                "/opt/homebrew/bin",
                "/Users/me/.local/bin",
                "/Users/me/.claude/local",
                "/Users/me/.cargo/bin",
                "/usr/local/bin",
            ]
            .map(PathBuf::from)
        );
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("claude"), "").unwrap();
        assert_eq!(
            find(
                "claude",
                &[PathBuf::from("/nowhere"), dir.path().to_owned()]
            ),
            Some(dir.path().join("claude"))
        );
        assert_eq!(find("lorectl", &[dir.path().to_owned()]), None);
    }

    #[test]
    fn runs_the_skill_with_only_lores_tools() {
        let prompt = prompt();
        assert!(
            prompt.starts_with("---\nname: lore-lessons"),
            "{}",
            &prompt[..40]
        );
        assert!(prompt.ends_with("End with the report."));
        assert_eq!(
            arguments(Path::new("/tmp/c.json")),
            [
                "-p",
                "--mcp-config",
                "/tmp/c.json",
                "--strict-mcp-config",
                "--allowedTools",
                "mcp__lore__recall_memory",
                "mcp__lore__mine_candidates",
                "mcp__lore__propose_memory",
                "--output-format",
                "json",
            ]
        );
        assert_eq!(
            mcp_config(Path::new("/bin/lorectl"), Path::new("/a")),
            serde_json::json!({ "mcpServers": { "lore": {
                "command": "/bin/lorectl", "args": ["mcp", "--archive", "/a"],
                "env": { "LORE_AGENT": "claude-code" },
            }}})
        );
    }

    #[test]
    fn reads_the_agents_report_or_its_error() {
        assert_eq!(
            report(r#"{"result":" Filed: 3 rules ","is_error":false}"#, ""),
            Ok("Filed: 3 rules".to_owned())
        );
        assert_eq!(
            report(r#"{"result":"Not logged in","is_error":true}"#, ""),
            Err("Not logged in".to_owned())
        );
        assert_eq!(
            report("", ""),
            Err("Claude Code did not finish its report".to_owned())
        );
        assert_eq!(
            report("", "error: unknown option '---'\n"),
            Err("Claude Code did not finish its report: error: unknown option '---'".to_owned())
        );
    }
}
