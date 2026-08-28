//! Git enrichment: resolve repository/worktree identity and record
//! `lore_captured` observations for a persisted session's segments.
//!
//! For each segment that carries a `cwd` and is not yet linked, Lore reads the
//! live repository (read-only, via [`crate::git::capture`]) and:
//! - groups it under a Repository keyed by the resolved git common directory
//!   (the high-confidence local grouping — linked worktrees of one repo share
//!   it, so they attach to one Repository; `GIT_INTEGRATION.md` §2);
//! - upserts the Worktree and a `git_common_dir` identity-evidence row;
//! - records a `lore_captured` GitObservation whose branch/HEAD/dirty state is
//!   labeled `current_only` — true at capture, never backdated to session time.
//!
//! [`reverify_session`] is the companion batch pass: it re-checks recorded
//! commits/branches against the repository as it exists now and records
//! `lore_reverified` observations, never overwriting the historical rows.
//!
//! Ambiguous remote/root-only matching and cross-repository merge/split are
//! deliberately out of scope here and remain separate observations. Capture
//! (filesystem I/O) runs before the write transaction opens; all row writes
//! commit atomically.

use std::collections::HashMap;
use std::path::Path;

use rusqlite::{params, Connection, OptionalExtension};

use crate::git::{self, CapturedRepo};
use crate::ingest::det_id;
use crate::storage::Result;

/// User-driven repository assignment correction (GAP-M4-01 / I3).
/// Moves a session segment to a target repository, updating resolution confidence to `high`
/// and refreshing the session's git search projection.
pub fn relink_segment_repository(
    conn: &Connection,
    segment_id: &str,
    target_repo_id: &str,
) -> Result<()> {
    let _write = crate::storage::write_lock();
    let tx = conn.unchecked_transaction()?;
    let session_id: Option<String> = tx
        .query_row(
            "SELECT session_id FROM session_segment WHERE id = ?1",
            params![segment_id],
            |row| row.get(0),
        )
        .optional()?;

    let Some(session_id) = session_id else {
        return Ok(());
    };

    tx.execute(
        "UPDATE session_segment
         SET repository_id = ?1, resolution_confidence = 'high'
         WHERE id = ?2",
        params![target_repo_id, segment_id],
    )?;

    crate::search::project_session_git(&tx, &session_id)?;
    tx.commit()?;
    Ok(())
}

/// Enrich every unresolved, cwd-bearing segment of `session_id`. Returns the
/// number of segments linked to a repository. Segments whose cwd is not inside a
/// git repository are left unlinked ("No repository") without error.
pub fn enrich_session(conn: &Connection, session_id: &str) -> Result<usize> {
    let segments = unresolved_segments(conn, session_id)?;
    if segments.is_empty() {
        return Ok(0);
    }

    // Capture is filesystem I/O: do it before opening the write transaction,
    // coalescing repeated cwds so one repository is read at most once.
    let mut cache: HashMap<String, Option<CapturedRepo>> = HashMap::new();
    let mut resolved: Vec<(String, CapturedRepo)> = Vec::new();
    for (segment_id, cwd) in segments {
        let facts = cache
            .entry(cwd.clone())
            .or_insert_with(|| git::capture(Path::new(&cwd)))
            .clone();
        if let Some(facts) = facts {
            resolved.push((segment_id, facts));
        }
    }
    if resolved.is_empty() {
        return Ok(0);
    }

    let _write = crate::storage::write_lock();
    let tx = conn.unchecked_transaction()?;
    let mut enriched = 0;
    for (segment_id, facts) in &resolved {
        link_segment(&tx, session_id, segment_id, facts)?;
        enriched += 1;
    }
    // Repository/worktree linkage and the lore_captured observations just
    // changed, so refresh the git filter projection in the same transaction.
    crate::search::project_session_git(&tx, session_id)?;
    tx.commit()?;
    Ok(enriched)
}

/// Re-verify a session's agent-recorded commits against the repositories as they
/// exist now, recording `lore_reverified` observations. This never modifies the
/// original `agent_recorded` rows: a rebased/GC'd/deleted commit yields a new
/// observation with `commit_exists = 0`, and history is preserved
/// (`GIT_INTEGRATION.md` §6).
///
/// Observations are **appended when the verdict changes**, not on every run: the
/// conclusion is folded into the observation id, so re-checking an unchanged
/// commit refreshes that row's `last_checked_at` metadata while a commit that
/// has since disappeared adds a second row beside the first. The transition is
/// therefore recoverable ("existed on the 3rd, gone by the 20th") without the
/// row count growing with the number of background passes.
///
/// Intended as a batch/background pass, not part of ingest — scheduled by the
/// low-priority `reverify` job drained from the pipeline (Phase 1 `I2`), and
/// callable on demand. Returns the number of targets checked, which is not the
/// number of rows added.
pub fn reverify_session(conn: &Connection, session_id: &str) -> Result<usize> {
    let targets = reverify_targets(conn, session_id)?;
    if targets.is_empty() {
        return Ok(0);
    }
    // Filesystem/git reads happen before the write transaction opens.
    let outcomes: Vec<ReverifyOutcome> = targets.into_iter().map(outcome_for_target).collect();
    write_outcomes(conn, &outcomes)?;
    Ok(outcomes.len())
}

/// Re-verify a recorded `(worktree, commit)` against the live repository,
/// coalescing the git read across every segment — of any session — that recorded
/// that commit in that worktree (I2). This is the batch primitive the background
/// reverify job drains: one repository read per distinct branch, not per session
/// and not per segment.
pub fn reverify_commit(conn: &Connection, worktree_id: &str, commit_sha: &str) -> Result<usize> {
    let targets = commit_targets(conn, worktree_id, commit_sha)?;
    if targets.is_empty() {
        return Ok(0);
    }
    let outcomes = outcomes_for_commit(targets);
    write_outcomes(conn, &outcomes)?;
    Ok(outcomes.len())
}

/// Read git for one target without any cross-target coalescing.
fn outcome_for_target(target: ReverifyTarget) -> ReverifyOutcome {
    let path = Path::new(&target.path);
    if !path.exists() {
        // The checkout is gone: this is the one case that marks the worktree missing.
        return ReverifyOutcome::unavailable(target, "path_missing", WorktreeState::Missing);
    }
    match git::reverify(path, &target.commit_sha, target.branch.as_deref()) {
        Some(result) => ReverifyOutcome::verified(target, &result),
        // Present but unreadable — transient, so the worktree stands.
        None => {
            ReverifyOutcome::unavailable(target, "repository_unreadable", WorktreeState::Unchanged)
        }
    }
}

/// Read git once per distinct branch across a `(worktree, commit)` group. Every
/// target here shares one path and commit, so the only read variance is the
/// recorded branch; a cache turns N segments into one read per branch.
fn outcomes_for_commit(targets: Vec<ReverifyTarget>) -> Vec<ReverifyOutcome> {
    let mut cache: HashMap<Option<String>, Option<crate::git::Reverification>> = HashMap::new();
    targets
        .into_iter()
        .map(|target| {
            let path = Path::new(&target.path);
            if !path.exists() {
                return ReverifyOutcome::unavailable(
                    target,
                    "path_missing",
                    WorktreeState::Missing,
                );
            }
            let result = cache
                .entry(target.branch.clone())
                .or_insert_with(|| {
                    git::reverify(path, &target.commit_sha, target.branch.as_deref())
                })
                .clone();
            match result {
                Some(result) => ReverifyOutcome::verified(target, &result),
                None => ReverifyOutcome::unavailable(
                    target,
                    "repository_unreadable",
                    WorktreeState::Unchanged,
                ),
            }
        })
        .collect()
}

/// Persist a set of re-verification outcomes in one transaction. The verdict is
/// part of the id, so an unchanged conclusion collides with its own earlier row
/// (refreshing only `last_checked_at`) while a changed conclusion appends a new
/// observation — history is appended, never overwritten (`DATA_MODEL.md` §5,
/// `GIT_INTEGRATION.md` §6). `observed_at` therefore always means "when this
/// conclusion was *first* reached" and is never rewritten.
fn write_outcomes(conn: &Connection, outcomes: &[ReverifyOutcome]) -> Result<()> {
    if outcomes.is_empty() {
        return Ok(());
    }
    let _write = crate::storage::write_lock();
    let tx = conn.unchecked_transaction()?;
    for outcome in outcomes {
        let obs_id = det_id(
            "gr",
            &[
                &outcome.session_id,
                &outcome.segment_id,
                &outcome.commit_sha,
                &outcome.verdict,
            ],
        );
        tx.execute(
            "INSERT INTO git_observation
                (id, session_id, segment_id, source, observed_at, temporal_confidence,
                 branch, commit_sha, commit_exists, metadata_json)
             VALUES (?1, ?2, ?3, 'lore_reverified', unixepoch('now')*1000, 'retrospective',
                     ?4, ?5, ?6, ?7)
             ON CONFLICT(id) DO UPDATE SET
                metadata_json = excluded.metadata_json",
            params![
                obs_id,
                outcome.session_id,
                outcome.segment_id,
                outcome.branch,
                outcome.commit_sha,
                outcome.commit_exists,
                outcome.metadata,
            ],
        )?;
        // Re-verification maintains the worktree missing latch: a vanished
        // checkout marks it missing, a successfully re-checked checkout clears
        // any previous latch, and a transient unreadable state leaves it
        // untouched. Repository-level missingness is derived from these flags in
        // `query::list_repositories`, never stored as a second value (F15).
        match outcome.worktree_state {
            WorktreeState::Missing => {
                if let Some(worktree_id) = &outcome.worktree_id {
                    tx.execute(
                        "UPDATE worktree SET is_missing = 1 WHERE id = ?1",
                        params![worktree_id],
                    )?;
                }
            }
            WorktreeState::Present => {
                if let Some(worktree_id) = &outcome.worktree_id {
                    tx.execute(
                        "UPDATE worktree SET is_missing = 0 WHERE id = ?1",
                        params![worktree_id],
                    )?;
                }
            }
            WorktreeState::Unchanged => {}
        }
    }
    // New lore_reverified rows are searchable evidence too — refresh the git
    // filter projection for every session these outcomes touched, in the same
    // transaction (migration 0011).
    let mut projected: std::collections::BTreeSet<&str> = std::collections::BTreeSet::new();
    for outcome in outcomes {
        if projected.insert(outcome.session_id.as_str()) {
            crate::search::project_session_git(&tx, &outcome.session_id)?;
        }
    }
    tx.commit()?;
    Ok(())
}

struct ReverifyTarget {
    session_id: String,
    segment_id: String,
    worktree_id: Option<String>,
    path: String,
    branch: Option<String>,
    commit_sha: String,
}

struct ReverifyOutcome {
    session_id: String,
    segment_id: String,
    worktree_id: Option<String>,
    branch: Option<String>,
    commit_sha: String,
    commit_exists: Option<i64>,
    metadata: String,
    worktree_state: WorktreeState,
    /// Short stable fingerprint of *what this check concluded*. It is part of
    /// the observation id, so re-running with the same conclusion refreshes one
    /// row while a changed conclusion appends a new one (`DATA_MODEL.md` §5).
    verdict: String,
}

/// What a re-check concluded about the checkout itself, used to maintain the
/// `worktree.is_missing` latch without conflating "gone" with "transiently
/// unreadable" (F15/F1b). Repository-level missingness is derived from these
/// flags in `query::list_repositories`, never stored as a second value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WorktreeState {
    /// The checkout path is gone: mark the worktree missing.
    Missing,
    /// The checkout is present and was re-checked: clear any missing latch.
    Present,
    /// The checkout could not be read (permission, lock, unmounted volume):
    /// leave the latch untouched.
    Unchanged,
}

/// Render an optional flag for a verdict fingerprint: `1`, `0`, or `-` (unknown).
fn flag(value: Option<bool>) -> char {
    match value {
        Some(true) => '1',
        Some(false) => '0',
        None => '-',
    }
}

/// Wall-clock milliseconds since the Unix epoch, saturating at 0 before it.
fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
}

impl ReverifyOutcome {
    fn verified(target: ReverifyTarget, result: &crate::git::Reverification) -> Self {
        let metadata = serde_json::json!({
            "commit_exists": result.commit_exists,
            "branch_exists": result.branch_exists,
            "branch_at_recorded_commit": result.branch_at_recorded_commit,
            "last_checked_at": now_ms(),
        })
        .to_string();
        let verdict = format!(
            "v:{}:{}:{}",
            flag(Some(result.commit_exists)),
            flag(result.branch_exists),
            flag(result.branch_at_recorded_commit),
        );
        ReverifyOutcome {
            session_id: target.session_id,
            segment_id: target.segment_id,
            worktree_id: target.worktree_id,
            branch: target.branch,
            commit_sha: target.commit_sha,
            commit_exists: Some(i64::from(result.commit_exists)),
            metadata,
            worktree_state: WorktreeState::Present,
            verdict,
        }
    }

    /// The repository could not be checked. `Missing` is passed **only** when
    /// the checkout is genuinely gone: a repository that is present but
    /// unreadable (a permission error, a lock, an unmounted volume) is a
    /// transient condition (`Unchanged`) and must not flag a live worktree as
    /// missing.
    fn unavailable(target: ReverifyTarget, reason: &str, state: WorktreeState) -> Self {
        let metadata = serde_json::json!({
            "unavailable": reason,
            "last_checked_at": now_ms(),
        })
        .to_string();
        ReverifyOutcome {
            session_id: target.session_id,
            segment_id: target.segment_id,
            worktree_id: target.worktree_id,
            branch: target.branch,
            commit_sha: target.commit_sha,
            commit_exists: None,
            metadata,
            worktree_state: state,
            verdict: format!("u:{reason}"),
        }
    }
}

/// Segments carrying an agent-recorded commit, with the local path (worktree, or
/// the recorded cwd) to re-check it against.
fn reverify_targets(conn: &Connection, session_id: &str) -> Result<Vec<ReverifyTarget>> {
    let mut stmt = conn.prepare(
        "SELECT s.id, s.worktree_id, COALESCE(w.path, s.cwd), o.branch, o.commit_sha
         FROM session_segment s
         JOIN git_observation o
            ON o.segment_id = s.id AND o.source = 'agent_recorded' AND o.commit_sha IS NOT NULL
         LEFT JOIN worktree w ON w.id = s.worktree_id
         WHERE s.session_id = ?1 AND COALESCE(w.path, s.cwd) IS NOT NULL",
    )?;
    let rows = stmt
        .query_map([session_id], |row| {
            Ok(ReverifyTarget {
                session_id: session_id.to_string(),
                segment_id: row.get(0)?,
                worktree_id: row.get(1)?,
                path: row.get(2)?,
                branch: row.get(3)?,
                commit_sha: row.get(4)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

/// Every segment — across any session — whose agent-recorded evidence points at
/// one `(worktree, commit)`, with the local path to re-check it against. This is
/// the coalescing unit for the background reverify job (I2).
fn commit_targets(
    conn: &Connection,
    worktree_id: &str,
    commit_sha: &str,
) -> Result<Vec<ReverifyTarget>> {
    let mut stmt = conn.prepare(
        "SELECT o.session_id, o.segment_id, s.worktree_id, COALESCE(w.path, s.cwd),
                o.branch, o.commit_sha
         FROM git_observation o
         JOIN session_segment s ON s.id = o.segment_id
         LEFT JOIN worktree w ON w.id = s.worktree_id
         WHERE s.worktree_id = ?1 AND o.source = 'agent_recorded' AND o.commit_sha = ?2
           AND COALESCE(w.path, s.cwd) IS NOT NULL",
    )?;
    let rows = stmt
        .query_map(params![worktree_id, commit_sha], |row| {
            Ok(ReverifyTarget {
                session_id: row.get(0)?,
                segment_id: row.get(1)?,
                worktree_id: row.get(2)?,
                path: row.get(3)?,
                branch: row.get(4)?,
                commit_sha: row.get(5)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

/// Segments with a recorded cwd that are not yet linked to a repository.
fn unresolved_segments(conn: &Connection, session_id: &str) -> Result<Vec<(String, String)>> {
    let mut stmt = conn.prepare(
        "SELECT id, cwd FROM session_segment
         WHERE session_id = ?1 AND cwd IS NOT NULL AND repository_id IS NULL",
    )?;
    let rows = stmt
        .query_map([session_id], |row| Ok((row.get(0)?, row.get(1)?)))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

/// The resolved repository identity for a captured repo.
struct Identity {
    key: String,
    confidence: &'static str,
    display_name: String,
}

/// Choose the repository identity from the strongest available evidence
/// (`GIT_INTEGRATION.md` §2):
/// - normalized remote(s) **and** a known root set → `high`; two clones of one
///   upstream collide, while a fork (same root, different remote) does not;
/// - remote(s) only (e.g. shallow/truncated history) → `medium`, remote-keyed;
/// - no remote → key on the local common dir (`high` locally). Root-set alone is
///   recorded as evidence but never used as the identity key, so unrelated repos
///   that merely share a root commit are never merged.
fn resolve_identity(facts: &CapturedRepo, common_key: &str, workdir: &Path) -> Identity {
    let roots_known = !facts.root_commits.is_empty() && !facts.history_truncated;
    let remote_name = facts
        .remotes
        .first()
        .and_then(|r| r.rsplit('/').next())
        .map(str::to_string);
    let local_name = workdir
        .file_name()
        .map(|n| n.to_string_lossy().into_owned());

    if !facts.remotes.is_empty() && roots_known {
        let mut material = facts.remotes.clone();
        material.extend(facts.root_commits.iter().cloned());
        Identity {
            key: format!("rr:{}", fnv1a_hex(material.join("\n").as_bytes())),
            confidence: "high",
            display_name: remote_name.or(local_name).unwrap_or_default(),
        }
    } else if !facts.remotes.is_empty() {
        Identity {
            key: format!("r:{}", fnv1a_hex(facts.remotes.join("\n").as_bytes())),
            confidence: "medium",
            display_name: remote_name.or(local_name).unwrap_or_default(),
        }
    } else {
        Identity {
            key: format!("gcd:{common_key}"),
            confidence: "high",
            display_name: local_name.unwrap_or_else(|| format!("gcd:{common_key}")),
        }
    }
}

fn link_segment(
    tx: &Connection,
    session_id: &str,
    segment_id: &str,
    facts: &CapturedRepo,
) -> Result<()> {
    let common_key = fnv1a_hex(facts.common_dir.to_string_lossy().as_bytes());
    let workdir = facts
        .workdir
        .clone()
        .unwrap_or_else(|| facts.common_dir.clone());
    let workdir_str = workdir.to_string_lossy().into_owned();
    let identity = resolve_identity(facts, &common_key, &workdir);
    let repo_id = det_id("repo", &[&identity.key]);
    // The main worktree's common dir is its own `.git`; a linked worktree's is
    // the main repo's, so it differs from `<workdir>/.git`.
    let is_primary = i64::from(workdir.join(".git") == facts.common_dir);

    tx.execute(
        "INSERT INTO repository
            (id, identity_key, display_name, primary_path, identity_confidence,
             created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, unixepoch('now')*1000, unixepoch('now')*1000)
         ON CONFLICT(identity_key) DO UPDATE SET
            primary_path = COALESCE(repository.primary_path, excluded.primary_path),
            updated_at = unixepoch('now')*1000",
        params![
            repo_id,
            identity.key,
            identity.display_name,
            workdir_str,
            identity.confidence
        ],
    )?;

    let wt_id = det_id("wt", &[&repo_id, &workdir_str]);
    tx.execute(
        "INSERT INTO worktree
            (id, repository_id, path, git_common_dir_hash, branch_hint, is_primary, is_missing)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, 0)
         ON CONFLICT(id) DO UPDATE SET branch_hint = excluded.branch_hint, is_missing = 0",
        params![
            wt_id,
            repo_id,
            workdir_str,
            common_key,
            facts.branch,
            is_primary
        ],
    )?;

    record_evidence(tx, &repo_id, &common_key, facts)?;

    tx.execute(
        "UPDATE session_segment
         SET repository_id = ?2, worktree_id = ?3, resolution_confidence = ?4
         WHERE id = ?1",
        params![segment_id, repo_id, wt_id, identity.confidence],
    )?;

    // lore_captured observation: current repository state, never session-time.
    let changed_files_json = facts
        .changed_files
        .as_ref()
        .map(|files| serde_json::json!(files).to_string());
    let obs_id = det_id("gc", &[session_id, segment_id]);
    tx.execute(
        "INSERT INTO git_observation
            (id, session_id, segment_id, source, observed_at, temporal_confidence,
             branch, commit_sha, is_dirty, ahead, behind, changed_files_json, commit_subject)
         VALUES (?1, ?2, ?3, 'lore_captured', unixepoch('now')*1000, 'current_only',
                 ?4, ?5, ?6, ?7, ?8, ?9, ?10)
         ON CONFLICT(id) DO UPDATE SET
            observed_at = unixepoch('now')*1000, branch = excluded.branch,
            commit_sha = excluded.commit_sha, is_dirty = excluded.is_dirty,
            ahead = excluded.ahead, behind = excluded.behind,
            changed_files_json = excluded.changed_files_json,
            commit_subject = excluded.commit_subject",
        params![
            obs_id,
            session_id,
            segment_id,
            facts.branch,
            facts.head_commit,
            facts.is_dirty.map(i64::from),
            facts.ahead,
            facts.behind,
            changed_files_json,
            facts.commit_subject,
        ],
    )?;
    Ok(())
}

/// Record all identity evidence for a repository, keyed deterministically so
/// re-enrichment refreshes rather than duplicates. Evidence never overwrites
/// another kind; each is stored with its own confidence.
fn record_evidence(
    tx: &Connection,
    repo_id: &str,
    common_key: &str,
    facts: &CapturedRepo,
) -> Result<()> {
    // git_common_dir — high local grouping.
    upsert_evidence(tx, repo_id, "git_common_dir", common_key, None, "high")?;

    // Each normalized remote — medium; the credential-free value is safe to show.
    for remote in &facts.remotes {
        let value_hash = fnv1a_hex(remote.as_bytes());
        upsert_evidence(
            tx,
            repo_id,
            "remote",
            &value_hash,
            Some(remote.as_str()),
            "medium",
        )?;
    }

    // Root-commit set — low; recorded for candidate matching, never an auto-merge
    // key on its own. Skipped when the history walk was truncated.
    if !facts.root_commits.is_empty() && !facts.history_truncated {
        let value_hash = fnv1a_hex(facts.root_commits.join("\n").as_bytes());
        upsert_evidence(tx, repo_id, "root_set", &value_hash, None, "low")?;
    }
    Ok(())
}

fn upsert_evidence(
    tx: &Connection,
    repo_id: &str,
    kind: &str,
    value_hash: &str,
    display_value: Option<&str>,
    confidence: &str,
) -> Result<()> {
    let ev_id = det_id("ev", &[repo_id, kind, value_hash]);
    tx.execute(
        "INSERT INTO repository_identity_evidence
            (id, repository_id, kind, value_hash, display_value, confidence,
             first_seen_at, last_seen_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, unixepoch('now')*1000, unixepoch('now')*1000)
         ON CONFLICT(id) DO UPDATE SET last_seen_at = unixepoch('now')*1000",
        params![ev_id, repo_id, kind, value_hash, display_value, confidence],
    )?;
    Ok(())
}

/// FNV-1a 64-bit hex digest (a content fingerprint, not a security primitive).
fn fnv1a_hex(bytes: &[u8]) -> String {
    const OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x0000_0100_0000_01b3;
    let mut hash = OFFSET;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(PRIME);
    }
    format!("{hash:016x}")
}

// Full enrichment is covered end-to-end by `tests/enrich.rs`, which builds
// fixture repositories; the identity-key derivation is unit-tested here.
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_key_is_stable_for_a_common_dir() {
        let a = fnv1a_hex(b"/repos/x/.git");
        let b = fnv1a_hex(b"/repos/x/.git");
        let c = fnv1a_hex(b"/repos/y/.git");
        assert_eq!(a, b);
        assert_ne!(a, c);
        assert_eq!(a.len(), 16);
    }
}
