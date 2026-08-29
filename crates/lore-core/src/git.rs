//! Git evidence: provenance-separated observations and repository identity.
//!
//! M4 (`docs/architecture/GIT_INTEGRATION.md`, ADR-0006). Live repository reads
//! use `gix` (gitoxide) with network transports disabled at the dependency level
//! so the archive core stays network-incapable; a hardened system-`git` fallback
//! is added in a later increment. All operations here are strictly read-only:
//! Lore never fetches, checks out, commits, mutates refs/index/worktree, or runs
//! hooks/helpers. This module also owns remote-URL normalization, which strips
//! credentials so a recorded remote can be stored in
//! `git_observation.remote_url_norm` and used as identity evidence without ever
//! persisting a secret.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// Normalize a git remote URL to a canonical, credential-free `host[:port]/path`
/// form suitable for identity matching and safe storage.
///
/// - Credentials (`user[:secret]@`) are removed — they are never stored.
/// - `scheme://`, scp-like (`git@host:path`), and bare `host/path` forms all
///   normalize to the same shape.
/// - The host is lowercased; a default git port (22/80/443/9418) is dropped; a
///   trailing `.git` and surrounding slashes are removed. The path is preserved
///   as-is (it may be case-sensitive on some forges).
///
/// Returns `None` for empty/whitespace input. Credentials never appear in the
/// output for any input.
#[must_use]
pub fn normalize_remote_url(raw: &str) -> Option<String> {
    let raw = raw.trim();
    if raw.is_empty()
        || raw
            .chars()
            .any(|c| c.is_ascii_control() || c.is_ascii_whitespace())
    {
        return None;
    }

    let (authority, path) = if let Some(scheme_end) = raw.find("://") {
        // scheme://[userinfo@]host[:port]/path
        split_authority_path(&raw[scheme_end + 3..])
    } else if let Some(colon) = raw.find(':') {
        // scp-like: [userinfo@]host:path (no scheme, path after the first colon)
        (&raw[..colon], trim_slashes(&raw[colon + 1..]))
    } else {
        // bare host/path
        split_authority_path(raw)
    };

    let host = canonical_host(strip_userinfo(authority));
    if host.is_empty() {
        return None;
    }
    let path = clean_path(path);
    if path.is_empty() {
        Some(host)
    } else {
        Some(format!("{host}/{path}"))
    }
}

/// Read-only facts captured from a live repository at observation time. These
/// are `lore_captured` evidence — true at capture, never backdated to session
/// time (`GIT_INTEGRATION.md` §4).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapturedRepo {
    /// Canonical git common directory shared by all linked worktrees of one
    /// local repository instance — the local grouping key for identity.
    pub common_dir: PathBuf,
    /// The worktree root, when the repository is not bare.
    pub workdir: Option<PathBuf>,
    /// Current branch short name, or `None` when HEAD is detached or unborn.
    pub branch: Option<String>,
    /// HEAD commit id (full hex), or `None` on an unborn branch.
    pub head_commit: Option<String>,
    /// HEAD is detached (a commit, not a branch).
    pub detached: bool,
    /// Working tree has uncommitted changes at capture, when determinable.
    pub is_dirty: Option<bool>,
    /// Repository-relative paths of the uncommitted working-tree changes, capped
    /// at [`CHANGED_FILES_CAP`]. `None` when the status could not be read safely.
    pub changed_files: Option<Vec<String>>,
    /// Commits the local branch is ahead of its tracking branch, when both exist
    /// and the count is bounded. `None` when detached, no upstream, or too large.
    pub ahead: Option<i64>,
    /// Commits the local branch is behind its tracking branch (see `ahead`).
    pub behind: Option<i64>,
    /// Subject (first line) of the HEAD commit, when HEAD exists and decodes.
    pub commit_subject: Option<String>,
    /// Normalized (credential-free) fetch remotes, sorted and deduped. Empty for
    /// a local-only repository.
    pub remotes: Vec<String>,
    /// Sorted set of root commits (parentless) reachable from HEAD. Two clones of
    /// one upstream share this set; a fork shares it too, which is why root alone
    /// never merges repositories.
    pub root_commits: Vec<String>,
    /// The root walk hit its bound, so `root_commits` may be incomplete; identity
    /// then falls back to weaker evidence rather than trusting a partial set.
    pub history_truncated: bool,
}

/// Upper bound on commits walked to find the root set, so a very large history
/// cannot make capture unbounded. Beyond this the root set is treated as unknown.
const ROOT_WALK_LIMIT: usize = 50_000;

/// Upper bound on the number of changed files recorded in a capture, so a huge
/// dirty working tree cannot make capture unbounded or its summary unbounded
/// (F9). Beyond this the list is truncated to the first entries.
const CHANGED_FILES_CAP: usize = 500;

/// Upper bound on the ahead/behind commit walk. Beyond this the counts are
/// treated as unknown rather than walking an unbounded history (F9).
const AHEAD_BEHIND_LIMIT: i64 = 10_000;

/// Capture read-only repository facts for the repository containing `path`.
///
/// Primary path is `gix`; if `gix` cannot read the repository but a `.git`
/// exists in an ancestor (so it really is a repository `gix` choked on), fall
/// back to the hardened system-`git` reader. Returns `None` when `path` is not
/// inside a git repository at all — without spawning any process in that case.
/// Never mutates anything and never touches the network. Non-absolute paths are
/// declined to prevent resolving relative paths against the host process.
#[must_use]
pub fn capture(path: &Path) -> Option<CapturedRepo> {
    if !path.is_absolute() {
        return None;
    }
    if let Some(facts) = capture_gix(path) {
        return Some(facts);
    }
    if looks_like_repo(path) {
        capture_via_git(path)
    } else {
        None
    }
}

/// Cheap ancestor check for a `.git` entry, so the fallback never spawns a
/// process for a plainly non-git path.
fn looks_like_repo(path: &Path) -> bool {
    let mut dir = Some(path);
    while let Some(current) = dir {
        if current.join(".git").exists() {
            return true;
        }
        dir = current.parent();
    }
    false
}

/// Capture via `gix`. Detached HEAD yields `branch = None` with the commit still
/// recorded.
fn capture_gix(path: &Path) -> Option<CapturedRepo> {
    let repo = gix::discover(path).ok()?;

    let raw_common = repo.common_dir();
    let common_dir = raw_common
        .canonicalize()
        .unwrap_or_else(|_| raw_common.to_path_buf());
    let workdir = repo
        .workdir()
        .map(|w| w.canonicalize().unwrap_or_else(|_| w.to_path_buf()));

    let head = repo.head().ok()?;
    let detached = head.is_detached();
    let branch = head
        .referent_name()
        .map(|name| name.shorten().to_string())
        .filter(|_| !detached);
    let head_commit = repo.head_id().ok().map(|id| id.to_hex().to_string());
    let is_dirty = repo.is_dirty().ok();
    let commit_subject = repo.head_commit().ok().and_then(|commit| {
        commit
            .message()
            .ok()
            .map(|m| m.title.to_string().trim_end().to_string())
    });
    let changed_files = collect_changed_files(&repo);
    let (ahead, behind) = collect_ahead_behind(&repo);
    let remotes = collect_remotes(&repo);
    let (root_commits, history_truncated) = collect_root_commits(&repo);

    Some(CapturedRepo {
        common_dir,
        workdir,
        branch,
        head_commit,
        detached,
        is_dirty,
        changed_files,
        ahead,
        behind,
        commit_subject,
        remotes,
        root_commits,
        history_truncated,
    })
}

/// Result of re-checking a previously recorded commit/branch against the
/// repository as it exists now (`lore_reverified`; `GIT_INTEGRATION.md` §4, §6).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reverification {
    /// The recorded commit object still exists (false after a GC/rebase drop).
    pub commit_exists: bool,
    /// The recorded branch still exists — `None` when no branch was recorded.
    pub branch_exists: Option<bool>,
    /// The recorded branch still points at the recorded commit — `None` when the
    /// branch is gone or nothing was recorded to compare.
    pub branch_at_recorded_commit: Option<bool>,
}

/// Re-verify a recorded commit (and optional branch) against the current
/// repository at `path`, read-only. Returns `None` when the repository cannot be
/// read (e.g. it was moved or deleted). Never mutates history; the caller records
/// the result as a separate `lore_reverified` observation.
#[must_use]
pub fn reverify(path: &Path, commit_sha: &str, branch: Option<&str>) -> Option<Reverification> {
    if !path.is_absolute() {
        return None;
    }
    let repo = gix::discover(path).ok()?;
    let parsed_oid = gix::ObjectId::from_hex(commit_sha.as_bytes()).ok();
    let commit_exists = parsed_oid.is_some_and(|oid| repo.find_object(oid).is_ok());

    let (branch_exists, branch_at_recorded_commit) = match branch {
        Some(branch) => match repo.find_reference(&format!("refs/heads/{branch}")) {
            Ok(mut reference) => {
                let at = reference
                    .peel_to_id()
                    .ok()
                    .map(|id| parsed_oid.is_some_and(|target| id.detach() == target));
                (Some(true), at)
            }
            Err(_) => (Some(false), None),
        },
        None => (None, None),
    };

    Some(Reverification {
        commit_exists,
        branch_exists,
        branch_at_recorded_commit,
    })
}

/// Normalized, credential-free fetch remotes (sorted, deduped).
fn collect_remotes(repo: &gix::Repository) -> Vec<String> {
    let mut out = Vec::new();
    for name in repo.remote_names() {
        let name_ref: &gix::bstr::BStr = name.as_ref();
        let Ok(remote) = repo.find_remote(name_ref) else {
            continue;
        };
        if let Some(url) = remote.url(gix::remote::Direction::Fetch) {
            if let Some(normalized) = normalize_remote_url(&url.to_bstring().to_string()) {
                out.push(normalized);
            }
        }
    }
    out.sort();
    out.dedup();
    out
}

/// Repository-relative paths of uncommitted working-tree changes, capped at
/// [`CHANGED_FILES_CAP`]. Reads the index-vs-worktree status via `gix` only —
/// never the hardened system-`git` fallback, which deliberately excludes
/// `status`/`diff` so hostile `.gitattributes` filters cannot execute (F9).
fn collect_changed_files(repo: &gix::Repository) -> Option<Vec<String>> {
    let platform = repo.status(gix::progress::Discard).ok()?;
    let iter = platform.into_index_worktree_iter(Vec::new()).ok()?;
    let mut out = Vec::new();
    for item in iter {
        let Ok(item) = item else { break };
        // `summary() == None` means "no real change" (a stat refresh or a
        // non-untracked directory walk entry) — not a changed file.
        if item.summary().is_none() {
            continue;
        }
        out.push(item.rela_path().to_string());
        if out.len() >= CHANGED_FILES_CAP {
            break;
        }
    }
    Some(out)
}

/// Ahead/behind commit counts relative to the branch's tracking branch, bounded
/// by [`AHEAD_BEHIND_LIMIT`]. `None` when detached/unborn, when there is no
/// configured upstream, or when the walk exceeds the bound — leave NULL rather
/// than approximate (F9).
fn collect_ahead_behind(repo: &gix::Repository) -> (Option<i64>, Option<i64>) {
    let Some((ahead, behind)) = (|| {
        let head = repo.head().ok()?;
        let head_id = head.id()?;
        let branch_ref = head.try_into_referent()?;
        let tracking_name = branch_ref
            .remote_tracking_ref_name(gix::remote::Direction::Fetch)?
            .ok()?;
        let mut tracking_ref = repo.find_reference(tracking_name.as_ref()).ok()?;
        let tracking_id = tracking_ref.peel_to_id().ok()?;
        let base = repo
            .merge_base(head_id.detach(), tracking_id.detach())
            .ok()?;
        let base_oid = base.detach();
        let ahead = count_until(repo, head_id.detach(), &base_oid)?;
        let behind = count_until(repo, tracking_id.detach(), &base_oid)?;
        Some((ahead, behind))
    })() else {
        return (None, None);
    };
    (Some(ahead), Some(behind))
}

/// Number of commits reachable from `from` before reaching `stop` (exclusive),
/// or `None` if the walk errors or exceeds [`AHEAD_BEHIND_LIMIT`].
fn count_until(repo: &gix::Repository, from: gix::ObjectId, stop: &gix::ObjectId) -> Option<i64> {
    let walk = repo.rev_walk(Some(from)).all().ok()?;
    let mut count = 0i64;
    for item in walk {
        let Ok(info) = item else { return None };
        if info.id == *stop {
            return Some(count);
        }
        count += 1;
        if count > AHEAD_BEHIND_LIMIT {
            return None;
        }
    }
    None
}

/// Sorted set of parentless (root) commits reachable from HEAD, bounded by
/// [`ROOT_WALK_LIMIT`]. Returns `(roots, truncated)`; on truncation the caller
/// treats the set as unknown rather than partial.
fn collect_root_commits(repo: &gix::Repository) -> (Vec<String>, bool) {
    let Ok(head) = repo.head_id() else {
        return (Vec::new(), false);
    };
    let Ok(walk) = repo.rev_walk(Some(head.detach())).all() else {
        return (Vec::new(), false);
    };
    let mut roots = std::collections::BTreeSet::new();
    let mut visited = 0usize;
    for item in walk {
        let Ok(info) = item else {
            return (Vec::new(), false);
        };
        visited += 1;
        if visited > ROOT_WALK_LIMIT {
            return (Vec::new(), true);
        }
        if info.parent_ids().count() == 0 {
            roots.insert(info.id.to_hex().to_string());
        }
    }
    (roots.into_iter().collect(), false)
}

/// Split `host[/path]` at the first `/`.
fn split_authority_path(s: &str) -> (&str, &str) {
    match s.find('/') {
        Some(slash) => (&s[..slash], &s[slash + 1..]),
        None => (s, ""),
    }
}

/// Drop `userinfo@` (using the last `@` so a secret containing `@` cannot leak).
fn strip_userinfo(authority: &str) -> &str {
    match authority.rfind('@') {
        Some(at) => &authority[at + 1..],
        None => authority,
    }
}

/// Lowercase the host and drop a well-known default git port.
fn canonical_host(host_port: &str) -> String {
    let lowered = host_port.to_ascii_lowercase();
    if let Some((host, port)) = lowered.rsplit_once(':') {
        if matches!(port, "22" | "80" | "443" | "9418") {
            return host.to_string();
        }
    }
    lowered
}

fn trim_slashes(path: &str) -> &str {
    path.trim_matches('/')
}

/// Trim surrounding slashes and a trailing `.git`.
fn clean_path(path: &str) -> String {
    let path = trim_slashes(path);
    path.strip_suffix(".git").unwrap_or(path).to_string()
}

// ── Hardened system-git fallback (GIT_INTEGRATION.md §5) ─────────────────────

/// Wall-clock cap on a single git invocation.
const GIT_TIMEOUT: Duration = Duration::from_secs(10);
/// Maximum bytes retained from a git command's stdout (excess is drained and
/// discarded so the child can finish, but never buffered).
const OUTPUT_CAP: usize = 4 * 1024 * 1024;
/// The only git subcommands the fallback may ever run — all local and read-only.
/// `status` and `diff` are excluded because repo-local `.gitattributes` clean/textconv
/// filters could trigger executable helper execution under hostile clones.
const ALLOWED_SUBCOMMANDS: &[&str] = &["rev-parse", "rev-list", "for-each-ref", "cat-file"];
/// Config overrides that neutralize every executable-config vector git honors,
/// applied on the command line so they win over hostile repo-local config.
const SAFE_CONFIG: &[&str] = &[
    "core.fsmonitor=",          // no filesystem-monitor hook (runs on status)
    "core.hooksPath=/dev/null", // no hooks
    "core.pager=cat",           // never launch a pager
    "core.editor=false",
    "core.sshCommand=false",
    "core.askpass=",
    "diff.external=",       // no external diff driver
    "credential.helper=",   // no credential helpers
    "protocol.allow=never", // deny all transports
    "uploadpack.packObjectsHook=",
    "gc.auto=0",
];

/// Fallback capture via a hardened, read-only system-`git`. Provides the core
/// facts (branch/HEAD/common dir/roots). Dirtiness is not read here (taking it
/// exclusively from `gix` prevents running external filter scripts).
#[must_use]
pub fn capture_via_git(path: &Path) -> Option<CapturedRepo> {
    let common_raw = run_git(path, &["rev-parse", "--git-common-dir"])?;
    let common_dir = resolve_dir(path, common_raw.trim());
    let workdir = run_git(path, &["rev-parse", "--show-toplevel"])
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .map(|s| resolve_dir(path, &s));

    let head_ref = run_git(path, &["rev-parse", "--abbrev-ref", "HEAD"])?;
    let detached = head_ref.trim() == "HEAD";
    let branch = (!detached).then(|| head_ref.trim().to_string());
    let head_commit = run_git(path, &["rev-parse", "HEAD"]).map(|s| s.trim().to_string());
    let is_dirty = None;
    let root_commits = run_git(path, &["rev-list", "--max-parents=0", "HEAD"])
        .map(|s| {
            let mut roots: Vec<String> = s.split_whitespace().map(str::to_string).collect();
            roots.sort();
            roots.dedup();
            roots
        })
        .unwrap_or_default();

    Some(CapturedRepo {
        common_dir,
        workdir,
        branch,
        head_commit,
        detached,
        is_dirty,
        // The hardened fallback never runs `status`/`diff`, so these stay NULL
        // (F9: gix-only, never approximate).
        changed_files: None,
        ahead: None,
        behind: None,
        commit_subject: None,
        remotes: Vec::new(),
        root_commits,
        history_truncated: false,
    })
}

/// Resolve a possibly-relative git path against the query directory.
fn resolve_dir(base: &Path, raw: &str) -> PathBuf {
    let candidate = PathBuf::from(raw);
    let candidate = if candidate.is_absolute() {
        candidate
    } else {
        base.join(candidate)
    };
    candidate.canonicalize().unwrap_or(candidate)
}

/// Run one allowlisted, hardened git command and return its trimmed stdout, or
/// `None` on a disallowed subcommand, spawn failure, non-zero exit, timeout, or
/// non-UTF-8 output. Never uses a shell; arguments are passed as an array.
fn run_git(repo: &Path, args: &[&str]) -> Option<String> {
    if !args
        .first()
        .is_some_and(|sub| ALLOWED_SUBCOMMANDS.contains(sub))
    {
        return None;
    }

    let mut cmd = Command::new("git");
    cmd.arg("--no-optional-locks");
    for override_kv in SAFE_CONFIG {
        cmd.arg("-c").arg(override_kv);
    }
    cmd.args(args);
    cmd.current_dir(repo);
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    sanitize_env(&mut cmd);

    let mut child = cmd.spawn().ok()?;
    // Drain both pipes on threads so a large output cannot deadlock the wait.
    let mut stdout = child.stdout.take()?;
    let mut stderr = child.stderr.take()?;
    let out_thread = std::thread::spawn(move || read_capped(&mut stdout));
    let err_thread = std::thread::spawn(move || drain(&mut stderr));

    let start = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) => {
                if start.elapsed() > GIT_TIMEOUT {
                    let _ = child.kill();
                    let _ = child.wait();
                    break None;
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            Err(_) => break None,
        }
    };

    let stdout_bytes = out_thread.join().ok()?;
    let _ = err_thread.join();
    if !status?.success() {
        return None;
    }
    String::from_utf8(stdout_bytes).ok()
}

/// A sanitized environment: cleared, then only PATH plus vars that disable
/// prompts, ignore system/global config, deny transports, and refuse askpass.
fn sanitize_env(cmd: &mut Command) {
    cmd.env_clear();
    if let Some(path) = std::env::var_os("PATH") {
        cmd.env("PATH", path);
    }
    cmd.env("HOME", "/nonexistent");
    cmd.env("GIT_TERMINAL_PROMPT", "0");
    cmd.env("GIT_CONFIG_NOSYSTEM", "1");
    cmd.env("GIT_CONFIG_GLOBAL", "/dev/null");
    cmd.env("GIT_CONFIG_SYSTEM", "/dev/null");
    cmd.env("GIT_ATTR_NOSYSTEM", "1");
    cmd.env("GIT_OPTIONAL_LOCKS", "0");
    cmd.env("GIT_ALLOW_PROTOCOL", ""); // empty allow-list denies every transport
    cmd.env("GIT_ASKPASS", "/bin/false");
    cmd.env("SSH_ASKPASS", "/bin/false");
}

/// Read up to `OUTPUT_CAP` bytes; keep draining past the cap (discarding) so the
/// child never blocks on a full pipe, but never buffer more than the cap.
fn read_capped(reader: &mut impl Read) -> Vec<u8> {
    let mut buf = Vec::with_capacity(4096.min(OUTPUT_CAP));
    let mut chunk = [0u8; 8192];
    loop {
        match reader.read(&mut chunk) {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                if buf.len() < OUTPUT_CAP {
                    let take = (OUTPUT_CAP - buf.len()).min(n);
                    buf.extend_from_slice(&chunk[..take]);
                }
            }
        }
    }
    buf
}

/// Drain a pipe to EOF, discarding everything (used for stderr).
fn drain(reader: &mut impl Read) {
    let mut sink = [0u8; 8192];
    while matches!(reader.read(&mut sink), Ok(n) if n > 0) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_credentials_and_canonicalizes() {
        // Credential-bearing HTTPS URL: secret removed, .git dropped.
        let out =
            normalize_remote_url("https://user:ghp_secrettoken@github.com/org/repo.git").unwrap();
        assert_eq!(out, "github.com/org/repo");
        assert!(!out.contains("ghp_secrettoken"));
        assert!(!out.contains('@'));
    }

    #[test]
    fn normalizes_scp_scheme_and_bare_forms_identically() {
        let canonical = "github.com/org/repo";
        assert_eq!(
            normalize_remote_url("git@github.com:org/repo.git").as_deref(),
            Some(canonical)
        );
        assert_eq!(
            normalize_remote_url("ssh://git@github.com:22/org/repo.git").as_deref(),
            Some(canonical)
        );
        assert_eq!(
            normalize_remote_url("https://github.com/org/repo").as_deref(),
            Some(canonical)
        );
        // Bare form (as recorded in the Codex fixture).
        assert_eq!(
            normalize_remote_url("github.com/org/repo").as_deref(),
            Some(canonical)
        );
    }

    #[test]
    fn lowercases_host_but_preserves_path_case() {
        assert_eq!(
            normalize_remote_url("HTTPS://GitHub.com/Org/Repo.git").as_deref(),
            Some("github.com/Org/Repo")
        );
    }

    #[test]
    fn keeps_non_default_port() {
        assert_eq!(
            normalize_remote_url("ssh://git@host.example.com:2222/team/repo.git").as_deref(),
            Some("host.example.com:2222/team/repo")
        );
    }

    #[test]
    fn empty_input_is_none() {
        assert!(normalize_remote_url("").is_none());
        assert!(normalize_remote_url("   ").is_none());
        assert!(normalize_remote_url("https://github.com/org/repo\n/bad").is_none());
        assert!(normalize_remote_url("https://github.com/org/repo with space").is_none());
        assert!(normalize_remote_url("https://github.com/org/repo\0evil").is_none());
    }

    #[test]
    fn multiple_at_signs_do_not_leak_userinfo() {
        // An `@` inside the userinfo must not be mistaken for the host boundary.
        let out = normalize_remote_url("https://user:p@ss@gitlab.example.com/g/p.git").unwrap();
        assert_eq!(out, "gitlab.example.com/g/p");
    }

    #[test]
    fn capture_on_unborn_repository_handles_zero_commits_cleanly() {
        let temp = tempfile::tempdir().unwrap();
        let repo_path = temp.path();
        let _ = std::process::Command::new("git")
            .arg("init")
            .current_dir(repo_path)
            .output();

        let captured = capture(repo_path);
        assert!(captured.is_some());
        let c = captured.unwrap();
        assert!(c.branch.is_some());
        assert_eq!(c.head_commit, None);
        assert!(!c.detached);
        assert!(c.root_commits.is_empty());
    }

    #[test]
    fn normalize_remote_url_with_git_protocol_and_custom_users() {
        assert_eq!(
            normalize_remote_url("git://github.com/org/repo.git").as_deref(),
            Some("github.com/org/repo")
        );
        assert_eq!(
            normalize_remote_url(
                "git+ssh://custom_user:secret_token@gitlab.example.com:9418/org/repo.git"
            )
            .as_deref(),
            Some("gitlab.example.com/org/repo")
        );
        assert_eq!(
            normalize_remote_url("git@ssh.dev.azure.com:v3/Company/Project/Repo").as_deref(),
            Some("ssh.dev.azure.com/v3/Company/Project/Repo")
        );
    }
}
