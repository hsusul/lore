//! Find lessons: run the person's own Claude Code or Codex, headless, on the
//! lore-lessons skill, with Lore's MCP server, so their agent reads each mined
//! correction with the turns around it and files the readable ones as
//! proposals. Lore opens no connection itself; the agent does, signed in as
//! the person signed it in, and only when they press the button.
//!
//! A run reads only corrections no earlier finished run read: Lore mines the
//! archive first, pins the agent's MCP server to the new corrections with
//! `LORE_CANDIDATES_FILE`, and records them in the vault's considered ledger
//! once the agent finishes. With nothing new, no agent is started. A run that
//! fails or times out records nothing, so the next one reads them again.

use std::collections::BTreeSet;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use lore_core::paths::ARCHIVE_DB_FILENAME;
use lore_ipc::LessonAgentDto;
use lore_memory::considered::{self, CorrectionId};
use lore_memory::mine;
use lore_memory::vault::{self, Vault};
use tauri::State;

use super::auto_lessons::{self, Runner};
use crate::state::AppState;

/// The skill each plugin ships, so the app needs no plugin installed.
const CLAUDE_SKILL: &str =
    include_str!("../../../plugins/claude-code/skills/lore-lessons/SKILL.md");
const CODEX_SKILL: &str = include_str!("../../../plugins/codex/skills/lore-lessons/SKILL.md");

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

/// The scratch file that lists the corrections a run reads, one id a line.
const PINNED_FILE: &str = "find-lessons.candidates.txt";
/// The variable that pins Lore's MCP server to that file.
const PINNED_ENV: &str = "LORE_CANDIDATES_FILE";

/// What a run returns when no correction is new, without starting an agent.
pub(crate) const NOTHING_NEW: &str = "No new corrections since the last Find lessons run.";

/// The agents that can write lessons.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Agent {
    Claude,
    Codex,
}

impl Agent {
    const ALL: [Agent; 2] = [Agent::Claude, Agent::Codex];

    pub(crate) fn parse(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|a| a.id() == id)
    }

    /// As `LORE_AGENT` and the archive name it.
    pub(crate) fn id(self) -> &'static str {
        match self {
            Agent::Claude => "claude-code",
            Agent::Codex => "codex",
        }
    }

    pub(crate) fn label(self) -> &'static str {
        match self {
            Agent::Claude => "Claude Code",
            Agent::Codex => "Codex",
        }
    }

    fn binary(self) -> &'static str {
        match self {
            Agent::Claude => "claude",
            Agent::Codex => "codex",
        }
    }

    /// What signs the agent in, for the person to run in a terminal.
    fn sign_in(self) -> &'static str {
        match self {
            Agent::Claude => "claude auth login",
            Agent::Codex => "codex login",
        }
    }

    /// The command that reports whether the agent is signed in.
    fn status_args(self) -> &'static [&'static str] {
        match self {
            Agent::Claude => &["auth", "status"],
            Agent::Codex => &["login", "status"],
        }
    }

    /// Whether the status command's output says the agent is signed in.
    fn signed_in(self, success: bool, stdout: &str) -> bool {
        match self {
            Agent::Claude => serde_json::from_str::<serde_json::Value>(stdout.trim())
                .is_ok_and(|v| v["loggedIn"].as_bool() == Some(true)),
            Agent::Codex => success,
        }
    }

    /// What the run is asked to do. It goes on stdin: the skill opens with
    /// `---`, which `claude` would read as an option if it were an argument.
    fn prompt(self) -> String {
        let skill = match self {
            Agent::Claude => CLAUDE_SKILL,
            Agent::Codex => CODEX_SKILL,
        };
        format!("{skill}{FROM_THE_APP}")
    }

    /// The arguments for a run that only writes text: no tools and no MCP
    /// servers. Codex writes its last message to `last`.
    pub(crate) fn text_only_arguments(self, last: &Path) -> Vec<String> {
        let owned = |list: &[&str]| list.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>();
        match self {
            Agent::Claude => owned(&[
                "-p",
                "--strict-mcp-config",
                "--tools",
                "",
                "--output-format",
                "json",
            ]),
            Agent::Codex => {
                let mut args = owned(&[
                    "exec",
                    "--skip-git-repo-check",
                    "--ephemeral",
                    "-s",
                    "read-only",
                    "-o",
                ]);
                args.push(last.display().to_string());
                args.push("-".to_owned());
                args
            }
        }
    }

    /// The arguments for one headless run with only Lore's MCP server, given
    /// a scratch folder for the files it needs.
    fn arguments(self, lorectl: &Path, archive: &Path, scratch: &Path) -> Vec<String> {
        let owned = |list: &[&str]| list.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>();
        match self {
            Agent::Claude => {
                let mut args = owned(&["-p", "--mcp-config"]);
                args.push(scratch.join("find-lessons.mcp.json").display().to_string());
                args.extend(owned(&["--strict-mcp-config", "--allowedTools"]));
                args.extend(owned(&TOOLS));
                args.extend(owned(&["--output-format", "json"]));
                args
            }
            Agent::Codex => {
                // TOML values: a JSON string is a valid TOML basic string.
                let quoted =
                    |p: &Path| serde_json::Value::from(p.display().to_string()).to_string();
                let mut args = owned(&[
                    "exec",
                    "--skip-git-repo-check",
                    "--ephemeral",
                    "-s",
                    "read-only",
                ]);
                for config in [
                    "approval_policy=\"never\"".to_owned(),
                    "mcp_servers.lore.default_tools_approval_mode=\"approve\"".to_owned(),
                    format!("mcp_servers.lore.command={}", quoted(lorectl)),
                    format!(
                        "mcp_servers.lore.args=[\"mcp\",\"--archive\",{}]",
                        quoted(archive)
                    ),
                    format!(
                        "mcp_servers.lore.env={{LORE_AGENT=\"codex\",{PINNED_ENV}={}}}",
                        quoted(&scratch.join(PINNED_FILE))
                    ),
                ] {
                    args.push("-c".to_owned());
                    args.push(config);
                }
                args.push("-o".to_owned());
                args.push(scratch.join("find-lessons.last.txt").display().to_string());
                args.push("-".to_owned());
                args
            }
        }
    }
}

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

/// The `lorectl` bundled next to the app's own binary (`Contents/MacOS`),
/// else one installed in `dirs`.
fn lorectl_in(own: Option<&Path>, dirs: &[PathBuf]) -> Option<PathBuf> {
    own.and_then(|d| find("lorectl", &[d.to_owned()]))
        .or_else(|| find("lorectl", dirs))
}

/// The MCP config that gives a Claude Code run Lore's tools over `archive`,
/// pinned to the corrections listed in `pinned`.
fn mcp_config(lorectl: &Path, archive: &Path, pinned: &Path) -> serde_json::Value {
    serde_json::json!({ "mcpServers": { "lore": {
        "command": lorectl,
        "args": ["mcp", "--archive", archive],
        "env": { "LORE_AGENT": "claude-code", PINNED_ENV: pinned },
    }}})
}

/// The person's vault, if they have one.
pub(crate) fn open_vault() -> Option<Vault> {
    vault::resolve_dir(None)
        .ok()
        .and_then(|dir| Vault::open(&dir).ok())
}

/// The corrections a run at `now_ms` would read: those in the default
/// `mine_candidates` listing, and the person's ordinary replies of the last
/// [`mine::REPLY_WINDOW_MS`], that no finished run has read, per `vault`'s
/// ledger.
pub(crate) fn pending(
    archive: &Path,
    vault: Option<&Vault>,
    now_ms: i64,
) -> Result<BTreeSet<CorrectionId>, String> {
    let conn = lore_core::storage::open_read_only(&archive.join(ARCHIVE_DB_FILENAME))
        .map_err(|e| e.to_string())?;
    pending_in(&conn, vault, now_ms)
}

/// [`pending`] over an open archive.
fn pending_in(
    conn: &rusqlite::Connection,
    vault: Option<&Vault>,
    now_ms: i64,
) -> Result<BTreeSet<CorrectionId>, String> {
    let turns = mine::read_turns(conn).map_err(|e| e.to_string())?;
    let seen = vault
        .map(considered::considered)
        .transpose()
        .map_err(|e| e.to_string())?
        .unwrap_or_default();
    let since = now_ms.saturating_sub(mine::REPLY_WINDOW_MS);
    Ok(considered::unconsidered(&turns, &seen, since))
}

/// Run `start` on the `pending` corrections: list them in `pinned` for the
/// agent's MCP server, and once `start` succeeds record them in `vault`'s
/// ledger as read by `run_id`. With none pending, `start` is never called.
pub(crate) fn run_pinned(
    pending: &BTreeSet<CorrectionId>,
    pinned: &Path,
    vault: Option<&Vault>,
    run_id: &str,
    start: impl FnOnce() -> Result<String, String>,
) -> Result<String, String> {
    if pending.is_empty() {
        return Ok(NOTHING_NEW.to_owned());
    }
    let listed: String = pending.iter().map(|id| format!("{id}\n")).collect();
    std::fs::write(pinned, listed).map_err(|e| e.to_string())?;
    let report = start()?;
    if let Some(vault) = vault {
        considered::record(vault, pending, run_id).map_err(|e| e.to_string())?;
    }
    let n = pending.len();
    let plural = if n == 1 { "" } else { "s" };
    Ok(format!("Read {n} new correction{plural}.\n\n{report}"))
}

/// Why a run left no report, from the end of what it printed.
fn no_report(label: &str, said: &str) -> String {
    let said = said.trim();
    let tail: String = said
        .chars()
        .skip(said.chars().count().saturating_sub(300))
        .collect();
    if tail.is_empty() {
        format!("{label} did not finish its report")
    } else {
        format!("{label} did not finish its report: {tail}")
    }
}

/// The agent's closing report from `claude -p --output-format json`, or why
/// there is none, from what it printed to stderr.
fn report(stdout: &str, stderr: &str) -> Result<String, String> {
    let value: serde_json::Value =
        serde_json::from_str(stdout.trim()).map_err(|_| no_report("Claude Code", stderr))?;
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

/// Where the agents and lorectl are looked for, and the PATH a run gets.
struct Found {
    home: PathBuf,
    dirs: Vec<PathBuf>,
}

fn found() -> Result<Found, String> {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .ok_or("HOME is not set")?;
    let path = std::env::var("PATH").ok();
    Ok(Found {
        dirs: search_dirs(&home, path.as_deref()),
        home,
    })
}

/// Where `agent` is installed in `dirs`, and whether its own status command
/// says it is signed in.
fn status(agent: Agent, dirs: &[PathBuf], path: &std::ffi::OsStr) -> (Option<PathBuf>, bool) {
    let binary = find(agent.binary(), dirs);
    let signed_in = binary.as_ref().is_some_and(|bin| {
        Command::new(bin)
            .args(agent.status_args())
            .env("PATH", path)
            .stdin(Stdio::null())
            .output()
            .is_ok_and(|o| agent.signed_in(o.status.success(), &String::from_utf8_lossy(&o.stdout)))
    });
    (binary, signed_in)
}

/// Whether `agent` is installed and signed in, so it can write lessons now.
/// Blocks while the agent's status command runs.
pub(crate) fn ready(agent: Agent) -> bool {
    found().is_ok_and(|found| {
        std::env::join_paths(&found.dirs).is_ok_and(|path| status(agent, &found.dirs, &path).1)
    })
}

/// The agent an automatic run uses: the one the person last ran Find lessons
/// with, else Claude Code, else Codex; each only when `ready`.
pub(crate) fn pick(last: Option<Agent>, ready: impl Fn(Agent) -> bool) -> Option<Agent> {
    last.into_iter()
        .chain(Agent::ALL)
        .find(|&agent| ready(agent))
}

/// Which agents can write lessons: installed, and signed in. Checked locally
/// by each agent's own status command.
#[tauri::command]
pub async fn lesson_agents() -> Result<Vec<LessonAgentDto>, String> {
    let found = found()?;
    tauri::async_runtime::spawn_blocking(move || {
        let path = std::env::join_paths(&found.dirs).map_err(|e| e.to_string())?;
        Ok(Agent::ALL
            .into_iter()
            .map(|agent| {
                let (binary, signed_in) = status(agent, &found.dirs, &path);
                LessonAgentDto {
                    id: agent.id().to_owned(),
                    label: agent.label().to_owned(),
                    installed: binary.is_some(),
                    signed_in,
                    sign_in: agent.sign_in().to_owned(),
                }
            })
            .collect())
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Everything one run of an agent on the lore-lessons skill needs, worked out
/// before it starts.
pub(crate) struct Prepared {
    agent: Agent,
    binary: PathBuf,
    home: PathBuf,
    path: std::ffi::OsString,
    args: Vec<String>,
    prompt: String,
    last: PathBuf,
    pinned: PathBuf,
}

impl Prepared {
    /// A run of `binary` as `agent`, with Lore's tools from `lorectl` over
    /// `archive` and its files in `scratch`. `model` is passed to Claude Code
    /// as `--model`; `None` keeps the person's default.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        agent: Agent,
        binary: PathBuf,
        home: PathBuf,
        path: std::ffi::OsString,
        lorectl: &Path,
        archive: &Path,
        scratch: &Path,
        model: Option<&str>,
    ) -> Result<Self, String> {
        let pinned = scratch.join(PINNED_FILE);
        let mut args = agent.arguments(lorectl, archive, scratch);
        if agent == Agent::Claude {
            std::fs::write(
                scratch.join("find-lessons.mcp.json"),
                mcp_config(lorectl, archive, &pinned).to_string(),
            )
            .map_err(|e| e.to_string())?;
            if let Some(model) = model {
                args.extend(["--model".to_owned(), model.to_owned()]);
            }
        }
        Ok(Self {
            agent,
            binary,
            home,
            path,
            args,
            prompt: agent.prompt(),
            last: scratch.join("find-lessons.last.txt"),
            pinned,
        })
    }

    /// A run of the installed `agent` over the archive in `archive_dir`, which
    /// also holds its scratch files.
    pub(crate) fn locate(
        agent: Agent,
        archive_dir: &Path,
        model: Option<&str>,
    ) -> Result<Self, String> {
        let (binary, home, path) = locate(agent)?;
        let Found { dirs, .. } = found()?;
        let label = agent.label();
        let own = std::env::current_exe()
            .ok()
            .and_then(|e| e.parent().map(Path::to_path_buf));
        let lorectl = lorectl_in(own.as_deref(), &dirs)
            .ok_or_else(|| format!("lorectl is missing, so {label} cannot reach Lore's tools."))?;
        Self::new(
            agent,
            binary,
            home,
            path,
            &lorectl,
            archive_dir,
            archive_dir,
            model,
        )
    }

    /// Run the agent pinned to `pending`, recording them in `vault`'s ledger
    /// once it finishes; see [`run_pinned`]. Blocks.
    pub(crate) fn run_on(
        &self,
        pending: &BTreeSet<CorrectionId>,
        vault: Option<&Vault>,
        run_id: &str,
    ) -> Result<String, String> {
        run_pinned(pending, &self.pinned, vault, run_id, || {
            run(
                self.agent,
                &self.binary,
                &self.args,
                &self.prompt,
                &self.home,
                &self.path,
                &self.last,
            )
        })
    }
}

/// The ledger's name for a run of `agent` started at `now_ms`.
pub(crate) fn run_id(agent: Agent, now_ms: i64) -> String {
    format!("find-lessons-{}-{now_ms}", agent.id())
}

/// What the button says when a run, manual or automatic, is already going.
pub(crate) const BUSY: &str = "Lore is already finding lessons. Try again when it finishes.";

/// Run the person's `agent` on the lore-lessons skill and return its report.
/// Proposals it files land in the vault's inbox for review.
#[tauri::command]
pub async fn find_lessons(state: State<'_, AppState>, agent: String) -> Result<String, String> {
    let agent = Agent::parse(&agent).ok_or("unknown agent")?;
    let guard = state.lessons.try_take(Runner::Manual).ok_or(BUSY)?;
    let prepared = Prepared::locate(agent, &state.archive_dir, None)?;
    // The automatic run uses the agent the person last chose here.
    if let Err(e) = auto_lessons::remember_agent(&state.db, agent) {
        eprintln!("warning: could not remember the Find lessons agent: {e}");
    }
    let archive = state.archive_dir.clone();
    let run_id = run_id(agent, lore_core::now_ms());
    tauri::async_runtime::spawn_blocking(move || {
        let _guard = guard;
        // No vault means no ledger: every correction is new, and none is
        // recorded, as the agent cannot file a proposal without one either.
        let vault = open_vault();
        let pending = pending(&archive, vault.as_ref(), lore_core::now_ms())?;
        prepared.run_on(&pending, vault.as_ref(), &run_id)
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Run `binary` headless with `args`, `prompt` on stdin, and return the
/// agent's closing message: Claude Code's JSON `result`, or the file Codex
/// writes it to (`last`). Blocks; stopped after [`LIMIT`].
pub(crate) fn run(
    agent: Agent,
    binary: &Path,
    args: &[String],
    prompt: &str,
    home: &Path,
    path: &std::ffi::OsStr,
    last: &Path,
) -> Result<String, String> {
    let label = agent.label();
    let _ = std::fs::remove_file(last);
    let mut child = Command::new(binary)
        .args(args)
        .current_dir(home)
        .env("PATH", path)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("could not start {label}: {e}"))?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin
            .write_all(prompt.as_bytes())
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
            return Err(format!(
                "{label} took more than 20 minutes and was stopped."
            ));
        }
        std::thread::sleep(Duration::from_millis(500));
    }
    let stdout = stdout.join().unwrap_or_default();
    let stderr = stderr.join().unwrap_or_default();
    match agent {
        Agent::Claude => report(&stdout, &stderr),
        Agent::Codex => std::fs::read_to_string(last)
            .ok()
            .map(|text| text.trim().to_owned())
            .filter(|text| !text.is_empty())
            .ok_or_else(|| no_report(label, &format!("{stdout}\n{stderr}"))),
    }
}

/// What a run needs to start `agent`: the binary, home, and PATH.
pub(crate) fn locate(agent: Agent) -> Result<(PathBuf, PathBuf, std::ffi::OsString), String> {
    let Found { home, dirs } = found()?;
    let label = agent.label();
    let binary = find(agent.binary(), &dirs)
        .ok_or_else(|| format!("{label} is not installed. Install it, sign in, then try again."))?;
    let path = std::env::join_paths(&dirs).map_err(|e| e.to_string())?;
    Ok((binary, home, path))
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
    fn prefers_the_lorectl_bundled_with_the_app() {
        let bundle = tempfile::tempdir().unwrap();
        let installed = tempfile::tempdir().unwrap();
        std::fs::write(installed.path().join("lorectl"), "").unwrap();
        let dirs = [installed.path().to_owned()];
        assert_eq!(
            lorectl_in(Some(bundle.path()), &dirs),
            Some(installed.path().join("lorectl"))
        );
        std::fs::write(bundle.path().join("lorectl"), "").unwrap();
        assert_eq!(
            lorectl_in(Some(bundle.path()), &dirs),
            Some(bundle.path().join("lorectl"))
        );
        assert_eq!(lorectl_in(None, &[]), None);
    }

    #[test]
    fn runs_the_skill_with_only_lores_tools() {
        for agent in Agent::ALL {
            let prompt = agent.prompt();
            assert!(
                prompt.starts_with("---\nname: lore-lessons"),
                "{}",
                &prompt[..40]
            );
            assert!(prompt.ends_with("End with the report."));
        }
        let (lorectl, archive, scratch) =
            (Path::new("/b/lorectl"), Path::new("/a"), Path::new("/s"));
        assert_eq!(
            Agent::Claude.arguments(lorectl, archive, scratch),
            [
                "-p",
                "--mcp-config",
                "/s/find-lessons.mcp.json",
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
            Agent::Codex.arguments(lorectl, archive, scratch),
            [
                "exec",
                "--skip-git-repo-check",
                "--ephemeral",
                "-s",
                "read-only",
                "-c",
                "approval_policy=\"never\"",
                "-c",
                "mcp_servers.lore.default_tools_approval_mode=\"approve\"",
                "-c",
                "mcp_servers.lore.command=\"/b/lorectl\"",
                "-c",
                "mcp_servers.lore.args=[\"mcp\",\"--archive\",\"/a\"]",
                "-c",
                "mcp_servers.lore.env={LORE_AGENT=\"codex\",LORE_CANDIDATES_FILE=\"/s/find-lessons.candidates.txt\"}",
                "-o",
                "/s/find-lessons.last.txt",
                "-",
            ]
        );
        assert_eq!(
            mcp_config(
                Path::new("/bin/lorectl"),
                Path::new("/a"),
                Path::new("/s/p.txt")
            ),
            serde_json::json!({ "mcpServers": { "lore": {
                "command": "/bin/lorectl", "args": ["mcp", "--archive", "/a"],
                "env": { "LORE_AGENT": "claude-code", "LORE_CANDIDATES_FILE": "/s/p.txt" },
            }}})
        );
    }

    /// A fake `claude` that prints `stdout` and exits with `code`.
    fn fake_claude(dir: &Path, stdout: &str, code: i32) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let bin = dir.join("claude");
        std::fs::write(
            &bin,
            format!("#!/bin/sh\ncat >/dev/null\nprintf '%s' '{stdout}'\nexit {code}\n"),
        )
        .unwrap();
        std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
        bin
    }

    fn ids(texts: &[&str]) -> BTreeSet<CorrectionId> {
        texts
            .iter()
            .map(|t| CorrectionId::parse(t).unwrap())
            .collect()
    }

    #[test]
    fn with_nothing_new_no_agent_starts() {
        let dir = tempfile::tempdir().unwrap();
        let (vault, _) = Vault::init(&dir.path().join("vault")).unwrap();
        let pinned = dir.path().join(PINNED_FILE);
        let result = run_pinned(&BTreeSet::new(), &pinned, Some(&vault), "r", || {
            panic!("the agent was started")
        });
        assert_eq!(result, Ok(NOTHING_NEW.to_owned()));
        assert!(!pinned.exists());
        assert!(considered::considered(&vault).unwrap().is_empty());
    }

    /// 2026-08-01T10:00:00Z, when the test archive's first session starts.
    const AUG_1: i64 = 1_785_578_400_000;

    #[test]
    fn a_run_reads_new_corrections_and_the_windows_replies_once() {
        let dir = tempfile::tempdir().unwrap();
        let conn = crate::commands::memory::tests::archive(
            dir.path(),
            &[&[
                ("user", "add axios"),
                ("assistant", "Running npm install axios."),
                ("user", "no, use pnpm not npm"),
                ("assistant", "Switched to pnpm."),
                ("user", "put the changelog entry on top"),
            ]],
        );
        let (vault, _) = Vault::init(&dir.path().join("vault")).unwrap();
        let turns = mine::read_turns(&conn).unwrap();
        let replies = mine::replies_in(&turns, &mine::Limits::default(), i64::MIN);
        assert_eq!(replies.len(), 1);
        let reply = CorrectionId::of(&replies[0]);

        // An hour later the run reads the correction and the reply.
        let pending = pending_in(&conn, Some(&vault), AUG_1 + 3_600_000).unwrap();
        assert_eq!(pending.len(), 2, "{pending:?}");
        assert!(pending.contains(&reply));
        // Three days later the reply has left the window.
        let later = pending_in(&conn, Some(&vault), AUG_1 + 3 * 86_400_000).unwrap();
        assert_eq!(later.len(), 1);
        assert!(!later.contains(&reply));

        // Once a run read both, nothing is new and no agent starts.
        considered::record(&vault, &pending, "run-1").unwrap();
        let none = pending_in(&conn, Some(&vault), AUG_1 + 3_600_000).unwrap();
        assert!(none.is_empty());
        let pinned = dir.path().join(PINNED_FILE);
        let result = run_pinned(&none, &pinned, Some(&vault), "run-2", || {
            panic!("the agent was started")
        });
        assert_eq!(result, Ok(NOTHING_NEW.to_owned()));
    }

    #[test]
    fn a_finished_run_records_the_corrections_it_was_pinned_to() {
        let dir = tempfile::tempdir().unwrap();
        let (vault, _) = Vault::init(&dir.path().join("vault")).unwrap();
        let pinned = dir.path().join(PINNED_FILE);
        let pending = ids(&["cr-0123456789abcdef", "cr-aaaaaaaaaaaaaaaa"]);
        let bin = fake_claude(
            dir.path(),
            r#"{"result":"Filed: 1 rule","is_error":false}"#,
            0,
        );
        let last = dir.path().join("last.txt");
        let result = run_pinned(&pending, &pinned, Some(&vault), "run-7", || {
            // The agent's MCP server reads exactly these while it runs.
            assert_eq!(considered::read_ids(&pinned).unwrap(), pending);
            run(
                Agent::Claude,
                &bin,
                &[],
                "skill",
                dir.path(),
                std::ffi::OsStr::new("/usr/bin:/bin"),
                &last,
            )
        });
        assert_eq!(
            result,
            Ok("Read 2 new corrections.\n\nFiled: 1 rule".to_owned())
        );
        assert_eq!(considered::considered(&vault).unwrap(), pending);
        let ledger =
            std::fs::read_to_string(dir.path().join("vault/.lore/considered.jsonl")).unwrap();
        assert!(
            ledger.lines().all(|l| l.contains(r#""run":"run-7""#)),
            "{ledger}"
        );
    }

    #[test]
    fn a_failed_run_records_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let (vault, _) = Vault::init(&dir.path().join("vault")).unwrap();
        let pinned = dir.path().join(PINNED_FILE);
        let pending = ids(&["cr-0123456789abcdef"]);
        let last = dir.path().join("last.txt");
        for (stdout, code) in [
            (r#"{"result":"Not logged in","is_error":true}"#, 1),
            ("", 1),
        ] {
            let bin = fake_claude(dir.path(), stdout, code);
            let result = run_pinned(&pending, &pinned, Some(&vault), "r", || {
                run(
                    Agent::Claude,
                    &bin,
                    &[],
                    "skill",
                    dir.path(),
                    std::ffi::OsStr::new("/usr/bin:/bin"),
                    &last,
                )
            });
            assert!(result.is_err(), "{result:?}");
        }
        assert!(considered::considered(&vault).unwrap().is_empty());
    }

    #[test]
    fn a_text_only_run_gets_no_tools() {
        assert_eq!(
            Agent::Claude.text_only_arguments(Path::new("/s/last.txt")),
            [
                "-p",
                "--strict-mcp-config",
                "--tools",
                "",
                "--output-format",
                "json"
            ]
        );
        assert_eq!(
            Agent::Codex.text_only_arguments(Path::new("/s/last.txt")),
            [
                "exec",
                "--skip-git-repo-check",
                "--ephemeral",
                "-s",
                "read-only",
                "-o",
                "/s/last.txt",
                "-"
            ]
        );
    }

    #[test]
    fn tells_whether_each_agent_is_signed_in() {
        assert!(Agent::Claude.signed_in(true, r#"{"loggedIn": true, "authMethod": "claude.ai"}"#));
        assert!(!Agent::Claude.signed_in(true, r#"{"loggedIn": false}"#));
        assert!(!Agent::Claude.signed_in(true, "not json"));
        assert!(Agent::Codex.signed_in(true, "Logged in using ChatGPT"));
        assert!(!Agent::Codex.signed_in(false, "Not logged in"));
        assert_eq!(Agent::parse("codex"), Some(Agent::Codex));
        assert_eq!(Agent::parse("cursor"), None);
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
