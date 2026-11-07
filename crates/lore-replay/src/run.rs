//! One replay: a case, an arm, a trial.
//!
//! Every run of a case shares one prompt-cache prefix (harness 2, see
//! [`HARNESS`]). Claude Code caches by prefix: tools, then the system prompt,
//! then the messages. The scratch path appears in the system prompt and in
//! the rewritten history, so every arm and trial of a case runs in the same
//! scratch folder, `<out>/<case>/scratch/wd`, resumes the same session id,
//! and gets the same argv; the arm's context arrives in the final user
//! message instead of the system prompt. The first run of a case writes the
//! cache and the rest read it.
//!
//! Sharing a path means runs of one case must not overlap, so a run holds an
//! exclusive lock on `<out>/<case>/scratch.lock` from before it rebuilds the
//! scratch until after it removes it. A second run of the case waits.
//!
//! The scratch (the clone `wd` and the stub tools `home`) is rebuilt fresh
//! before each run. The run directory, `<out>/<case>/<arm>-<trial>`, holds what
//! is the run's own: the real project folder Claude Code writes into
//! (`claude-project`), `outward.log`, and the record (`run.json`). The only
//! thing outside both is the project-folder symlink, removed when the agent
//! exits.
//!
//! The scratch and the project folder are deleted when the run ends, unless
//! [`RunOptions::keep_scratch`] is set. Each clone is a full copy of the
//! repository, and a few hundred kept clones of one large repository filled a
//! disk.

use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use serde_json::Value;

use crate::cases::{claude_config_dir, claude_projects_dir};
use crate::codex;
use crate::error::{Error, Result};
use crate::isolation::{self, project_slug};
use crate::prefix::{self, Rewrite};
use crate::scratch::{self, Source};
use crate::types::{Agent, Arm, Case, RunRecord, DELIVERY, HARNESS};

/// Overrides the `claude` found on `PATH`.
pub const CLAUDE_ENV: &str = "LORE_REPLAY_CLAUDE";
/// How much of the agent's stderr the record keeps, in characters.
const STDERR_TAIL: usize = 2000;
/// `--max-turns` unless the caller says otherwise. Of 103 real runs, 88 ended
/// within 4 turns.
pub const DEFAULT_MAX_TURNS: u32 = 6;
/// After a forced stop, how long [`drive`] waits for the agent's stdin and
/// stderr to close. A descendant that left the agent's process group survives
/// the stop and can hold them open for as long as it runs.
const STOPPED_PIPE_GRACE: Duration = Duration::from_secs(2);

#[derive(Debug, Clone)]
pub struct RunOptions {
    /// The root all run directories go under.
    pub out: PathBuf,
    pub trial: u32,
    pub model: String,
    pub max_turns: u32,
    /// Keep the last `trim` user and assistant lines of the prefix whole and
    /// trim older tool output and images; 0 is off. See [`prefix::trim_line`].
    pub trim: u32,
    /// Keep the scratch and `claude-project` after the run, for inspection.
    pub keep_scratch: bool,
}

/// `<out>/<case id, : as _>`: the case's run directories, scratch, and lock.
#[must_use]
pub fn case_dir(out: &Path, case_id: &str) -> PathBuf {
    out.join(case_id.replace(':', "_"))
}

/// `<out>/<case id, : as _>/<arm>-<trial>`.
#[must_use]
pub fn run_dir(case: &Case, arm: &Arm, opts: &RunOptions) -> PathBuf {
    case_dir(&opts.out, &case.id).join(format!("{}-{}", arm.name(), opts.trial))
}

/// Replay `case` under `arm`, write `run.json` in its run directory, and
/// return the record. A previous run in the same directory is replaced. Waits
/// while another run of the same case holds the case's scratch.
pub fn run(case: &Case, arm: &Arm, opts: &RunOptions) -> Result<RunRecord> {
    let case_root = case_dir(&opts.out, &case.id);
    std::fs::create_dir_all(&case_root).map_err(|e| Error::io("create", &case_root, e))?;
    let case_root =
        std::fs::canonicalize(&case_root).map_err(|e| Error::io("resolve", &case_root, e))?;
    // Declared first, so it is released last: after the scratch is removed.
    let _lock = CaseLock::take(&case_root.join("scratch.lock"))?;

    let dir = case_root.join(format!("{}-{}", arm.name(), opts.trial));
    remove_if_present(&dir, "remove the old run")?;
    std::fs::create_dir_all(&dir).map_err(|e| Error::io("create", &dir, e))?;
    let scratch_root = case_root.join("scratch");
    remove_if_present(&scratch_root, "remove the old scratch")?;
    std::fs::create_dir_all(&scratch_root).map_err(|e| Error::io("create", &scratch_root, e))?;
    let _scratch = Scratch {
        paths: vec![
            scratch_root.clone(),
            dir.join("claude-project"),
            dir.join("codex-home"),
        ],
        keep: opts.keep_scratch,
    };

    let source = scratch::source(case);
    let wd = scratch_root.join("wd");
    if let Source::Commit { toplevel, commit } = &source {
        scratch::clone_at(toplevel, commit, case.branch.as_deref(), &wd)?;
    }
    std::fs::create_dir_all(&wd).map_err(|e| Error::io("create", &wd, e))?;

    let session_id = case_session_id(&case.id);
    let session_bytes =
        std::fs::read(&case.file).map_err(|e| Error::io("read session file", &case.file, e))?;
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let rewrite = Rewrite {
        session_id: &session_id,
        scratch: &wd,
        toplevel: source.toplevel(),
        home: home.as_deref(),
        trim: opts.trim as usize,
    };
    let tools = isolation::tool_env(&scratch_root.join("home"), &dir.join("outward.log"))?;
    let prompt = user_message(arm_context(case.agent, arm, &wd).as_deref(), &case.prompt);
    let setup = Setup {
        case,
        opts,
        dir: &dir,
        wd: &wd,
        scratch_root: &scratch_root,
        session_id: &session_id,
        session_bytes: &session_bytes,
        rewrite: &rewrite,
        tools: &tools,
        prompt,
    };
    let ran = match case.agent {
        Agent::ClaudeCode => launch_claude(setup)?,
        Agent::Codex => launch_codex(setup)?,
    };

    let mut record = RunRecord {
        harness: HARNESS,
        delivery: Some(DELIVERY.to_owned()),
        case: case.id.clone(),
        agent: case.agent,
        arm: arm.name(),
        trial: opts.trial,
        model: requested_model(case.agent, &opts.model),
        resolved_model: ran.resolved_model,
        max_turns: Some(opts.max_turns),
        trim: opts.trim,
        fidelity: source.fidelity(),
        foreign_paths: isolation::foreign_paths(&case.actions, source.toplevel()),
        prefix_lines: case.prompt_line,
        seconds: ran.seconds,
        cost_usd: ran.cost_usd,
        is_error: ran.is_error,
        capped: ran.capped,
        limited: false,
        stderr: tail(&ran.stderr, STDERR_TAIL),
        outward: std::fs::read_to_string(&tools.outward_log)
            .map(|log| log.lines().map(str::to_owned).collect())
            .unwrap_or_default(),
        events: ran.events,
    };
    record.limited = limit_message(&record).is_some();
    let path = dir.join("run.json");
    let json = serde_json::to_string(&record).map_err(|source| Error::Json {
        path: path.clone(),
        source,
    })?;
    std::fs::write(&path, json).map_err(|e| Error::io("write", &path, e))?;
    Ok(record)
}

/// The model a run asks for and records: `model` when given, else the
/// agent's default (`sonnet` for Claude Code; Codex's own, recorded as
/// [`codex::DEFAULT_MODEL`]).
#[must_use]
pub fn requested_model(agent: Agent, model: &str) -> String {
    match (agent, model) {
        (_, m) if !m.is_empty() => m.to_owned(),
        (Agent::ClaudeCode, _) => "sonnet".to_owned(),
        (Agent::Codex, _) => codex::DEFAULT_MODEL.to_owned(),
    }
}

/// What both launches need.
struct Setup<'a> {
    case: &'a Case,
    opts: &'a RunOptions,
    dir: &'a Path,
    wd: &'a Path,
    scratch_root: &'a Path,
    session_id: &'a str,
    session_bytes: &'a [u8],
    rewrite: &'a Rewrite<'a>,
    tools: &'a isolation::ToolEnv,
    prompt: String,
}

/// What a launch reports back.
struct Ran {
    events: Vec<Value>,
    stderr: String,
    seconds: f64,
    cost_usd: Option<f64>,
    is_error: Option<bool>,
    capped: bool,
    resolved_model: Option<String>,
}

fn launch_claude(s: Setup<'_>) -> Result<Ran> {
    let project_real = s.dir.join("claude-project");
    std::fs::create_dir_all(&project_real).map_err(|e| Error::io("create", &project_real, e))?;
    let projects = claude_projects_dir().ok_or(Error::NoHome)?;
    let link = ProjectLink::create(&projects.join(project_slug(s.wd)), &project_real)?;

    let prefix = prefix::build(s.session_bytes, s.case.prompt_line, s.rewrite);
    let prefix_path = project_real.join(format!("{}.jsonl", s.session_id));
    std::fs::write(&prefix_path, &prefix.text).map_err(|e| Error::io("write", &prefix_path, e))?;

    let args = agent_args(prefix.has_messages.then_some(s.session_id), s.opts);
    let program = std::env::var_os(CLAUDE_ENV).unwrap_or_else(|| "claude".into());
    let mut cmd = Command::new(&program);
    cmd.args(&args)
        .current_dir(s.wd)
        .env("CLAUDE_CODE_DISABLE_CLAUDE_MDS", "1")
        .env("CLAUDE_CODE_DISABLE_AUTO_MEMORY", "1")
        .envs(s.tools.vars.iter().map(|(k, v)| (k, v)));
    let out = drive(cmd, &program, s.prompt, |_| false);
    drop(link);
    let out = out?;

    let result = result_event(&out.events);
    let capped = result
        .and_then(|r| r.get("subtype"))
        .and_then(Value::as_str)
        == Some("error_max_turns");
    Ok(Ran {
        cost_usd: result
            .and_then(|r| r.get("total_cost_usd"))
            .and_then(Value::as_f64),
        is_error: result
            .and_then(|r| r.get("is_error"))
            .and_then(Value::as_bool)
            .map(|e| e && !capped),
        capped,
        resolved_model: None,
        seconds: out.seconds,
        stderr: out.stderr,
        events: out.events,
    })
}

/// Codex: the prefix rollout goes into a `CODEX_HOME` of the run's own
/// (`<run>/codex-home`), where `exec resume` finds it and Lore never looks.
/// See [`crate::codex`] for the rest of the isolation.
fn launch_codex(s: Setup<'_>) -> Result<Ran> {
    let codex_home = s.dir.join("codex-home");
    std::fs::create_dir_all(&codex_home).map_err(|e| Error::io("create", &codex_home, e))?;
    let real_home = codex::codex_home().ok_or(Error::NoHome)?;
    codex::link_auth(&codex_home, &real_home)?;

    let prefix = codex::prefix(s.session_bytes, s.case.prompt_line, s.rewrite);
    let resume = prefix.has_messages.then_some(s.session_id);
    if resume.is_some() {
        let path = codex::rollout_path(&codex_home, s.session_id, s.case.prompt_ms);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| Error::io("create", parent, e))?;
        }
        std::fs::write(&path, &prefix.text).map_err(|e| Error::io("write", &path, e))?;
    }

    let tools_home = s.scratch_root.join("home");
    let tmp = tools_home.join("tmp");
    std::fs::create_dir_all(&tmp).map_err(|e| Error::io("create", &tmp, e))?;
    let log = &s.tools.outward_log;
    std::fs::write(log, "").map_err(|e| Error::io("write", log, e))?;
    let profile = codex::profile(&codex::Reach {
        tools: &tools_home,
        tmp: &tmp,
        outward_log: log,
    });
    let model = Some(s.opts.model.as_str()).filter(|m| !m.is_empty());
    let args = codex::args(resume, model, s.wd, &profile);
    let program = std::env::var_os(codex::CODEX_ENV).unwrap_or_else(|| "codex".into());
    let mut cmd = Command::new(&program);
    cmd.args(&args)
        .current_dir(s.wd)
        .env("CODEX_HOME", &codex_home)
        .env("TMPDIR", &tmp)
        .envs(s.tools.vars.iter().map(|(k, v)| (k, v)));
    let mut cap = codex::Cap::new(s.opts.max_turns);
    let out = drive(cmd, &program, s.prompt, |event| cap.reached(event));
    codex::restore_auth(&codex_home, &real_home);
    let out = out?;

    let (is_error, _) = codex::outcome(&out.events, out.stopped);
    Ok(Ran {
        cost_usd: None,
        is_error: Some(is_error),
        capped: out.stopped,
        resolved_model: codex::model_used(&codex_home),
        seconds: out.seconds,
        stderr: out.stderr,
        events: out.events,
    })
}

/// What [`drive`] collected.
struct Driven {
    events: Vec<Value>,
    stderr: String,
    seconds: f64,
    /// `stop` asked for the agent to be stopped, and it was.
    stopped: bool,
}

/// Start `cmd` with `prompt` on stdin, read its JSON lines from stdout, and
/// stop it when `stop` says so for an event.
fn drive(
    mut cmd: Command,
    program: &std::ffi::OsStr,
    prompt: String,
    mut stop: impl FnMut(&Value) -> bool,
) -> Result<Driven> {
    cmd.stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    // Its own process group, so stopping it stops what it started too: the
    // `codex` on PATH is a Node wrapper around the real binary.
    #[cfg(unix)]
    std::os::unix::process::CommandExt::process_group(&mut cmd, 0);
    let started = Instant::now();
    let mut child = cmd.spawn().map_err(|source| Error::Spawn {
        program: program.to_string_lossy().into_owned(),
        source,
    })?;
    let stdin = child.stdin.take().map(|mut pipe| {
        background(move || {
            // A closed pipe means the agent stopped reading; its exit says why.
            let _ = pipe.write_all(prompt.as_bytes());
        })
    });
    let stderr = child.stderr.take().map(|mut pipe| {
        background(move || {
            let mut buf = Vec::new();
            let _ = pipe.read_to_end(&mut buf);
            String::from_utf8_lossy(&buf).into_owned()
        })
    });
    let mut events = Vec::new();
    let mut stopped = false;
    if let Some(stdout) = child.stdout.take() {
        for line in BufReader::new(stdout)
            .split(b'\n')
            .map_while(std::io::Result::ok)
        {
            if let Ok(event) = serde_json::from_slice::<Value>(&line) {
                let halt = stop(&event);
                events.push(event);
                if halt {
                    stopped = true;
                    kill_group(&mut child);
                    break;
                }
            }
        }
    }
    let waited = child.wait();
    let seconds = (started.elapsed().as_secs_f64() * 10.0).round() / 10.0;
    if let Some(rx) = stdin {
        finish(&rx, stopped);
    }
    let stderr = stderr
        .and_then(|rx| finish(&rx, stopped))
        .unwrap_or_default();
    waited.map_err(|e| Error::io("wait for", Path::new(program), e))?;
    Ok(Driven {
        events,
        stderr,
        seconds,
        stopped,
    })
}

/// The agent's argv. It depends only on the case (through the session id)
/// and the options shared by a sweep, never on the arm or the trial, so every
/// run of a case sends the same tools and system prompt.
fn agent_args(resume: Option<&str>, opts: &RunOptions) -> Vec<String> {
    let mut args: Vec<String> = vec!["-p".into()];
    if let Some(session_id) = resume {
        args.extend([
            "--resume".into(),
            session_id.into(),
            "--fork-session".into(),
        ]);
    }
    args.extend([
        "--no-session-persistence".into(),
        "--model".into(),
        requested_model(Agent::ClaudeCode, &opts.model),
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
    args
}

/// The final user message: the arm's context in a `<system-reminder>` block,
/// then the original prompt. No block when the context is absent or empty.
#[must_use]
pub fn user_message(context: Option<&str>, prompt: &str) -> String {
    match context.filter(|c| !c.is_empty()) {
        Some(context) => format!("<system-reminder>\n{context}\n</system-reminder>\n\n{prompt}"),
        None => prompt.to_owned(),
    }
}

/// The session id a case's prefix is written under: derived from the case
/// id, so every run of the case resumes the same id and sends the same argv.
/// Runs of a case never overlap (see [`run`]), so they never share it at once.
#[must_use]
pub fn case_session_id(case_id: &str) -> String {
    let fnv = |seed: u64| {
        case_id.bytes().fold(seed, |h, b| {
            (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3)
        })
    };
    let mut bytes = [0_u8; 16];
    bytes[..8].copy_from_slice(&fnv(0xcbf2_9ce4_8422_2325).to_be_bytes());
    bytes[8..].copy_from_slice(&fnv(0x8422_2325_cbf2_9ce4).to_be_bytes());
    uuid::Builder::from_random_bytes(bytes)
        .into_uuid()
        .to_string()
}

/// Run `f` on its own thread; the receiver gets what it returns, or an
/// error if it panicked.
fn background<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> mpsc::Receiver<T> {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(f());
    });
    rx
}

/// What a [`background`] thread returned. Waits as long as it takes unless the
/// agent was `stopped`, so a run that ends on its own keeps all of its stderr
/// however long the last writer takes.
fn finish<T>(rx: &mpsc::Receiver<T>, stopped: bool) -> Option<T> {
    if stopped {
        rx.recv_timeout(STOPPED_PIPE_GRACE).ok()
    } else {
        rx.recv().ok()
    }
}

/// Stop `child` and every process in its group. [`drive`] made `child` the
/// leader of its own group, so the group id is its pid.
///
/// The signal goes from this process, not through a `kill` program: when one
/// misread its argv (procps on Linux takes `-<pid>` for an option) or failed
/// to run, only `child` was stopped, and what it had started kept running and
/// held its stderr open.
fn kill_group(child: &mut std::process::Child) {
    #[cfg(unix)]
    {
        use rustix::process::{kill_process_group, Pid, Signal};
        // Fails only when the group is already gone.
        let _ = kill_process_group(Pid::from_child(child), Signal::KILL);
    }
    let _ = child.kill();
}

/// The last `result` event of a run.
#[must_use]
pub fn result_event(events: &[Value]) -> Option<&Value> {
    events
        .iter()
        .rev()
        .find(|e| e.get("type").and_then(Value::as_str) == Some("result"))
}

/// True when the run failed because the subscription's usage limit was hit:
/// an error with zero turns or zero cost, or a result that says "hit your
/// ... limit". Such a run returns at once and costs nothing, so a sweep that
/// kept going would burn through every remaining run the same way.
#[must_use]
pub fn usage_limited(record: &RunRecord) -> bool {
    if record.limited || limit_message(record).is_some() {
        return true;
    }
    if record.agent == Agent::Codex {
        return false;
    }
    let result = result_event(&record.events);
    let text = result
        .and_then(|r| r.get("result"))
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_lowercase();
    if text.contains("hit your") && text.contains("limit") {
        return true;
    }
    let turns = result
        .and_then(|r| r.get("num_turns"))
        .and_then(Value::as_u64);
    record.is_error == Some(true) && (turns == Some(0) || record.cost_usd == Some(0.0))
}

/// The agent's own words saying the usage limit was hit, when it said so:
/// Claude Code's "You've hit your ... limit" in its result, Codex's "usage
/// limit" or "try again at" in an error event, or either on stderr. Only
/// error text is read, never what the agent wrote as its answer.
#[must_use]
pub fn limit_message(record: &RunRecord) -> Option<String> {
    let says_limit = |text: &str| {
        let t = text.to_lowercase();
        (t.contains("hit your") && t.contains("limit"))
            || t.contains("usage limit")
            || t.contains("try again at")
    };
    let errors: Vec<&str> = match record.agent {
        Agent::ClaudeCode => result_event(&record.events)
            .filter(|r| r.get("is_error").and_then(Value::as_bool) != Some(false))
            .and_then(|r| r.get("result"))
            .and_then(Value::as_str)
            .into_iter()
            .collect(),
        Agent::Codex => codex::error_messages(&record.events).collect(),
    };
    errors
        .into_iter()
        .chain(record.stderr.lines())
        .map(str::trim)
        .find(|text| says_limit(text))
        .map(|text| crate::claude::head(text, 200))
}

/// The one-line progress record `run` and `sweep` print for each run.
#[must_use]
pub fn progress(record: &RunRecord) -> String {
    serde_json::json!({
        "case": record.case, "arm": record.arm, "trial": record.trial,
        "fidelity": record.fidelity, "seconds": record.seconds,
        "cost_usd": record.cost_usd, "is_error": record.is_error,
    })
    .to_string()
}

/// The text the arm adds before the prompt. `own` reads the person's
/// instruction files now, not when the case was extracted: the arm measures
/// the instructions they keep today. The agent's own loading of those files
/// is switched off for every arm, so this is the only way they arrive.
fn arm_context(agent: Agent, arm: &Arm, wd: &Path) -> Option<String> {
    match arm {
        Arm::None => None,
        Arm::File(text) | Arm::Lore(text) | Arm::Oracle(text) => Some(text.clone()),
        Arm::Own => Some(own_context(agent, Some(wd))),
    }
}

/// The `own` arm's text, each file read now: for Claude Code the person's
/// `CLAUDE.md` in its config folder, then the repository's in `wd` when
/// given; for Codex the same with `AGENTS.md` and `$CODEX_HOME`.
#[must_use]
pub fn own_context(agent: Agent, wd: Option<&Path>) -> String {
    let (dir, name) = match agent {
        Agent::ClaudeCode => (claude_config_dir(), "CLAUDE.md"),
        Agent::Codex => (codex::codex_home(), "AGENTS.md"),
    };
    let files = dir
        .map(|dir| dir.join(name))
        .into_iter()
        .chain(wd.map(|wd| wd.join(name)));
    let parts: Vec<String> = files
        .filter_map(|path| {
            let text = std::fs::read_to_string(&path).ok()?;
            Some(format!("Contents of {}:\n{text}", path.display()))
        })
        .collect();
    parts.join("\n\n")
}

fn tail(s: &str, chars: usize) -> String {
    let skip = s.chars().count().saturating_sub(chars);
    s.chars().skip(skip).collect()
}

fn remove_if_present(dir: &Path, action: &'static str) -> Result<()> {
    match std::fs::remove_dir_all(dir) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(Error::io(action, dir, e)),
    }
}

/// An exclusive lock on a case's scratch, held until drop. Blocks while
/// another run of the case holds it.
struct CaseLock {
    _file: std::fs::File,
}

impl CaseLock {
    fn take(path: &Path) -> Result<Self> {
        let file = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(path)
            .map_err(|e| Error::io("open the lock", path, e))?;
        file.lock()
            .map_err(|e| Error::io("lock the case scratch", path, e))?;
        // The lock is released when the file closes.
        Ok(Self { _file: file })
    }
}

/// Removes a run's scratch folders on drop, so a failed run is cleaned up too.
struct Scratch {
    paths: Vec<PathBuf>,
    keep: bool,
}

impl Drop for Scratch {
    fn drop(&mut self) {
        if self.keep {
            return;
        }
        for path in &self.paths {
            let _ = std::fs::remove_dir_all(path);
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
pub(crate) fn symlink(target: &Path, link: &Path) -> std::io::Result<()> {
    std::os::unix::fs::symlink(target, link)
}

#[cfg(not(unix))]
pub(crate) fn symlink(_target: &Path, _link: &Path) -> std::io::Result<()> {
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

    #[test]
    fn the_context_goes_before_the_prompt_in_a_reminder() {
        assert_eq!(
            user_message(Some("Do X."), "fix it"),
            "<system-reminder>\nDo X.\n</system-reminder>\n\nfix it"
        );
        assert_eq!(user_message(None, "fix it"), "fix it");
        assert_eq!(user_message(Some(""), "fix it"), "fix it");
    }

    #[test]
    fn a_case_has_one_stable_session_id() {
        let id = case_session_id("s_abc:4");
        assert_eq!(id, case_session_id("s_abc:4"));
        assert_ne!(id, case_session_id("s_abc:5"));
        assert!(uuid::Uuid::parse_str(&id).is_ok());
    }

    fn record(is_error: bool, cost: Option<f64>, result: Value) -> RunRecord {
        serde_json::from_value(serde_json::json!({
            "case": "c", "arm": "none", "trial": 0, "model": "m", "fidelity": "no-cwd",
            "foreign_paths": [], "prefix_lines": 0, "seconds": 0.0, "cost_usd": cost,
            "is_error": is_error, "stderr": "", "outward": [], "events": [result],
        }))
        .unwrap_or_else(|e| panic!("{e}"))
    }

    #[test]
    fn a_usage_limit_is_recognised() {
        let limit = serde_json::json!({"type": "result", "is_error": true, "num_turns": 0,
            "result": "You've hit your 5-hour limit"});
        assert!(usage_limited(&record(true, Some(0.0), limit)));
        let free = serde_json::json!({"type": "result", "is_error": true, "num_turns": 3});
        assert!(usage_limited(&record(true, Some(0.0), free)));
        let real = serde_json::json!({"type": "result", "is_error": true, "num_turns": 3});
        assert!(!usage_limited(&record(true, Some(0.4), real)));
        let ok = serde_json::json!({"type": "result", "is_error": false, "num_turns": 0});
        assert!(!usage_limited(&record(false, Some(0.0), ok)));
    }

    #[test]
    fn old_records_read_as_harness_one() {
        let r = record(false, None, Value::Null);
        assert_eq!(r.harness, 1);
        assert_eq!(r.delivery, None);
    }
}
