//! Deterministic synthetic agent-profile generator for tests and manual QA.
//!
//! This writes a fake `~/.claude` / `~/.codex` layout under a caller-provided
//! directory so scale, lifecycle, and end-to-end tests never read a developer's
//! real coding history (the hard rule in `docs/development/TESTING.md` §8). It is
//! **write-only into the target directory** and performs no discovery of, or
//! access to, any real agent root.
//!
//! Output is fully deterministic: identical `ProfileSpec` (including `seed`)
//! produces byte-identical files. No clock or RNG is used — per-session
//! variation is derived from the seed and the session index with a small FNV
//! mix, and all timestamps are fixed. The generated JSONL mirrors the shapes the
//! Claude and Codex adapters parse (see `docs/agents/*`), with unique session
//! ids and message ids so every file ingests as a distinct session.

use std::io::Write;
use std::path::{Path, PathBuf};

use crate::adapters::DiscoveryRoots;
use crate::discovery::DiscoveryConfig;

/// What to generate. Small by default; scale tests scale the counts up.
#[derive(Debug, Clone, Copy)]
pub struct ProfileSpec {
    /// Number of Claude Code sessions to write.
    pub claude_sessions: usize,
    /// Number of Codex sessions to write.
    pub codex_sessions: usize,
    /// Upper bound on *extra* user+assistant turns appended beyond the two-line
    /// minimum, distributed deterministically so most sessions are small and a
    /// few are large (roughly every 500th session gets a much larger body).
    pub max_extra_turns: usize,
    /// Seed for deterministic per-session variation.
    pub seed: u64,
}

impl Default for ProfileSpec {
    fn default() -> Self {
        Self {
            claude_sessions: 8,
            codex_sessions: 8,
            max_extra_turns: 4,
            seed: 0x5eed,
        }
    }
}

/// Where a generated profile lives and how much it contains.
#[derive(Debug, Clone)]
pub struct SyntheticProfile {
    pub claude_root: PathBuf,
    pub codex_root: PathBuf,
    pub claude_files: usize,
    pub codex_files: usize,
    /// Total normalized messages written across all sessions (exact).
    pub message_count: usize,
}

impl SyntheticProfile {
    /// A [`DiscoveryConfig`] whose Claude/Codex roots point at this profile —
    /// never at the real agent roots.
    #[must_use]
    pub fn discovery_config(&self) -> DiscoveryConfig {
        let mut config = DiscoveryConfig::new();
        config.set_roots(
            "claude-code",
            DiscoveryRoots::new(vec![self.claude_root.clone()]),
        );
        config.set_roots("codex", DiscoveryRoots::new(vec![self.codex_root.clone()]));
        config
    }
}

/// Write a synthetic profile under `dir`, creating `claude/projects` and
/// `codex/sessions` subtrees. Returns the exact layout and message tally.
pub fn generate(dir: &Path, spec: &ProfileSpec) -> std::io::Result<SyntheticProfile> {
    let claude_root = dir.join("claude/projects");
    let codex_root = dir.join("codex/sessions");
    let mut message_count = 0;

    for i in 0..spec.claude_sessions {
        let extra = extra_turns(spec, i);
        // Spread across a handful of encoded-cwd project dirs.
        let project = claude_root.join(format!("project-{:02}", i % 16));
        std::fs::create_dir_all(&project)?;
        let uuid = uuid_for(0xC, i, 0);
        let file = project.join(format!("{uuid}.jsonl"));
        message_count += write_claude_session(&file, i, extra)?;
    }

    for i in 0..spec.codex_sessions {
        let extra = extra_turns(spec, i);
        // Spread across day directories, mirroring `~/.codex/sessions/YYYY/MM/DD`.
        let day = 1 + (i % 28);
        let day_dir = codex_root.join(format!("2026/08/{day:02}"));
        std::fs::create_dir_all(&day_dir)?;
        let file = day_dir.join(format!("rollout-{:012x}.jsonl", session_key(0x0, i)));
        message_count += write_codex_session(&file, i, extra)?;
    }

    Ok(SyntheticProfile {
        claude_root,
        codex_root,
        claude_files: spec.claude_sessions,
        codex_files: spec.codex_sessions,
        message_count,
    })
}

/// Deterministic extra-turn count for session `i`: a small spread for most,
/// with roughly every 500th session inflated to exercise a large file.
fn extra_turns(spec: &ProfileSpec, i: usize) -> usize {
    if spec.max_extra_turns == 0 {
        return 0;
    }
    let base = (mix(spec.seed, i as u64) as usize) % (spec.max_extra_turns + 1);
    if i % 500 == 499 {
        base + spec.max_extra_turns * 20
    } else {
        base
    }
}

/// Write one Claude session file. Returns the number of messages written
/// (2 base + 2 per extra turn). The trailing `custom-title` line is metadata,
/// not a message, matching the fixture accounting.
fn write_claude_session(path: &Path, i: usize, extra: usize) -> std::io::Result<usize> {
    let session_id = format!("aaaaaaaa-0000-4000-8000-{:012x}", session_key(0xC, i));
    let mut buf = Vec::new();

    let user_uuid = uuid_for(0xC, i, 0);
    writeln!(
        buf,
        "{}",
        serde_json::json!({
            "type": "user", "uuid": user_uuid, "parentUuid": null,
            "sessionId": session_id, "timestamp": "2026-08-10T10:00:00.000Z",
            "cwd": "/repo/app", "gitBranch": "main", "version": "1.2.3",
            "isSidechain": false, "userType": "external",
            "message": {"role": "user", "content": "add a health check endpoint"}
        })
    )?;

    let asst_uuid = uuid_for(0xC, i, 1);
    writeln!(
        buf,
        "{}",
        serde_json::json!({
            "type": "assistant", "uuid": asst_uuid, "parentUuid": user_uuid,
            "sessionId": session_id, "timestamp": "2026-08-10T10:00:05.000Z",
            "cwd": "/repo/app", "gitBranch": "main", "version": "1.2.3",
            "message": {"role": "assistant", "model": "claude-x",
                "content": [{"type": "text", "text": "I'll add GET /healthz returning 200."}]}
        })
    )?;

    let mut prev = asst_uuid;
    let mut messages = 2;
    let mut line = 2;
    for _ in 0..extra {
        let u = uuid_for(0xC, i, line);
        writeln!(
            buf,
            "{}",
            serde_json::json!({
                "type": "user", "uuid": u, "parentUuid": prev,
                "sessionId": session_id, "timestamp": "2026-08-10T10:01:00.000Z",
                "cwd": "/repo/app", "gitBranch": "main", "version": "1.2.3",
                "isSidechain": false, "userType": "external",
                "message": {"role": "user", "content": "also add readiness"}
            })
        )?;
        let a = uuid_for(0xC, i, line + 1);
        writeln!(
            buf,
            "{}",
            serde_json::json!({
                "type": "assistant", "uuid": a, "parentUuid": u,
                "sessionId": session_id, "timestamp": "2026-08-10T10:01:05.000Z",
                "cwd": "/repo/app", "gitBranch": "main", "version": "1.2.3",
                "message": {"role": "assistant", "model": "claude-x",
                    "content": [{"type": "text", "text": "Added GET /readyz as well."}]}
            })
        )?;
        prev = a;
        line += 2;
        messages += 2;
    }

    writeln!(
        buf,
        "{}",
        serde_json::json!({
            "type": "custom-title", "customTitle": "Add health check endpoint",
            "sessionId": session_id
        })
    )?;
    std::fs::write(path, buf)?;
    Ok(messages)
}

/// Write one Codex rollout file. Returns the number of messages written
/// (3 base + 2 per extra turn).
fn write_codex_session(path: &Path, i: usize, extra: usize) -> std::io::Result<usize> {
    let id = format!("019e0000-0000-7000-8000-{:012x}", session_key(0x0, i));
    let mut buf = Vec::new();

    writeln!(
        buf,
        "{}",
        serde_json::json!({
            "type": "session_meta", "timestamp": "2026-08-11T10:00:00.000Z",
            "payload": {"id": id, "cwd": "/proj", "cli_version": "0.133.0",
                "source": "cli", "model_provider": "openai",
                "git": {"branch": "main", "commit_hash": "3ab9f1",
                    "repository_url": "github.com/x/proj"}}
        })
    )?;
    writeln!(
        buf,
        "{}",
        serde_json::json!({
            "type": "turn_context", "timestamp": "2026-08-11T10:00:00.500Z",
            "payload": {"cwd": "/proj", "model": "gpt-x", "effort": "medium", "turn_id": "t1"}
        })
    )?;
    writeln!(
        buf,
        "{}",
        serde_json::json!({
            "type": "response_item", "timestamp": "2026-08-11T10:00:01.000Z",
            "payload": {"type": "message", "role": "user",
                "content": [{"type": "input_text", "text": "add a retry to the client"}]}
        })
    )?;
    writeln!(
        buf,
        "{}",
        serde_json::json!({
            "type": "response_item", "timestamp": "2026-08-11T10:00:02.000Z",
            "payload": {"type": "reasoning", "summary": "consider backoff",
                "content": [{"type": "reasoning_text", "text": "use exponential backoff"}]}
        })
    )?;
    writeln!(
        buf,
        "{}",
        serde_json::json!({
            "type": "response_item", "timestamp": "2026-08-11T10:00:03.000Z",
            "payload": {"type": "message", "role": "assistant",
                "content": [{"type": "output_text", "text": "I'll add exponential backoff."}]}
        })
    )?;

    let mut messages = 3;
    for t in 0..extra {
        writeln!(
            buf,
            "{}",
            serde_json::json!({
                "type": "response_item", "timestamp": "2026-08-11T10:00:05.000Z",
                "payload": {"type": "message", "role": "user",
                    "content": [{"type": "input_text", "text": "also handle timeouts"}]}
            })
        )?;
        writeln!(
            buf,
            "{}",
            serde_json::json!({
                "type": "response_item", "timestamp": "2026-08-11T10:00:06.000Z",
                "payload": {"type": "message", "role": "assistant",
                    "content": [{"type": "output_text", "text": "Added a timeout guard."}]}
            })
        )?;
        let _ = t;
        messages += 2;
    }

    writeln!(
        buf,
        "{}",
        serde_json::json!({
            "type": "event_msg", "timestamp": "2026-08-11T10:00:09.000Z",
            "payload": {"type": "task_complete", "last_agent_message": "done",
                "duration_ms": 9000, "turn_id": "t1"}
        })
    )?;
    std::fs::write(path, buf)?;
    Ok(messages)
}

/// Stable, unique-looking UUID for session `i`, line `line` in namespace `ns`.
fn uuid_for(ns: u8, i: usize, line: usize) -> String {
    format!(
        "{:02x}{:06x}-0000-4000-8000-{:012x}",
        ns,
        i & 0xff_ffff,
        (session_key(ns, i) ^ (line as u64).wrapping_mul(0x9e37)) & 0xffff_ffff_ffff
    )
}

/// Deterministic per-session key in namespace `ns` (keeps Claude/Codex ids
/// disjoint and unique across the whole profile).
fn session_key(ns: u8, i: usize) -> u64 {
    mix(u64::from(ns).wrapping_add(0x100), i as u64) & 0xffff_ffff_ffff
}

/// FNV-1a style mix — deterministic, no clock or RNG.
fn mix(seed: u64, i: u64) -> u64 {
    let mut h = 0xcbf2_9ce4_8422_2325 ^ seed;
    for byte in i.to_le_bytes() {
        h ^= u64::from(byte);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generation_is_deterministic_for_a_seed() {
        let spec = ProfileSpec {
            claude_sessions: 5,
            codex_sessions: 5,
            max_extra_turns: 3,
            seed: 42,
        };
        let a = tempfile::tempdir().unwrap();
        let b = tempfile::tempdir().unwrap();
        let pa = generate(a.path(), &spec).unwrap();
        let pb = generate(b.path(), &spec).unwrap();
        assert_eq!(pa.message_count, pb.message_count);
        assert_eq!(pa.claude_files, 5);
        assert_eq!(pa.codex_files, 5);

        // A representative file is byte-identical across runs.
        let name = std::fs::read_dir(pa.claude_root.join("project-00"))
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .file_name();
        let bytes_a = std::fs::read(pa.claude_root.join("project-00").join(&name)).unwrap();
        let bytes_b = std::fs::read(pb.claude_root.join("project-00").join(&name)).unwrap();
        assert_eq!(bytes_a, bytes_b, "same seed must produce identical bytes");
    }

    #[test]
    fn synthetic_profile_spec_defaults_and_zero_extra_turns() {
        let spec_default = ProfileSpec::default();
        assert_eq!(spec_default.claude_sessions, 8);
        assert_eq!(spec_default.codex_sessions, 8);
        assert_eq!(spec_default.max_extra_turns, 4);

        let spec_zero = ProfileSpec {
            claude_sessions: 2,
            codex_sessions: 2,
            max_extra_turns: 0,
            seed: 123,
        };
        let dir = tempfile::tempdir().unwrap();
        let profile = generate(dir.path(), &spec_zero).unwrap();
        assert_eq!(profile.claude_files, 2);
        assert_eq!(profile.codex_files, 2);
        // Each Claude file has 2 messages, each Codex file has 3 messages -> 2*2 + 2*3 = 10
        assert_eq!(profile.message_count, 10);

        let config = profile.discovery_config();
        assert_eq!(config.roots_for("claude-code").roots.len(), 1);
        assert_eq!(config.roots_for("codex").roots.len(), 1);
    }

    #[test]
    fn synthetic_generate_empty_sessions_and_profile_clones() {
        let spec_empty = ProfileSpec {
            claude_sessions: 0,
            codex_sessions: 0,
            max_extra_turns: 0,
            seed: 0,
        };
        let spec_copy = spec_empty;
        assert_eq!(spec_empty.claude_sessions, spec_copy.claude_sessions);

        let dir = tempfile::tempdir().unwrap();
        let profile = generate(dir.path(), &spec_empty).unwrap();
        assert_eq!(profile.claude_files, 0);
        assert_eq!(profile.codex_files, 0);
        assert_eq!(profile.message_count, 0);

        let profile_clone = profile.clone();
        assert_eq!(profile.claude_root, profile_clone.claude_root);
        assert_eq!(profile.codex_root, profile_clone.codex_root);
    }

    #[test]
    fn synthetic_generate_single_agent_variations() {
        let dir = tempfile::tempdir().unwrap();
        let spec_claude_only = ProfileSpec {
            claude_sessions: 3,
            codex_sessions: 0,
            max_extra_turns: 0,
            seed: 42,
        };
        let profile_claude = generate(&dir.path().join("claude_only"), &spec_claude_only).unwrap();
        assert_eq!(profile_claude.claude_files, 3);
        assert_eq!(profile_claude.codex_files, 0);
        assert_eq!(profile_claude.message_count, 6);

        let spec_codex_only = ProfileSpec {
            claude_sessions: 0,
            codex_sessions: 3,
            max_extra_turns: 0,
            seed: 42,
        };
        let profile_codex = generate(&dir.path().join("codex_only"), &spec_codex_only).unwrap();
        assert_eq!(profile_codex.claude_files, 0);
        assert_eq!(profile_codex.codex_files, 3);
        assert_eq!(profile_codex.message_count, 9);
    }
}
