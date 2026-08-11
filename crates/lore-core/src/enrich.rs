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
//! Ambiguous remote/root-only matching, cross-repository merge/split, and later
//! re-verification are deliberately out of scope here and remain separate
//! observations. Capture (filesystem I/O) runs before the write transaction
//! opens; all row writes commit atomically.

use std::collections::HashMap;
use std::path::Path;

use rusqlite::{params, Connection};

use crate::git::{self, CapturedRepo};
use crate::ingest::det_id;
use crate::storage::Result;

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

    let tx = conn.unchecked_transaction()?;
    let mut enriched = 0;
    for (segment_id, facts) in &resolved {
        link_segment(&tx, session_id, segment_id, facts)?;
        enriched += 1;
    }
    tx.commit()?;
    Ok(enriched)
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
    let obs_id = det_id("gc", &[session_id, segment_id]);
    tx.execute(
        "INSERT INTO git_observation
            (id, session_id, segment_id, source, observed_at, temporal_confidence,
             branch, commit_sha, is_dirty)
         VALUES (?1, ?2, ?3, 'lore_captured', unixepoch('now')*1000, 'current_only',
                 ?4, ?5, ?6)
         ON CONFLICT(id) DO UPDATE SET
            observed_at = unixepoch('now')*1000, branch = excluded.branch,
            commit_sha = excluded.commit_sha, is_dirty = excluded.is_dirty",
        params![
            obs_id,
            session_id,
            segment_id,
            facts.branch,
            facts.head_commit,
            facts.is_dirty.map(i64::from),
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
