//! M6 scale acceptance: FTS query latency at ~1M messages (`SEARCH.md` §6).
//!
//! This is the reproducible harness behind the committed reference performance
//! report. It generates a deterministic synthetic profile large enough to reach
//! the target message count, ingests it through the real worker pipeline, then
//! times representative query classes — warm (steady-state), cold (fresh SQLite
//! connection), adversarial (very common terms / multi-term / filtered), and a
//! deep keyset page — and prints a structured report with the corpus size and
//! SQLite version.
//!
//! Everything is synthetic and local (`lore_core::synthetic`); no real
//! `~/.claude` / `~/.codex` is ever read (`docs/development/TESTING.md` §8).
//!
//! Heavy and `#[ignore]`d — building a 1M-message corpus takes minutes. Run it
//! explicitly for scale validation:
//!
//! ```text
//! cargo test -p lore-core --release --test search_scale -- --ignored --nocapture
//! ```
//!
//! Knobs (env):
//! * `LORE_SCALE_MSGS`   — target message count (default 1_000_000).
//! * `LORE_SCALE_REPORT` — path to also write the Markdown report to.
//!
//! The test asserts only a *generous* sanity ceiling (p95 < 1s) so it fails
//! loudly on a super-linear regression without being flaky about the 200 ms
//! target, which is hardware-specific and lives in the committed report.
//!
//! # Reference result (M6 acceptance)
//!
//! Apple M4 Pro (14 cores), 24 GB, macOS 15.7.3, SQLite 3.46.0. Corpus: 9,804
//! sessions / 1,076,658 messages / 1,076,658 search documents.
//!
//! | Query class | p50 | p95 |
//! |---|---:|---:|
//! | warm (steady state) | 12.0 ms | 16.9 ms |
//! | cold (fresh connection) | 43.3 ms | 232.6 ms |
//! | common term `add` (ranks ~all docs) | 168.9 ms | 174.7 ms |
//! | multi-term AND | 18.0 ms | 18.9 ms |
//! | agent filter | 108.7 ms | 122.8 ms |
//! | deep keyset (page 20, match-everything term) | — | 202.6 ms |
//!
//! Verdict: interactive queries are ~17 ms at 1M messages (10× under target) and
//! even a match-everything term stays under 200 ms — FTS5 is sufficient for V0,
//! no `sqlite-vec` needed (ADR-0004 holds). The two edges that graze 200 ms are
//! the very first cold query (p50 43 ms) and deep-paging a term that matches the
//! whole corpus; both are extreme, non-typical cases with documented optional
//! mitigations. Ingest ran ~2,137 msgs/s (per-message cost rises with index
//! size); that is a background one-time cost, not the query target.
#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::io::Write as _;
use std::time::{Duration, Instant};

use lore_core::adapters::AdapterRegistry;
use lore_core::search::{search, search_page};
use lore_core::storage::blob::BlobStore;
use lore_core::synthetic::{generate, ProfileSpec};
use lore_core::worker::{open_worker, WorkerConfig};
use rusqlite::Connection;

/// Average messages per generated session is ~`2 + 2*(max_extra_turns/2)` for
/// Claude and one more for Codex; at `max_extra_turns = 100` that is ~102, so
/// this many sessions per agent reaches ~1M messages. The generator returns the
/// exact tally, which the harness asserts against the target.
const MAX_EXTRA_TURNS: usize = 100;

fn count(conn: &Connection, sql: &str) -> i64 {
    conn.query_row(sql, [], |r| r.get(0)).unwrap()
}

/// p50/p95/max over a set of samples (sorted copy).
struct Stats {
    p50: Duration,
    p95: Duration,
    max: Duration,
    n: usize,
}

fn stats(mut samples: Vec<Duration>) -> Stats {
    assert!(!samples.is_empty(), "need at least one sample");
    samples.sort_unstable();
    let pick = |q: f64| samples[((samples.len() as f64 * q) as usize).min(samples.len() - 1)];
    Stats {
        p50: pick(0.50),
        p95: pick(0.95),
        max: *samples.last().unwrap(),
        n: samples.len(),
    }
}

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}

#[test]
#[ignore = "heavy: builds a ~1M-message corpus; run explicitly for scale validation"]
fn fts_query_latency_at_one_million_messages() {
    let target: usize = std::env::var("LORE_SCALE_MSGS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(1_000_000);

    // Size the profile so the exact generated tally clears the target.
    let per_agent = target.div_ceil(2 * (2 + MAX_EXTRA_TURNS));
    let spec = ProfileSpec {
        claude_sessions: per_agent,
        codex_sessions: per_agent,
        max_extra_turns: MAX_EXTRA_TURNS,
        seed: 2026,
    };

    let home = tempfile::tempdir().unwrap();
    let blob_dir = tempfile::tempdir().unwrap();
    let db = tempfile::tempdir().unwrap();
    let db_path = db.path().join("lore.db");

    let gen_start = Instant::now();
    let profile = generate(home.path(), &spec).unwrap();
    let gen_elapsed = gen_start.elapsed();
    let sessions = profile.claude_files + profile.codex_files;
    eprintln!(
        "generated {sessions} sessions / {} messages in {:.1}s",
        profile.message_count,
        gen_elapsed.as_secs_f64()
    );

    let worker = open_worker(
        &db_path,
        AdapterRegistry::v0(),
        BlobStore::open(blob_dir.path()).unwrap(),
        profile.discovery_config(),
        WorkerConfig {
            drain_batch: 500,
            ..WorkerConfig::default()
        },
    )
    .unwrap();

    let ingest_start = Instant::now();
    let summary = worker.scan(&lore_core::pipeline::NullSink).unwrap();
    let ingest_elapsed = ingest_start.elapsed();
    assert_eq!(summary.ingested, sessions, "every session ingests");
    assert_eq!(summary.failed, 0);

    let conn = lore_core::storage::open(&db_path).unwrap();
    let messages = count(&conn, "SELECT count(*) FROM message");
    let docs = count(&conn, "SELECT count(*) FROM search_document");
    let sqlite_version: String = conn
        .query_row("SELECT sqlite_version()", [], |r| r.get(0))
        .unwrap();
    assert!(
        messages as usize >= target,
        "corpus ({messages} messages) must reach the {target} target"
    );
    eprintln!(
        "ingested in {:.1}s → {messages} messages, {docs} search documents",
        ingest_elapsed.as_secs_f64()
    );

    // Query classes exercised. Terms are drawn from the synthetic corpus so they
    // actually match. `add` is the adversarial common term (in nearly every
    // session); the filtered and multi-term cases probe the join/keyset paths.
    let warm_query = "backoff";
    let cold_queries = [
        "health",
        "readiness",
        "timeout",
        "endpoint",
        "retry",
        "exponential",
        "guard",
        "check",
    ];
    let adversarial: &[(&str, &str)] = &[
        ("very common term", "add"),
        ("multi-term AND", "add health check"),
        ("agent filter", "add agent:codex"),
        ("path filter", "add path:/repo"),
    ];

    // WARM: steady state on one hot connection; drop the first (compile) run.
    let mut warm = Vec::new();
    for i in 0..25 {
        let t = Instant::now();
        let hits = search(&conn, warm_query, 50).unwrap();
        let d = t.elapsed();
        assert!(!hits.is_empty(), "warm query should match");
        if i > 0 {
            warm.push(d);
        }
    }
    let warm = stats(warm);

    // COLD: a fresh connection per query so SQLite's own page cache starts empty.
    let mut cold = Vec::new();
    for q in cold_queries {
        let fresh = lore_core::storage::open(&db_path).unwrap();
        let t = Instant::now();
        let _ = search(&fresh, q, 50).unwrap();
        cold.push(t.elapsed());
    }
    let cold = stats(cold);

    // ADVERSARIAL: measured on the hot connection (worst case is the work, not
    // the cache). Each runs a few times.
    let mut adv_rows = Vec::new();
    for (label, q) in adversarial {
        let mut samples = Vec::new();
        for _ in 0..5 {
            let t = Instant::now();
            let hits = search(&conn, q, 50).unwrap();
            samples.push((t.elapsed(), hits.len()));
        }
        let n_hits = samples[0].1;
        let s = stats(samples.into_iter().map(|(d, _)| d).collect());
        adv_rows.push((*label, *q, s, n_hits));
    }

    // DEEP KEYSET PAGE: walk 20 pages of 50 into the common-term result set and
    // time the last page — keyset must not degrade with depth.
    let mut cursor = None;
    let mut deep = Duration::ZERO;
    for _ in 0..20 {
        let t = Instant::now();
        let page = search_page(&conn, "add", 50, cursor.as_deref()).unwrap();
        deep = t.elapsed();
        match page.next_cursor {
            Some(c) => cursor = Some(c),
            None => break,
        }
    }

    let report = build_report(BuildReport {
        sessions,
        messages,
        docs,
        sqlite_version: &sqlite_version,
        gen_elapsed,
        ingest_elapsed,
        warm: &warm,
        cold: &cold,
        adversarial: &adv_rows,
        deep,
    });
    eprintln!("\n{report}");
    if let Ok(path) = std::env::var("LORE_SCALE_REPORT") {
        let mut f = std::fs::File::create(&path).unwrap();
        f.write_all(report.as_bytes()).unwrap();
        eprintln!("wrote report to {path}");
    }

    // Generous sanity ceiling — catches a super-linear regression without
    // pinning the hardware-specific 200 ms target.
    assert!(
        warm.p95 < Duration::from_secs(1),
        "warm p95 {:.1}ms exceeded the 1s sanity ceiling",
        ms(warm.p95)
    );
}

struct BuildReport<'a> {
    sessions: usize,
    messages: i64,
    docs: i64,
    sqlite_version: &'a str,
    gen_elapsed: Duration,
    ingest_elapsed: Duration,
    warm: &'a Stats,
    cold: &'a Stats,
    adversarial: &'a [(&'a str, &'a str, Stats, usize)],
    deep: Duration,
}

fn build_report(r: BuildReport<'_>) -> String {
    let mut s = String::new();
    let target_met = |p95: Duration| {
        if p95 < Duration::from_millis(200) {
            "✅"
        } else {
            "⚠️"
        }
    };
    s.push_str("# Search latency reference report (M6)\n\n");
    s.push_str(
        "> Generated by `cargo test -p lore-core --release --test search_scale -- --ignored`.\n",
    );
    s.push_str("> Fill in the hardware line before committing as a reference.\n\n");
    s.push_str("- Hardware: TODO (fill in: chip, RAM, disk)\n");
    s.push_str(&format!("- SQLite: {}\n", r.sqlite_version));
    s.push_str(&format!(
        "- Corpus: {} sessions, {} messages, {} search documents\n",
        r.sessions, r.messages, r.docs
    ));
    s.push_str(&format!(
        "- Generate: {:.1}s · Ingest: {:.1}s ({:.0} msgs/s)\n\n",
        r.gen_elapsed.as_secs_f64(),
        r.ingest_elapsed.as_secs_f64(),
        r.messages as f64 / r.ingest_elapsed.as_secs_f64().max(0.001),
    ));
    s.push_str("Target: typical query < 200 ms.\n\n");
    s.push_str("| Query class | n | p50 (ms) | p95 (ms) | max (ms) | <200ms |\n");
    s.push_str("|---|---:|---:|---:|---:|:--:|\n");
    s.push_str(&format!(
        "| warm (steady state) | {} | {:.2} | {:.2} | {:.2} | {} |\n",
        r.warm.n,
        ms(r.warm.p50),
        ms(r.warm.p95),
        ms(r.warm.max),
        target_met(r.warm.p95)
    ));
    s.push_str(&format!(
        "| cold (fresh connection) | {} | {:.2} | {:.2} | {:.2} | {} |\n",
        r.cold.n,
        ms(r.cold.p50),
        ms(r.cold.p95),
        ms(r.cold.max),
        target_met(r.cold.p95)
    ));
    for (label, q, st, hits) in r.adversarial {
        s.push_str(&format!(
            "| adversarial: {} (`{}`, {} hits) | {} | {:.2} | {:.2} | {:.2} | {} |\n",
            label,
            q,
            hits,
            st.n,
            ms(st.p50),
            ms(st.p95),
            ms(st.max),
            target_met(st.p95)
        ));
    }
    s.push_str(&format!(
        "| deep keyset (page 20) | 1 | — | — | {:.2} | {} |\n",
        ms(r.deep),
        target_met(r.deep)
    ));
    s
}
