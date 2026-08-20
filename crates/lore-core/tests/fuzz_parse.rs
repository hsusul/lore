//! Deterministic adversarial-input coverage for the tolerant-parsing contract
//! (`AGENTS.md` §5, `TESTING.md` §5): the adapters must never panic on hostile
//! input and must return an internally consistent `ParsedSession`, and the
//! secret scanner/redactor must never panic on the resulting text. This is the
//! seeded-generator stand-in until `proptest`/`cargo-fuzz` are adopted.
#![allow(clippy::expect_used, clippy::unwrap_used)]

use lore_core::adapters::claude_code::ClaudeCodeAdapter;
use lore_core::adapters::codex::CodexAdapter;
use lore_core::model::ParsedSession;

/// Tiny deterministic PRNG (xorshift64*) so failures are reproducible from the
/// seed alone — no `rand` dependency, no wall-clock nondeterminism.
struct Rng(u64);

impl Rng {
    fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next_u64() % n as u64) as usize
    }
    fn pick<'a>(&mut self, xs: &[&'a str]) -> &'a str {
        xs[self.below(xs.len())]
    }
}

/// Fragments chosen to stress JSON structure, the event/type dispatch of both
/// adapters, UTF-8 boundaries, control bytes, secret anchors, and size.
fn fragments() -> Vec<&'static str> {
    vec![
        "{",
        "}",
        "[",
        "]",
        ":",
        ",",
        "\"",
        "\\",
        "\n",
        "\r\n",
        "\t",
        " ",
        "\0",
        "\"type\"",
        "\"user\"",
        "\"assistant\"",
        "\"system\"",
        "\"summary\"",
        "\"message\"",
        "\"content\"",
        "\"role\"",
        "\"sessionId\"",
        "\"cwd\"",
        "\"uuid\"",
        "\"gitBranch\"",
        "\"tool_use\"",
        "\"tool_result\"",
        "\"thinking\"",
        "\"custom-title\"",
        "\"session_meta\"",
        "\"response_item\"",
        "\"payload\"",
        "\"call_id\"",
        "\"status\"",
        "\"timestamp\"",
        "\"encrypted_content\"",
        "\"id\"",
        "\"parent_uuid\"",
        "true",
        "false",
        "null",
        "0",
        "-1",
        "1e999",
        "/repo/src",
        "é",
        "🔑",
        "\u{202e}",
        "café",
        "ghp_",
        "-----BEGIN ",
        "sk-ant-",
        "AKIA",
        "postgres://u:p@h/d",
        " token=",
    ]
}

/// Build one adversarial document from `k` fragments plus occasional long runs.
fn build_doc(rng: &mut Rng, frags: &[&str], k: usize) -> String {
    let mut doc = String::new();
    for _ in 0..k {
        if rng.below(40) == 0 {
            // Occasional huge run to exercise size handling and truncation.
            let unit = rng.pick(frags);
            for _ in 0..rng.below(500) {
                doc.push_str(unit);
            }
        } else {
            doc.push_str(rng.pick(frags));
        }
    }
    doc
}

/// Every message must reference a real segment, and any session with messages
/// must have at least one segment. A violation would silently drop context (or,
/// before hardening, index out of bounds).
fn assert_consistent(parsed: &ParsedSession) {
    if !parsed.messages.is_empty() {
        assert!(
            !parsed.segments.is_empty(),
            "messages present but no segment assigned"
        );
    }
    let mut prev_seq: Option<i64> = None;
    for message in &parsed.messages {
        assert!(message.seq >= 0, "message seq must be non-negative");
        if let Some(prev) = prev_seq {
            assert!(
                message.seq > prev,
                "message seq must strictly increase: {} <= {}",
                message.seq,
                prev
            );
        }
        prev_seq = Some(message.seq);
        assert!(
            message.segment_ix < parsed.segments.len(),
            "segment_ix {} out of bounds ({} segments)",
            message.segment_ix,
            parsed.segments.len()
        );
        for part in &message.parts {
            assert!(part.ordinal >= 0, "part ordinal must be non-negative");
        }
    }
    for segment in &parsed.segments {
        assert!(
            segment.seq_start <= segment.seq_end,
            "segment seq_start {} exceeds seq_end {}",
            segment.seq_start,
            segment.seq_end
        );
    }
    // Tool-call refs point at (seq, ordinal) coordinates that must be plausible.
    for call in &parsed.tool_calls {
        assert!(call.call_ref.0 >= 0 && call.call_ref.1 >= 0);
    }
}

#[test]
fn adapters_and_secret_scan_never_panic_on_adversarial_input() {
    let frags = fragments();
    let claude = ClaudeCodeAdapter::new();
    let codex = CodexAdapter::new();
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);

    for i in 0..4000u32 {
        let k = 1 + rng.below(60);
        let doc = build_doc(&mut rng, &frags, k);

        // Both adapters must tolerate it (no panic) and stay consistent.
        let a = claude.parse_str(&doc, "fuzz");
        assert_consistent(&a);
        let b = codex.parse_str(&doc, "fuzz");
        assert_consistent(&b);

        // The scanner is fallible-by-contract; whatever it returns, redacting the
        // same text with those findings must not panic (the hardened invariant).
        if let Ok(findings) = lore_core::secrets::scan(&doc) {
            let redacted = lore_core::secrets::redact(&doc, &findings);
            // Redaction preserves surrounding text and never emits a raw finding
            // byte-slice boundary panic; a trivially-checkable postcondition is
            // that the output is valid UTF-8 (guaranteed by String) and non-panic.
            let _ = redacted.len();
        }

        // Also feed raw random bytes through the lossy boundary.
        if i % 7 == 0 {
            let mut bytes = Vec::new();
            for _ in 0..rng.below(256) {
                bytes.push((rng.next_u64() & 0xFF) as u8);
            }
            let lossy = String::from_utf8_lossy(&bytes);
            assert_consistent(&claude.parse_str(&lossy, "fuzz"));
            assert_consistent(&codex.parse_str(&lossy, "fuzz"));
        }
    }
}
