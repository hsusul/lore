//! Secret scanning (rules + entropy) that gates search/export.
//!
//! Load-bearing for **preventing amplification** (see
//! `docs/architecture/SECRET_SCANNING.md`): every cleartext field Lore will
//! index, export, cache, or log is scanned first, and flagged spans are masked
//! out of derived surfaces. This is an own engine with curated rules seeded from
//! public provider formats — no third-party rule file, no regex dependency.
//!
//! Two complementary detectors are merged and de-overlapped:
//! 1. **Rule matchers** — fixed prefixes + charset/length, high precision for
//!    known provider secrets;
//! 2. **Entropy heuristic** — high Shannon-entropy base64/base64url tokens the
//!    rules miss, with context suppressors for hashes/UUIDs/paths/data URIs.
//!
//! The scanner never stores a second cleartext copy of a value: a finding
//! carries offsets, a rule id, a severity, and a keyed fingerprint only.

use std::panic::AssertUnwindSafe;

#[cfg(test)]
use std::cell::Cell;

#[cfg(test)]
thread_local! {
    // Test seam: while armed (on the current thread), `scan` fails content-free
    // so quarantine behavior is exercisable deterministically. Thread-local so a
    // test arming it never affects other tests' scans.
    pub(crate) static FAIL_SCANS_FOR_TEST: Cell<bool> = const { Cell::new(false) };
}

/// Arm/disarm the scan-failure seam for the current thread (tests only).
#[cfg(test)]
pub(crate) fn set_fail_scans_for_test(on: bool) {
    FAIL_SCANS_FOR_TEST.with(|armed| armed.set(on));
}

/// Finding severity, most to least urgent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Severity {
    Low,
    Medium,
    High,
    Critical,
}

impl Severity {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Severity::Low => "low",
            Severity::Medium => "medium",
            Severity::High => "high",
            Severity::Critical => "critical",
        }
    }
}

/// One detected secret span (byte offsets into the scanned text).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    pub rule: &'static str,
    pub start: usize,
    pub end: usize,
    pub severity: Severity,
}

impl Finding {
    /// A keyed, non-reversible-enough fingerprint of the flagged value plus its
    /// rule, for allowlist/dedup matching. Never a second cleartext copy.
    #[must_use]
    pub fn fingerprint(&self, text: &str) -> String {
        fingerprint(self.rule, &text[self.start..self.end])
    }
}

/// A trailing character class for a prefix rule.
type CharPred = fn(u8) -> bool;

fn alnum(b: u8) -> bool {
    b.is_ascii_alphanumeric()
}
fn upper_num(b: u8) -> bool {
    b.is_ascii_uppercase() || b.is_ascii_digit()
}
fn alnum_dash_us(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'-' || b == b'_'
}
fn slack_tail(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'-'
}

/// A known provider format: one or more literal prefixes followed by at least
/// `min_tail` characters of `tail`.
struct PrefixRule {
    id: &'static str,
    severity: Severity,
    prefixes: &'static [&'static str],
    tail: CharPred,
    min_tail: usize,
}

/// Curated rules seeded from public provider formats (SECRET_SCANNING §3).
/// Order matters: more specific prefixes (e.g. `sk-ant-`) precede broader ones
/// (`sk-`) so de-overlapping keeps the specific rule.
const PREFIX_RULES: &[PrefixRule] = &[
    PrefixRule {
        id: "private-key-block",
        severity: Severity::Critical,
        prefixes: &["-----BEGIN "],
        tail: |_| false, // handled specially below; placeholder never matches
        min_tail: usize::MAX,
    },
    PrefixRule {
        id: "aws-access-key-id",
        severity: Severity::High,
        prefixes: &["AKIA", "ASIA", "AGPA", "AIDA", "AROA", "ANPA"],
        tail: upper_num,
        min_tail: 16,
    },
    PrefixRule {
        id: "gcp-api-key",
        severity: Severity::High,
        prefixes: &["AIza"],
        tail: alnum_dash_us,
        min_tail: 35,
    },
    PrefixRule {
        id: "github-token",
        severity: Severity::High,
        prefixes: &["ghp_", "gho_", "ghu_", "ghs_", "ghr_"],
        tail: alnum,
        min_tail: 36,
    },
    PrefixRule {
        id: "gitlab-pat",
        severity: Severity::High,
        prefixes: &["glpat-"],
        tail: alnum_dash_us,
        min_tail: 20,
    },
    PrefixRule {
        id: "slack-token",
        severity: Severity::High,
        prefixes: &["xoxb-", "xoxa-", "xoxp-", "xoxr-", "xoxs-"],
        tail: slack_tail,
        min_tail: 10,
    },
    PrefixRule {
        id: "stripe-key",
        severity: Severity::Critical,
        prefixes: &["sk_live_", "rk_live_"],
        tail: alnum,
        min_tail: 16,
    },
    PrefixRule {
        id: "anthropic-key",
        severity: Severity::High,
        prefixes: &["sk-ant-"],
        tail: alnum_dash_us,
        min_tail: 20,
    },
    PrefixRule {
        id: "openai-key",
        severity: Severity::High,
        prefixes: &["sk-"],
        tail: alnum,
        min_tail: 20,
    },
    PrefixRule {
        id: "google-oauth-secret",
        severity: Severity::High,
        prefixes: &["GOCSPX-"],
        tail: alnum_dash_us,
        min_tail: 20,
    },
    PrefixRule {
        id: "npm-token",
        severity: Severity::High,
        prefixes: &["npm_"],
        tail: alnum,
        min_tail: 36,
    },
];

/// Documented example/test keys and placeholder shapes that must never be
/// flagged (SECRET_SCANNING §5). Assembled from split literals so the source
/// never contains a complete provider-format token (avoids tripping upstream
/// push-protection scanners on our own fixtures).
const ALLOWLIST_VALUES: &[&str] = &[
    concat!("AKIA", "IOSFODNN7EXAMPLE"),
    concat!("wJalrXUtnFEMI", "/K7MDENG/bPxRfiCYEXAMPLEKEY"),
];

/// Scan `text` and return findings sorted by start offset and de-overlapped.
///
/// Scanning is load-bearing for preventing amplification, so it is **fallible
/// by contract**: a scanner defect on untrusted input must never panic the
/// worker. A panic anywhere inside the detectors is captured and reported as a
/// content-free failure, which quarantines the field from search/export
/// (`SECRET_SCANNING.md` §6).
pub fn scan(text: &str) -> Result<Vec<Finding>> {
    #[cfg(test)]
    if FAIL_SCANS_FOR_TEST.with(|armed| armed.get()) {
        return Err(ScanError::Failed);
    }
    let findings = std::panic::catch_unwind(AssertUnwindSafe(|| scan_inner(text)))
        .map_err(|_| ScanError::Failed)?;
    // Enforce that every finding is an in-bounds, char-aligned sub-slice of
    // `text`. Downstream `Finding::fingerprint` and `redact` slice `text` by
    // these offsets *outside* any panic guard (during ingest and export), so a
    // scanner defect that produced an unsafe span would otherwise panic the
    // worker thread and wedge ingestion. Quarantining the field here (Err →
    // no findings recorded, no projection) keeps the pipeline fail-secure
    // (SECRET_SCANNING.md §6). This never triggers for the current scanners,
    // whose spans are always ASCII-anchored token boundaries.
    if !findings_sliceable(text, &findings) {
        return Err(ScanError::Failed);
    }
    Ok(findings)
}

/// True when every finding's byte span is in-bounds, non-inverted, and aligned
/// to UTF-8 char boundaries — the precondition for slicing `text` by it without
/// a panic.
fn findings_sliceable(text: &str, findings: &[Finding]) -> bool {
    findings.iter().all(|f| {
        f.start <= f.end
            && f.end <= text.len()
            && text.is_char_boundary(f.start)
            && text.is_char_boundary(f.end)
    })
}

/// The infallible detector pass; wrapped by [`scan`] so any panic on untrusted
/// input becomes a content-free [`ScanError::Failed`].
fn scan_inner(text: &str) -> Vec<Finding> {
    let bytes = text.as_bytes();
    let mut raw: Vec<Finding> = Vec::new();

    scan_prefix_rules(bytes, &mut raw);
    scan_private_key_blocks(text, &mut raw);
    scan_jwt(bytes, &mut raw);
    scan_connection_strings(text, bytes, &mut raw);
    scan_webhooks(text, bytes, &mut raw);
    scan_entropy(text, bytes, &mut raw);

    // Drop allowlisted values.
    raw.retain(|f| !is_allowlisted(&text[f.start..f.end]));

    de_overlap(raw)
}

/// A secret-scan failure. Content-free: never echoes the offending text or a
/// diagnostic (SECRET_SCANNING.md §6 — a failure quarantines the field).
#[derive(Debug, thiserror::Error)]
pub enum ScanError {
    #[error("secret scan failed")]
    Failed,
}

/// Convenience result alias for the scanner.
pub type Result<T> = std::result::Result<T, ScanError>;

/// Produce a redacted copy of `text` with every flagged span replaced by a
/// deterministic, content-free mask. The surrounding text is preserved so it
/// stays searchable; the raw span never appears.
#[must_use]
pub fn redact(text: &str, findings: &[Finding]) -> String {
    if findings.is_empty() {
        return text.to_string();
    }
    let mut out = String::with_capacity(text.len());
    let mut cursor = 0usize;
    let mut ordered = findings.to_vec();
    ordered.sort_by_key(|f| f.start);
    for finding in ordered {
        if finding.start < cursor {
            continue; // overlap guard
        }
        out.push_str(&text[cursor..finding.start]);
        out.push_str("«redacted:");
        out.push_str(finding.rule);
        out.push('»');
        cursor = finding.end;
    }
    out.push_str(&text[cursor..]);
    out
}

fn scan_prefix_rules(bytes: &[u8], out: &mut Vec<Finding>) {
    for rule in PREFIX_RULES {
        if rule.id == "private-key-block" {
            continue; // scanned separately
        }
        for prefix in rule.prefixes {
            let pb = prefix.as_bytes();
            let mut from = 0;
            while let Some(rel) = find(&bytes[from..], pb) {
                let start = from + rel;
                from = start + 1;
                // Require a word boundary before the prefix.
                if start > 0 && is_word_byte(bytes[start - 1]) {
                    continue;
                }
                let tail_start = start + pb.len();
                let run = run_len(bytes, tail_start, rule.tail);
                if run >= rule.min_tail {
                    out.push(Finding {
                        rule: rule.id,
                        start,
                        end: tail_start + run,
                        severity: rule.severity,
                    });
                }
            }
        }
    }
}

fn scan_private_key_blocks(text: &str, out: &mut Vec<Finding>) {
    let mut from = 0;
    while let Some(rel) = text[from..].find("-----BEGIN ") {
        let start = from + rel;
        // Match up to the corresponding END line, else the BEGIN line only.
        let after = &text[start..];
        if !after
            .lines()
            .next()
            .is_some_and(|line| line.contains("PRIVATE KEY-----"))
        {
            from = start + 1;
            continue;
        }
        let end = match after.find("PRIVATE KEY-----").and_then(|_| {
            after
                .match_indices("-----END ")
                .find(|(_, _)| true)
                .map(|(i, _)| i)
        }) {
            Some(end_rel) => {
                let end_line = &after[end_rel..];
                let line_end = end_line.find('\n').map_or(after.len(), |n| end_rel + n + 1);
                start + line_end
            }
            None => start + after.lines().next().map_or(0, str::len),
        };
        out.push(Finding {
            rule: "private-key-block",
            start,
            end,
            severity: Severity::Critical,
        });
        from = end;
    }
}

fn scan_jwt(bytes: &[u8], out: &mut Vec<Finding>) {
    let needle = b"eyJ";
    let mut from = 0;
    while let Some(rel) = find(&bytes[from..], needle) {
        let start = from + rel;
        from = start + 1;
        if start > 0 && is_word_byte(bytes[start - 1]) {
            continue;
        }
        // header.payload.signature, each base64url and non-trivial.
        let seg0 = run_len(bytes, start, is_b64url);
        let mut p = start + seg0;
        if seg0 < 10 || p >= bytes.len() || bytes[p] != b'.' {
            continue;
        }
        p += 1;
        let seg1 = run_len(bytes, p, is_b64url);
        p += seg1;
        if seg1 < 10 || p >= bytes.len() || bytes[p] != b'.' {
            continue;
        }
        p += 1;
        let seg2 = run_len(bytes, p, is_b64url);
        if seg2 < 10 {
            continue;
        }
        out.push(Finding {
            rule: "jwt",
            start,
            end: p + seg2,
            severity: Severity::Medium,
        });
    }
}

fn scan_connection_strings(text: &str, bytes: &[u8], out: &mut Vec<Finding>) {
    const SCHEMES: &[&str] = &[
        "postgres://",
        "postgresql://",
        "mysql://",
        "mongodb://",
        "mongodb+srv://",
        "redis://",
        "amqp://",
    ];
    for scheme in SCHEMES {
        let mut from = 0;
        while let Some(rel) = text[from..].find(scheme) {
            let start = from + rel;
            from = start + 1;
            let creds_start = start + scheme.len();
            // Require user:pass@ with non-space, non-@ credentials.
            let user = run_until(bytes, creds_start, |b| b == b':' || b == b'@' || b == b' ');
            let after_user = creds_start + user;
            if user == 0 || after_user >= bytes.len() || bytes[after_user] != b':' {
                continue;
            }
            let pass = run_until(bytes, after_user + 1, |b| b == b'@' || b == b' ');
            let after_pass = after_user + 1 + pass;
            if pass == 0 || after_pass >= bytes.len() || bytes[after_pass] != b'@' {
                continue;
            }
            out.push(Finding {
                rule: "connection-string",
                start,
                end: after_pass, // through the credentials, before '@'
                severity: Severity::High,
            });
        }
    }
}

/// Slack/Discord incoming-webhook URLs (the path token is the secret).
fn scan_webhooks(text: &str, bytes: &[u8], out: &mut Vec<Finding>) {
    const HOOKS: &[(&str, &str)] = &[
        ("hooks.slack.com/services/", "slack-webhook"),
        ("discord.com/api/webhooks/", "discord-webhook"),
        ("discordapp.com/api/webhooks/", "discord-webhook"),
    ];
    for (needle, rule) in HOOKS {
        let mut from = 0;
        while let Some(rel) = text[from..].find(needle) {
            let start = from + rel;
            let mut end = start + needle.len();
            while end < bytes.len() && is_url_token(bytes[end]) {
                end += 1;
            }
            if end > start + needle.len() {
                out.push(Finding {
                    rule,
                    start,
                    end,
                    severity: Severity::Medium,
                });
            }
            from = start + needle.len();
        }
    }
}

fn is_url_token(b: u8) -> bool {
    !b.is_ascii_whitespace() && !matches!(b, b'"' | b'\'' | b'<' | b'>' | b')' | b']')
}

fn scan_entropy(text: &str, bytes: &[u8], out: &mut Vec<Finding>) {
    let mut i = 0;
    while i < bytes.len() {
        if !is_token_byte(bytes[i]) {
            i += 1;
            continue;
        }
        let start = i;
        while i < bytes.len() && is_token_byte(bytes[i]) {
            i += 1;
        }
        let end = i;
        let token = &text[start..end];
        if token.len() < 20 {
            continue;
        }
        if is_hexish(token) {
            continue; // git SHAs, UUIDs, hex hashes
        }
        if preceded_by_data_uri(bytes, start) {
            continue; // base64 image / data URI
        }
        let entropy = shannon_per_char(token.as_bytes());
        if entropy < 4.0 {
            continue;
        }
        let severity = if near_secret_keyword(bytes, start) {
            Severity::Medium
        } else {
            Severity::Low
        };
        out.push(Finding {
            rule: "high-entropy",
            start,
            end,
            severity,
        });
    }
}

// ── de-overlap ───────────────────────────────────────────────────────────────

/// Sort by start; when spans overlap keep the higher-severity (then longer) one.
fn de_overlap(mut findings: Vec<Finding>) -> Vec<Finding> {
    findings.sort_by(|a, b| {
        a.start
            .cmp(&b.start)
            .then(b.severity.cmp(&a.severity))
            .then((b.end - b.start).cmp(&(a.end - a.start)))
    });
    let mut kept: Vec<Finding> = Vec::new();
    for finding in findings {
        match kept.last() {
            Some(last) if finding.start < last.end => {
                // Overlaps the kept span; replace only if strictly better.
                if finding.severity > last.severity && finding.end > last.end {
                    kept.pop();
                    kept.push(finding);
                }
            }
            _ => kept.push(finding),
        }
    }
    kept
}

// ── helpers ──────────────────────────────────────────────────────────────────

fn is_allowlisted(value: &str) -> bool {
    if ALLOWLIST_VALUES.contains(&value) {
        return true;
    }
    // Placeholder shapes: <...>, ${...}, all-x, common placeholders.
    let lower = value.to_ascii_lowercase();
    if lower.contains("example")
        || lower.contains("your_")
        || lower.contains("changeme")
        || lower.contains("placeholder")
        || lower.contains("xxxxxxxx")
    {
        return true;
    }
    let after_underscore = value.rsplit('_').next().unwrap_or(value);
    after_underscore.chars().all(|c| c == 'x' || c == 'X') && after_underscore.len() >= 8
}

fn is_word_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

fn is_b64url(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'-' || b == b'_'
}

/// Token alphabet for the entropy pass (base64/base64url-ish, plus `+/=`).
fn is_token_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || matches!(b, b'+' | b'/' | b'=' | b'-' | b'_')
}

fn is_hexish(token: &str) -> bool {
    token.bytes().all(|b| b.is_ascii_hexdigit() || b == b'-')
}

fn run_len(bytes: &[u8], start: usize, pred: CharPred) -> usize {
    let mut i = start;
    while i < bytes.len() && pred(bytes[i]) {
        i += 1;
    }
    i - start
}

fn run_until(bytes: &[u8], start: usize, stop: impl Fn(u8) -> bool) -> usize {
    let mut i = start;
    while i < bytes.len() && !stop(bytes[i]) {
        i += 1;
    }
    i - start
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || needle.len() > haystack.len() {
        return None;
    }
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

/// True if a `key=`/`token:`/`secret …` keyword appears shortly before `pos`.
fn near_secret_keyword(bytes: &[u8], pos: usize) -> bool {
    const KEYWORDS: &[&[u8]] = &[
        b"key",
        b"token",
        b"secret",
        b"password",
        b"passwd",
        b"auth",
        b"apikey",
    ];
    let window_start = pos.saturating_sub(24);
    let ctx = to_lower(&bytes[window_start..pos]);
    KEYWORDS.iter().any(|kw| contains(&ctx, kw))
}

fn preceded_by_data_uri(bytes: &[u8], pos: usize) -> bool {
    let window_start = pos.saturating_sub(16);
    let ctx = to_lower(&bytes[window_start..pos]);
    contains(&ctx, b"data:") || contains(&ctx, b"base64,")
}

fn to_lower(bytes: &[u8]) -> Vec<u8> {
    bytes.iter().map(u8::to_ascii_lowercase).collect()
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    find(haystack, needle).is_some()
}

/// Shannon entropy in bits per character over the token bytes.
fn shannon_per_char(bytes: &[u8]) -> f64 {
    if bytes.is_empty() {
        return 0.0;
    }
    let mut counts = [0u32; 256];
    for &b in bytes {
        counts[b as usize] += 1;
    }
    let n = bytes.len() as f64;
    let mut entropy = 0.0;
    for count in counts {
        if count > 0 {
            let p = f64::from(count) / n;
            entropy -= p * p.log2();
        }
    }
    entropy
}

/// FNV-1a hex fingerprint of `rule` + value. Not cryptographic; used only to
/// match/dedup findings and allowlist entries, never to reconstruct a value.
fn fingerprint(rule: &str, value: &str) -> String {
    const OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x0000_0100_0000_01b3;
    let mut hash = OFFSET;
    for byte in rule
        .bytes()
        .chain(b"\0".iter().copied())
        .chain(value.bytes())
    {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(PRIME);
    }
    format!("{hash:016x}")
}

#[cfg(test)]
mod tests {
    use super::*;

    // Synthetic secrets are assembled from split parts so no complete
    // provider-format token is ever a contiguous literal in the source. The
    // scanner still receives the full value at runtime; this only prevents our
    // own fixtures from tripping upstream (e.g. GitHub) push-protection scanners.
    fn t(prefix: &str, body: &str) -> String {
        format!("{prefix}{body}")
    }

    fn rules_in(text: &str) -> Vec<&'static str> {
        let mut r: Vec<&'static str> = scan_ok(text).into_iter().map(|f| f.rule).collect();
        r.sort_unstable();
        r
    }

    #[test]
    fn only_char_aligned_in_bounds_spans_are_sliceable() {
        // "héllo": é occupies bytes 1..3, so byte 2 is mid-character.
        let text = "héllo";
        let fin = |start, end| Finding {
            rule: "test",
            start,
            end,
            severity: Severity::Low,
        };
        // Valid, char-aligned spans slice safely.
        assert!(findings_sliceable(text, &[fin(0, 1)]));
        assert!(findings_sliceable(text, &[fin(3, 6)]));
        // These would otherwise panic Finding::fingerprint / redact on the ingest
        // worker thread, so scan() must quarantine the field instead.
        assert!(!findings_sliceable(text, &[fin(1, 2)]), "end mid-character");
        assert!(
            !findings_sliceable(text, &[fin(2, 3)]),
            "start mid-character"
        );
        assert!(
            !findings_sliceable(text, &[fin(0, 999)]),
            "end out of bounds"
        );
        assert!(!findings_sliceable(text, &[fin(4, 1)]), "inverted span");
    }

    fn scan_ok(text: &str) -> Vec<Finding> {
        scan(text).unwrap()
    }

    fn found(secret: &str) -> Vec<&'static str> {
        rules_in(&format!("prefix {secret} suffix"))
    }

    #[test]
    fn flags_one_known_fake_per_rule() {
        assert!(found(&t("AKIA", "ABCDEFGHIJKLMNOP")).contains(&"aws-access-key-id"));
        assert!(found(&t("AIza", "SyA1234567890abcdefghijklmnopqrstuvz")).contains(&"gcp-api-key"));
        assert!(found(&t("ghp", "_0123456789abcdefghijklmnopqrstuvwxyz")).contains(&"github-token"));
        assert!(found(&t("glpat", "-abcdef0123456789ABCDEF")).contains(&"gitlab-pat"));
        assert!(found(&t("xoxb", "-123456789012-abcdefABCDEF")).contains(&"slack-token"));
        assert!(found(&t("sk", "_live_0123456789abcdefABCD")).contains(&"stripe-key"));
        assert!(found(&t("sk-ant", "-api03-abcdef0123456789ABCDEFG")).contains(&"anthropic-key"));
        assert!(found(&t("sk", "-proj0123456789abcdefghij")).contains(&"openai-key"));
        assert!(found(&t("GOCSPX", "-abcdefghij0123456789ABC")).contains(&"google-oauth-secret"));
        assert!(found(&t("npm", "_0123456789abcdefghijklmnopqrstuvwxyz12")).contains(&"npm-token"));

        let jwt = t(
            "eyJ",
            "hbGciOiJIUzI1NiJ9.eyJzdWIiOiIxMjM0NTY3ODkwIn0.dozjgNryP4J3jVmNHl0w5N",
        );
        assert!(found(&jwt).contains(&"jwt"));

        let conn = format!("postgres://admin:{}@db.internal/app", "s3cr3tPassw0rd");
        assert!(found(&conn).contains(&"connection-string"));

        let slack = format!(
            "https://hooks.slack.com/services/{}",
            "T00000000/B00000000/abcdefghijklmnopqrstuvwx"
        );
        assert!(found(&slack).contains(&"slack-webhook"));

        let discord = format!(
            "https://discord.com/api/webhooks/{}",
            "123456789012345678/abcdefABCDEF-0123456789_ghijkl"
        );
        assert!(found(&discord).contains(&"discord-webhook"));

        let pem = format!(
            "-----BEGIN RSA {p} KEY-----\nMIIEabc123\n-----END RSA {p} KEY-----",
            p = "PRIVATE"
        );
        assert!(rules_in(&pem).contains(&"private-key-block"));
    }

    #[test]
    fn anthropic_beats_openai_on_overlap() {
        // `sk-ant-…` must be labeled anthropic, not the broader openai `sk-`.
        let rules = found(&t("sk-ant", "-api03-abcdefghijklmnop0123456789"));
        assert!(rules.contains(&"anthropic-key"));
        assert!(!rules.contains(&"openai-key"));
    }

    #[test]
    fn does_not_flag_the_negative_corpus() {
        // git SHA (40 hex), UUID, data-URI base64, .env.example placeholders.
        let sha = t("9f1c2d3e4b5a", "69788c9daebf0011223344556677");
        assert!(scan_ok(&format!("commit {sha}")).is_empty());
        assert!(scan_ok("id 550e8400-e29b-41d4-a716-446655440000").is_empty());
        assert!(scan_ok("img data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAAB").is_empty());
        assert!(scan_ok(&format!("API_KEY={}", "YOUR_API_KEY_HERE")).is_empty());
        assert!(scan_ok(&format!("TOKEN={}", "x".repeat(20))).is_empty());
    }

    #[test]
    fn allowlists_documented_example_keys() {
        let example = t("AKIA", "IOSFODNN7EXAMPLE");
        assert!(scan_ok(&format!("aws_access_key_id = {example}")).is_empty());
    }

    #[test]
    fn high_entropy_base64_token_is_flagged() {
        let token = t("Zm9vYmFy", "QmF6UXV4MTIzNDU2Nzg5MFFXZXJ0eVpY");
        let findings = scan_ok(&format!("api_key: {token}"));
        assert!(findings.iter().any(|f| f.rule == "high-entropy"));
    }

    #[test]
    fn finds_a_secret_in_the_middle_of_a_large_field() {
        let secret = t("ghp", "_0123456789abcdefghijklmnopqrstuvwxyz");
        let mut text = "lorem ipsum ".repeat(2000);
        let at = text.len();
        text.push_str(&secret);
        text.push(' ');
        text.push_str(&"dolor sit ".repeat(2000));
        let findings = scan_ok(&text);
        let github = findings.iter().find(|f| f.rule == "github-token").unwrap();
        assert_eq!(github.start, at);
    }

    #[test]
    fn redaction_masks_the_span_and_keeps_context() {
        let secret = t("ghp", "_0123456789abcdefghijklmnopqrstuvwxyz");
        let text = format!("use {secret} now");
        let findings = scan_ok(&text);
        let redacted = redact(&text, &findings);
        assert!(!redacted.contains(&secret));
        assert!(redacted.contains("use "));
        assert!(redacted.contains("now"));
        assert!(redacted.contains("«redacted:github-token»"));
    }

    #[test]
    fn redaction_is_correct_when_secret_is_surrounded_by_multibyte_text() {
        // Real sessions contain emoji/accented text; a token flanked by multibyte
        // characters must still be found, masked, and leave the surrounding
        // multibyte context intact — with the span offsets landing on char
        // boundaries so the byte slicing never panics.
        let secret = t("ghp", "_0123456789abcdefghijklmnopqrstuvwxyz");
        let text = format!("café 🔑 deploy {secret} 日本語 done");
        let findings = scan_ok(&text);
        assert!(
            findings.iter().any(|f| f.rule == "github-token"),
            "token adjacent to multibyte text must still be detected"
        );
        let redacted = redact(&text, &findings);
        assert!(!redacted.contains(&secret), "raw secret must not survive");
        assert!(redacted.contains("café 🔑 deploy "));
        assert!(redacted.contains(" 日本語 done"));
        assert!(redacted.contains("«redacted:github-token»"));
    }

    #[test]
    fn fingerprint_is_stable_and_not_the_value() {
        let text = t("sk", "_live_0123456789abcdefABCD");
        let finding = &scan_ok(&text)[0];
        let fp = finding.fingerprint(&text);
        assert_eq!(fp.len(), 16);
        assert!(!fp.contains("live_"));
        assert_eq!(fp, finding.fingerprint(&text));
    }

    #[test]
    fn a_scan_panic_is_captured_as_a_content_free_failure() {
        // The seam simulates a scanner defect on untrusted input: it must be a
        // captured failure (which quarantines the field), never a panic.
        set_fail_scans_for_test(true);
        let result = scan("any untrusted text");
        set_fail_scans_for_test(false);

        assert!(
            result.is_err(),
            "a scanner failure is an error, never a panic"
        );
        assert!(scan_ok("id 550e8400-e29b-41d4-a716-446655440000").is_empty());
    }
}
