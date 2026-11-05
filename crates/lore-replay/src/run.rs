//! One replay: a case, an arm, a trial.
//!
//! The run directory holds everything the replay made: the scratch clone
//! (`wd`), the real project folder Claude Code writes into
//! (`claude-project`), the stub tools (`home`), `outward.log`, and the record
//! (`run.json`). The only thing outside it is the project-folder symlink,
//! which is removed when the agent exits.
//!
//! The clone, the project folder, and the stubs are deleted when the run ends,
//! unless [`RunOptions::keep_scratch`] is set. Each clone is a full copy of the
//! repository, and a few hundred kept clones of one large repository filled a
//! disk.

use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Instant;

use serde_json::Value;

use crate::cases::{claude_config_dir, claude_projects_dir};
use crate::error::{Error, Result};
use crate::isolation::{self, project_slug};
use crate::prefix::{self, Rewrite};
use crate::scratch::{self, Source};
use crate::types::{Arm, Case, RunRecord};

/// Overrides the `claude` found on `PATH`.
pub const CLAUDE_ENV: &str = "LORE_REPLAY_CLAUDE";
/// How much of the agent's stderr the record keeps, in characters.
const STDERR_TAIL: usize = 2000;

#[derive(Debug, Clone)]
pub struct RunOptions {
    /// The root all run directories go under.
    pub out: PathBuf,
    pub trial: u32,
    pub model: String,
    pub max_turns: u32,
    /// Keep `wd`, `home`, and `claude-project` after the run, for inspection.
    pub keep_scratch: bool,
}

/// `<out>/<case id, : as _>/<arm>-<trial>`.
#[must_use]
pub fn run_dir(case: &Case, arm: &Arm, opts: &RunOptions) -> PathBuf {
    opts.out
        .join(case.id.replace(':', "_"))
        .join(format!("{}-{}", arm.name(), opts.trial))
}

/// Replay `case` under `arm`, write `run.json` in its run directory, and
/// return the record. A previous run in the same directory is replaced.
pub fn run(case: &Case, arm: &Arm, opts: &RunOptions) -> Result<RunRecord> {
    let dir = run_dir(case, arm, opts);
    match std::fs::remove_dir_all(&dir) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(Error::io("remove the old run", &dir, e)),
    }
    std::fs::create_dir_all(&dir).map_err(|e| Error::io("create", &dir, e))?;
    let dir = std::fs::canonicalize(&dir).map_err(|e| Error::io("resolve", &dir, e))?;
    let _scratch = Scratch {
        dir: dir.clone(),
        keep: opts.keep_scratch,
    };

    let source = scratch::source(case);
    let wd = dir.join("wd");
    if let Source::Commit { toplevel, commit } = &source {
        scratch::clone_at(toplevel, commit, case.branch.as_deref(), &wd)?;
    }
    std::fs::create_dir_all(&wd).map_err(|e| Error::io("create", &wd, e))?;

    let session_id = uuid::Uuid::new_v4().to_string();
    let project_real = dir.join("claude-project");
    std::fs::create_dir_all(&project_real).map_err(|e| Error::io("create", &project_real, e))?;
    let projects = claude_projects_dir().ok_or(Error::NoHome)?;
    let link = ProjectLink::create(&projects.join(project_slug(&wd)), &project_real)?;

    let session_bytes =
        std::fs::read(&case.file).map_err(|e| Error::io("read session file", &case.file, e))?;
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let prefix = prefix::build(
        &session_bytes,
        case.prompt_line,
        &Rewrite {
            session_id: &session_id,
            scratch: &wd,
            toplevel: source.toplevel(),
            home: home.as_deref(),
        },
    );
    let prefix_path = project_real.join(format!("{session_id}.jsonl"));
    std::fs::write(&prefix_path, &prefix.text).map_err(|e| Error::io("write", &prefix_path, e))?;

    let tools = isolation::tool_env(&dir)?;
    let mut args: Vec<String> = vec!["-p".into()];
    if prefix.has_messages {
        args.extend([
            "--resume".into(),
            session_id.clone(),
            "--fork-session".into(),
        ]);
    }
    args.extend([
        "--no-session-persistence".into(),
        "--model".into(),
        opts.model.clone(),
        "--output-format".into(),
        "stream-json".into(),
        "--verbose".into(),
        "--max-turns".into(),
        opts.max_turns.to_string(),
        "--setting-sources".into(),
        String::new(),
        "--strict-mcp-config".into(),
        "--settings".into(),
        isolation::settings().to_string(),
    ]);
    if let Some(extra) = appended_context(arm, &wd).filter(|t| !t.is_empty()) {
        args.extend(["--append-system-prompt".into(), extra]);
    }

    let program = std::env::var_os(CLAUDE_ENV).unwrap_or_else(|| "claude".into());
    let mut cmd = Command::new(&program);
    cmd.args(&args)
        .current_dir(&wd)
        .env("CLAUDE_CODE_DISABLE_CLAUDE_MDS", "1")
        .env("CLAUDE_CODE_DISABLE_AUTO_MEMORY", "1")
        .envs(tools.vars.iter().map(|(k, v)| (k, v)))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let started = Instant::now();
    let mut child = cmd.spawn().map_err(|source| Error::Spawn {
        program: program.to_string_lossy().into_owned(),
        source,
    })?;
    let stdin = child.stdin.take().map(|mut pipe| {
        let prompt = case.prompt.clone();
        std::thread::spawn(move || {
            // A closed pipe means the agent stopped reading; its exit says why.
            let _ = pipe.write_all(prompt.as_bytes());
        })
    });
    let stderr = child.stderr.take().map(|mut pipe| {
        std::thread::spawn(move || {
            let mut buf = Vec::new();
            let _ = pipe.read_to_end(&mut buf);
            String::from_utf8_lossy(&buf).into_owned()
        })
    });
    let mut events = Vec::new();
    if let Some(stdout) = child.stdout.take() {
        for line in BufReader::new(stdout)
            .split(b'\n')
            .map_while(std::io::Result::ok)
        {
            if let Ok(event) = serde_json::from_slice::<Value>(&line) {
                events.push(event);
            }
        }
    }
    let waited = child.wait();
    let seconds = (started.elapsed().as_secs_f64() * 10.0).round() / 10.0;
    if let Some(handle) = stdin {
        let _ = handle.join();
    }
    let stderr = stderr.and_then(|h| h.join().ok()).unwrap_or_default();
    drop(link);
    waited.map_err(|e| Error::io("wait for", Path::new(&program), e))?;

    let result = events
        .iter()
        .rev()
        .find(|e| e.get("type").and_then(Value::as_str) == Some("result"));
    let record = RunRecord {
        case: case.id.clone(),
        arm: arm.name(),
        trial: opts.trial,
        model: opts.model.clone(),
        fidelity: source.fidelity(),
        foreign_paths: isolation::foreign_paths(&case.actions, source.toplevel()),
        prefix_lines: case.prompt_line,
        seconds,
        cost_usd: result
            .and_then(|r| r.get("total_cost_usd"))
            .and_then(Value::as_f64),
        is_error: result
            .and_then(|r| r.get("is_error"))
            .and_then(Value::as_bool),
        stderr: tail(&stderr, STDERR_TAIL),
        outward: std::fs::read_to_string(&tools.outward_log)
            .map(|log| log.lines().map(str::to_owned).collect())
            .unwrap_or_default(),
        events,
    };
    let path = dir.join("run.json");
    let json = serde_json::to_string(&record).map_err(|source| Error::Json {
        path: path.clone(),
        source,
    })?;
    std::fs::write(&path, json).map_err(|e| Error::io("write", &path, e))?;
    Ok(record)
}

/// The text the arm appends to the system prompt. `own` reads the person's
/// CLAUDE.md files now, not when the case was extracted: the arm measures the
/// instructions they keep today. Claude Code's own loading of those files is
/// switched off for every arm, so this is the only way they arrive.
fn appended_context(arm: &Arm, wd: &Path) -> Option<String> {
    match arm {
        Arm::None => None,
        Arm::File(text) | Arm::Lore(text) | Arm::Oracle(text) => Some(text.clone()),
        Arm::Own => {
            let files = claude_config_dir()
                .map(|dir| dir.join("CLAUDE.md"))
                .into_iter()
                .chain([wd.join("CLAUDE.md")]);
            let parts: Vec<String> = files
                .filter_map(|path| {
                    let text = std::fs::read_to_string(&path).ok()?;
                    Some(format!("Contents of {}:\n{text}", path.display()))
                })
                .collect();
            Some(parts.join("\n\n"))
        }
    }
}

fn tail(s: &str, chars: usize) -> String {
    let skip = s.chars().count().saturating_sub(chars);
    s.chars().skip(skip).collect()
}

/// Removes a run's scratch folders on drop, so a failed run is cleaned up too.
struct Scratch {
    dir: PathBuf,
    keep: bool,
}

impl Drop for Scratch {
    fn drop(&mut self) {
        if self.keep {
            return;
        }
        for name in ["wd", "home", "claude-project"] {
            let _ = std::fs::remove_dir_all(self.dir.join(name));
        }
    }
}

/// The replay's Claude Code project folder: a symlink under `projects/` to a
/// real folder in the run directory, removed on drop.
struct ProjectLink {
    path: PathBuf,
}

impl ProjectLink {
    fn create(path: &Path, target: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| Error::io("create", parent, e))?;
        }
        match std::fs::symlink_metadata(path) {
            Ok(meta) if meta.file_type().is_symlink() => {
                std::fs::remove_file(path)
                    .map_err(|e| Error::io("remove the stale link", path, e))?;
            }
            Ok(_) => return Err(Error::ProjectDirOccupied(path.to_owned())),
            Err(_) => {}
        }
        symlink(target, path).map_err(|e| Error::io("link the project folder", path, e))?;
        Ok(Self {
            path: path.to_owned(),
        })
    }
}

impl Drop for ProjectLink {
    fn drop(&mut self) {
        if std::fs::symlink_metadata(&self.path).is_ok_and(|m| m.file_type().is_symlink()) {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

#[cfg(unix)]
fn symlink(target: &Path, link: &Path) -> std::io::Result<()> {
    std::os::unix::fs::symlink(target, link)
}

#[cfg(not(unix))]
fn symlink(_target: &Path, _link: &Path) -> std::io::Result<()> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "replays need symlinks",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tail_keeps_the_last_characters() {
        assert_eq!(tail("abcdé", 2), "dé");
        assert_eq!(tail("ab", 5), "ab");
    }
}
