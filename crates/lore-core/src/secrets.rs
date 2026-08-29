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
        // GitHub fine-grained PATs (`github_pat_…`) carry `_` inside the token
        // body, so they need the alnum_dash_us alphabet and their own rule id
        // (the classic `ghp_`-family rule uses a strict alphanumeric tail).
        id: "github-fine-grained-pat",
        severity: Severity::High,
        prefixes: &["github_pat_"],
        tail: alnum_dash_us,
        min_tail: 20,
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
    scan_aws_secret_key(text, bytes, &mut raw);
    scan_gcp_service_account(text, bytes, &mut raw);
    scan_entropy(text, bytes, &mut raw);
    // Last, and strictly additive: it only reports spans no other detector
    // already covers (see `scan_generic_assignment`).
    scan_generic_assignment(text, bytes, &mut raw);

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

/// AWS secret access keys are exactly 40 base64 characters with **no prefix**,
/// so shape alone is far too common to flag — hashes, ids, and encoded blobs all
/// match. The `aws` context gate does all the precision work here.
const AWS_SECRET_LEN: usize = 40;
/// How far back to look for `aws`. `aws_secret_access_key = ` is 24 bytes, so
/// this covers the usual assignment plus quoting and indentation.
const AWS_CONTEXT_WINDOW: usize = 48;

fn is_base64_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || matches!(b, b'+' | b'/' | b'=')
}

/// AWS secret access keys (`SECRET_SCANNING.md` §3, rule `aws-secret-key`).
///
/// Unlike `aws-access-key-id` (`AKIA…`) there is no prefix to key off, so this
/// requires an exactly-40-character base64 run **and** `aws` nearby. The context
/// check is deliberately local rather than an extension of
/// [`near_secret_keyword`]: widening that shared helper would silently change
/// the severity the entropy detector assigns everywhere else.
fn scan_aws_secret_key(text: &str, bytes: &[u8], out: &mut Vec<Finding>) {
    let mut i = 0;
    while i < bytes.len() {
        if !is_base64_byte(bytes[i]) {
            i += 1;
            continue;
        }
        let start = i;
        while i < bytes.len() && is_base64_byte(bytes[i]) {
            i += 1;
        }
        if i - start != AWS_SECRET_LEN {
            continue;
        }
        let window = &bytes[start.saturating_sub(AWS_CONTEXT_WINDOW)..start];
        if !contains_ignore_ascii_case_bytes(window, b"aws") {
            continue;
        }
        // A run of only hex is a digest, not a key, whatever the context.
        if is_hexish(&text[start..i]) {
            continue;
        }
        out.push(Finding {
            rule: "aws-secret-key",
            start,
            end: i,
            severity: Severity::High,
        });
    }
}

/// GCP service-account JSON (`SECRET_SCANNING.md` §3, rule `gcp-sa-key`).
///
/// Structural rather than token-shaped: the file is only a credential when the
/// `service_account` marker and a `private_key` field appear together. The span
/// covers the private-key value, which subsumes the PEM block that
/// [`scan_private_key_blocks`] would otherwise report on its own — de-overlap
/// then keeps this finding, which names the provider.
///
/// **Span choice (deliberate).** The span starts at the `"private_key"` field
/// name, not at the value. Both rules are `Critical` and both start at the PEM
/// header, so a value-only span would lose the de-overlap tie-break to the
/// greedier `private-key-block` and the provider would never be named. Fifteen
/// characters of field name are redacted along with the key; naming the
/// credential is worth more than preserving the label of the field it sat in.
fn scan_gcp_service_account(text: &str, bytes: &[u8], out: &mut Vec<Finding>) {
    if find(bytes, b"\"service_account\"").is_none() {
        return;
    }
    let Some(key_at) = find(bytes, b"\"private_key\"") else {
        return;
    };
    let mut i = key_at + b"\"private_key\"".len();
    i += run_len(bytes, i, |b| b == b' ' || b == b'\t');
    if i >= bytes.len() || bytes[i] != b':' {
        return;
    }
    i += 1;
    i += run_len(bytes, i, |b| b == b' ' || b == b'\t');
    if i >= bytes.len() || bytes[i] != b'"' {
        return;
    }
    let value_start = i + 1;
    // JSON string: stop at the first unescaped quote.
    let mut end = value_start;
    while end < bytes.len() {
        match bytes[end] {
            b'\\' => end += 2,
            b'"' => break,
            _ => end += 1,
        }
    }
    let end = end.min(bytes.len());
    if end <= value_start || !text.is_char_boundary(key_at) || !text.is_char_boundary(end) {
        return;
    }
    out.push(Finding {
        rule: "gcp-sa-key",
        start: key_at,
        end,
        severity: Severity::Critical,
    });
}

/// Shortest assigned value worth flagging (`SECRET_SCANNING.md` §3).
const ASSIGN_MIN_LEN: usize = 8;
/// Entropy floor for an assigned value, in bits per character.
///
/// This number is the whole precision story for `generic-assignment`, so it is
/// deliberately explicit rather than inlined. [`scan_entropy`] already covers
/// tokens of 20+ characters at 4.0 bits/char, so this rule exists for the 8–19
/// character band — where a naive length-only match would flag ordinary prose,
/// config samples, and documentation. 3.0 bits/char means an 8-character value
/// must be near-maximally varied to qualify.
///
/// The tradeoff is a known and accepted miss: a repetitive weak value such as
/// `password = supersecret` scores ≈2.75 and is not flagged. Raising recall
/// there costs precision across a corpus that is mostly source code and prose;
/// retune against the corpus in `mod tests`, not by intuition.
const ASSIGN_MIN_ENTROPY: f64 = 3.0;

/// `keyword = value` / `"keyword": "value"` assignments — the shape most
/// credentials actually take in an agent transcript (`SECRET_SCANNING.md` §3,
/// rule `generic-assignment`).
///
/// The span covers the **value only**, so the shared allowlist in [`scan_inner`]
/// sees the candidate secret rather than the surrounding syntax.
///
/// This detector is **strictly additive**: it runs last and skips any span another
/// detector already reported, so a provider-shaped token keeps its own specific,
/// higher-severity rule and this one never displaces it. Relying on [`de_overlap`]
/// alone would not be enough — an equal span at equal severity would be decided by
/// detector order rather than by specificity.
fn scan_generic_assignment(text: &str, bytes: &[u8], out: &mut Vec<Finding>) {
    // Only spans present before this detector ran count as "already covered".
    let already = out.len();
    const KEYWORDS: &[&str] = &[
        "password",
        "passwd",
        "secret",
        "api_key",
        "api-key",
        "apikey",
        "access_token",
        "access-token",
        "auth_token",
        "auth-token",
        "token",
    ];
    for keyword in KEYWORDS {
        let needle = keyword.as_bytes();
        let mut from = 0;
        while let Some(rel) = find_ignore_ascii_case(&bytes[from..], needle) {
            let start = from + rel;
            from = start + 1;

            // Word boundary: `token` must not match inside `access_token`,
            // which has its own (longer, earlier) keyword entry.
            if start > 0 && is_word_byte(bytes[start - 1]) {
                continue;
            }
            let mut i = start + needle.len();
            if i < bytes.len() && is_word_byte(bytes[i]) {
                continue;
            }

            // Optional closing quote (`"api_key": …`), whitespace, separator.
            if i < bytes.len() && matches!(bytes[i], b'"' | b'\'') {
                i += 1;
            }
            i += run_len(bytes, i, |b| b == b' ' || b == b'\t');
            if i >= bytes.len() || !matches!(bytes[i], b':' | b'=') {
                continue;
            }
            i += 1;
            i += run_len(bytes, i, |b| b == b' ' || b == b'\t');

            // Optional opening quote; when present it also terminates the value.
            let quote = bytes.get(i).copied().filter(|b| matches!(b, b'"' | b'\''));
            if quote.is_some() {
                i += 1;
            }
            let value_start = i;
            // A template hole (`${VAR}`, `$(VAR)`, `{{var}}`, `%{var}`, `%VAR%`, `<placeholder>`) must be
            // captured whole, including its closer, so the allowlist can
            // recognise it — otherwise the closing brace/parenthesis terminates the value
            // and the truncated fragment looks like a credential.
            let is_double_brace = matches!(
                (bytes.get(value_start), bytes.get(value_start + 1)),
                (Some(b'{'), Some(b'{'))
            );
            let is_percent_var = matches!(
                (bytes.get(value_start), bytes.get(value_start + 1)),
                (Some(b'%'), Some(b)) if *b != b'{' && is_token_byte(*b)
            );
            let template_close = match (bytes.get(value_start), bytes.get(value_start + 1)) {
                (Some(b'$'), Some(b'{')) => Some(b'}'),
                (Some(b'$'), Some(b'(')) => Some(b')'),
                (Some(b'%'), Some(b'{')) => Some(b'}'),
                (Some(b'<'), _) => Some(b'>'),
                _ => None,
            };
            let len = if is_double_brace {
                if let Some(pos) = find(&bytes[value_start + 2..], b"}}") {
                    pos + 4
                } else {
                    run_until(bytes, value_start, |b| match quote {
                        Some(q) => b == q || b.is_ascii_whitespace(),
                        None => {
                            b.is_ascii_whitespace()
                                || matches!(b, b'"' | b'\'' | b',' | b';' | b')' | b']')
                        }
                    })
                }
            } else if is_percent_var {
                if let Some(pos) = find(&bytes[value_start + 1..], b"%") {
                    pos + 2
                } else {
                    run_until(bytes, value_start, |b| match quote {
                        Some(q) => b == q || b.is_ascii_whitespace(),
                        None => {
                            b.is_ascii_whitespace()
                                || matches!(b, b'"' | b'\'' | b',' | b';' | b'}' | b')' | b']')
                        }
                    })
                }
            } else if let Some(closer) = template_close {
                let run = run_until(bytes, value_start, |b| b == closer);
                // Include the closer when it is actually there.
                if value_start + run < bytes.len() {
                    run + 1
                } else {
                    run
                }
            } else {
                run_until(bytes, value_start, |b| match quote {
                    Some(q) => b == q || b.is_ascii_whitespace(),
                    None => {
                        b.is_ascii_whitespace()
                            || matches!(b, b'"' | b'\'' | b',' | b';' | b'}' | b')' | b']')
                    }
                })
            };
            let value_end = value_start + len;
            if len < ASSIGN_MIN_LEN {
                continue;
            }
            let value = &text[value_start..value_end];
            if shannon_per_char(value.as_bytes()) < ASSIGN_MIN_ENTROPY {
                continue;
            }
            let covered = out[..already]
                .iter()
                .any(|f| f.start < value_end && value_start < f.end);
            if covered {
                continue;
            }
            out.push(Finding {
                rule: "generic-assignment",
                start: value_start,
                end: value_end,
                severity: Severity::Medium,
            });
        }
    }
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

fn contains_ignore_ascii_case(haystack: &str, needle: &str) -> bool {
    let needle_bytes = needle.as_bytes();
    if needle_bytes.is_empty() {
        return true;
    }
    if haystack.len() < needle.len() {
        return false;
    }
    haystack
        .as_bytes()
        .windows(needle_bytes.len())
        .any(|window| window.eq_ignore_ascii_case(needle_bytes))
}

fn is_allowlisted(value: &str) -> bool {
    if ALLOWLIST_VALUES.contains(&value) {
        return true;
    }
    // Placeholder shapes: <...>, ${...}, $(...), {{...}}, %{...}, %...%, all-x, common placeholders.
    // A value that is *wholly* delimited is a template hole, not a credential.
    let wrapped = |open: &str, close: &str| value.starts_with(open) && value.ends_with(close);
    if wrapped("${", "}")
        || wrapped("$(", ")")
        || wrapped("%{", "}")
        || wrapped("<", ">")
        || wrapped("{{", "}}")
        || (wrapped("%", "%") && value.len() >= 3 && !value[1..value.len() - 1].contains('%'))
    {
        return true;
    }
    if contains_ignore_ascii_case(value, "example")
        || contains_ignore_ascii_case(value, "your_")
        || contains_ignore_ascii_case(value, "changeme")
        || contains_ignore_ascii_case(value, "placeholder")
        || contains_ignore_ascii_case(value, "xxxxxxxx")
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

/// Position of `needle` in `haystack`, ignoring ASCII case, without allocating.
fn find_ignore_ascii_case(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || needle.len() > haystack.len() {
        return None;
    }
    haystack
        .windows(needle.len())
        .position(|window| window.eq_ignore_ascii_case(needle))
}

/// True if `needle` appears in `haystack`, ignoring ASCII case, without allocating.
fn contains_ignore_ascii_case_bytes(haystack: &[u8], needle: &[u8]) -> bool {
    needle.is_empty() || find_ignore_ascii_case(haystack, needle).is_some()
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
    let ctx = &bytes[window_start..pos];
    KEYWORDS
        .iter()
        .any(|&kw| contains_ignore_ascii_case_bytes(ctx, kw))
}

fn preceded_by_data_uri(bytes: &[u8], pos: usize) -> bool {
    let window_start = pos.saturating_sub(16);
    let ctx = &bytes[window_start..pos];
    contains_ignore_ascii_case_bytes(ctx, b"data:")
        || contains_ignore_ascii_case_bytes(ctx, b"base64,")
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
        // Fine-grained PATs carry an internal `_`; the strict `ghp_` rule misses
        // them, so they get their own rule (detected, not just generic entropy).
        assert!(found(&t(
            "github_pat",
            "_ABCDEFGHIJKLMNOPQRSTUV_abcdefghijklmnopqrstuvwxyz0123456789"
        ))
        .contains(&"github-fine-grained-pat"));
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
    fn aws_secret_key_needs_both_shape_and_context() {
        // 40 base64 characters, varied enough not to be hex.
        let key = t("wJalrXUtnFEMI", "/K7MDENG/bPxRfiCYzzzKEY1234");
        assert_eq!(key.len(), 40, "the fixture must be exactly 40 chars");

        let with_ctx = format!("aws_secret_access_key = {key}");
        assert!(
            rules_in(&with_ctx).contains(&"aws-secret-key"),
            "got {:?}",
            rules_in(&with_ctx)
        );

        // The same shape with no `aws` nearby is not an AWS finding. It may
        // still be caught by another detector; assert only this rule's absence.
        let no_ctx = format!("blob = {key}");
        assert!(!rules_in(&no_ctx).contains(&"aws-secret-key"));
    }

    #[test]
    fn aws_secret_key_ignores_a_forty_char_digest() {
        // A 40-hex SHA-1 next to `aws` is a digest, not a key.
        let sha = t("9f1c2d3e4b5a", "69788c9daebf0011223344556677");
        assert_eq!(sha.len(), 40, "the fixture must be exactly 40 chars");
        assert!(!rules_in(&format!("aws artifact sha {sha}")).contains(&"aws-secret-key"));
    }

    #[test]
    fn gcp_service_account_json_is_flagged_critical() {
        let json = concat!(
            "{\"type\": \"service_account\", \"project_id\": \"p\", ",
            "\"private_key\": \"-----BEGIN PRIVATE KEY-----\\nMIIEvQIBADAN\\n-----END PRIVATE KEY-----\\n\"}"
        );
        let findings = scan_ok(json);
        let hit = findings
            .iter()
            .find(|f| f.rule == "gcp-sa-key")
            .unwrap_or_else(|| panic!("expected gcp-sa-key, got {:?}", rules_in(json)));
        assert_eq!(hit.severity, Severity::Critical);
        assert!(
            json[hit.start..hit.end].contains("MIIEvQIBADAN"),
            "the span must cover the key material"
        );
        // It wins over the generic PEM rule, so the provider is named.
        assert!(!rules_in(json).contains(&"private-key-block"));
    }

    #[test]
    fn a_service_account_marker_without_a_private_key_is_not_flagged() {
        let json = "{\"type\": \"service_account\", \"project_id\": \"p\"}";
        assert!(!rules_in(json).contains(&"gcp-sa-key"));
        // …and a private_key with no service-account marker stays a plain
        // private-key block rather than being labelled as GCP.
        let pem = "key: -----BEGIN PRIVATE KEY-----\nMIIEvQIBADAN\n-----END PRIVATE KEY-----";
        let rules = rules_in(pem);
        assert!(rules.contains(&"private-key-block"), "got {rules:?}");
        assert!(!rules.contains(&"gcp-sa-key"));
    }

    #[test]
    fn generic_assignment_flags_short_assigned_values() {
        // The 8–19 character band the entropy detector deliberately skips.
        for text in [
            "password = hunter2xyz",
            "api_key: 9f3Kd0Lq7v",
            "auth-token='aB3xY7pQ2m'",
            "\"apikey\": \"Zq4vN8wR1t\"",
        ] {
            assert!(
                rules_in(text).contains(&"generic-assignment"),
                "expected a generic-assignment finding in {text:?}"
            );
        }
    }

    #[test]
    fn generic_assignment_spans_only_the_value() {
        let findings = scan_ok("password = hunter2xyz");
        let hit = findings
            .iter()
            .find(|f| f.rule == "generic-assignment")
            .expect("flagged");
        assert_eq!(&"password = hunter2xyz"[hit.start..hit.end], "hunter2xyz");
        assert_eq!(hit.severity, Severity::Medium);

        // Quotes are delimiters, not part of the secret.
        let quoted = "auth-token='aB3xY7pQ2m'";
        let findings = scan_ok(quoted);
        let hit = findings
            .iter()
            .find(|f| f.rule == "generic-assignment")
            .expect("flagged");
        assert_eq!(&quoted[hit.start..hit.end], "aB3xY7pQ2m");
    }

    #[test]
    fn generic_assignment_does_not_flag_placeholders_or_prose() {
        // Extends the negative corpus: these are the shapes that make a naive
        // length-only assignment rule unshippable.
        for text in [
            "API_KEY=YOUR_API_KEY_HERE",
            "TOKEN=xxxxxxxxxxxxxxxxxxxx",
            "password=changeme123",
            "secret=placeholder_value",
            "the password is required",
            "token: true",
            "api_key = ${MY_API_KEY}",
            "api_key = $(MY_API_KEY)",
            "api_key = {{MY_API_KEY}}",
            "api_key = \"{{MY_API_KEY}}\"",
            "api_key = \"%{MY_API_KEY}\"",
            "api_key = %MY_API_KEY%",
            "api_key = \"%MY_API_KEY%\"",
            "auth_token: <MY_AUTH_TOKEN>",
        ] {
            assert!(
                !rules_in(text).contains(&"generic-assignment"),
                "must not flag {text:?}"
            );
        }
    }

    #[test]
    fn generic_assignment_requires_a_word_boundary_and_a_separator() {
        // `token` inside `access_token` is covered by the longer keyword, not
        // matched twice, and a keyword with no separator is not an assignment.
        let findings = scan_ok("access_token = Kd0Lq7vN8wR1");
        assert_eq!(
            findings
                .iter()
                .filter(|f| f.rule == "generic-assignment")
                .count(),
            1,
            "one finding, not one per overlapping keyword"
        );
        assert!(!rules_in("mytoken means something").contains(&"generic-assignment"));
    }

    #[test]
    fn generic_assignment_defers_to_a_specific_rule() {
        // A provider-shaped token in an assignment keeps its own specific,
        // higher-severity rule; the fallback never displaces it.
        let gh = t("ghp", "_0123456789abcdefghijklmnopqrstuvwxyz");
        let rules = rules_in(&format!("api_key: {gh}"));
        assert!(rules.contains(&"github-token"), "got {rules:?}");
        assert!(!rules.contains(&"generic-assignment"), "got {rules:?}");

        // Nor does it displace the entropy detector on a span that detector
        // already owns (the regression that caught the ordering bug).
        let token = t("Zm9vYmFy", "QmF6UXV4MTIzNDU2Nzg5MFFXZXJ0eVpY");
        let rules = rules_in(&format!("api_key: {token}"));
        assert!(rules.contains(&"high-entropy"), "got {rules:?}");
        assert!(!rules.contains(&"generic-assignment"), "got {rules:?}");
    }

    #[test]
    fn generic_assignment_entropy_floor_is_pinned() {
        // The floor is a tuning decision, so pin both sides of it. An 8-char
        // value must be near-maximally varied; a repetitive one is a known,
        // documented miss (see ASSIGN_MIN_ENTROPY).
        assert!(rules_in("secret = aB3xY7pQ").contains(&"generic-assignment"));
        assert!(!rules_in("password = supersecret").contains(&"generic-assignment"));
        // Below the length floor regardless of entropy.
        assert!(!rules_in("secret = aB3xY7p").contains(&"generic-assignment"));
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

    #[test]
    fn redaction_handles_adjacent_secrets_and_empty_inputs() {
        assert_eq!(redact("", &[]), "");
        assert_eq!(redact("no secrets here", &[]), "no secrets here");

        let s1 = t("ghp", "_0123456789abcdefghijklmnopqrstuvwxyz");
        let s2 = t("sk", "_live_0123456789abcdefABCD");
        let combined = format!("{s1} {s2}");
        let findings = scan_ok(&combined);
        assert_eq!(findings.len(), 2);
        let redacted = redact(&combined, &findings);
        assert!(!redacted.contains(&s1));
        assert!(!redacted.contains(&s2));
        assert_eq!(redacted, "«redacted:github-token» «redacted:stripe-key»");
    }

    #[test]
    fn scan_detects_multiple_distinct_tokens_in_one_pass() {
        let gh = t("ghp", "_0123456789abcdefghijklmnopqrstuvwxyz");
        let slack = t("xoxb", "-123456789012-abcdefABCDEF");
        let combined = format!("GitHub: {gh}\nSlack: {slack}");
        let findings = scan_ok(&combined);
        assert_eq!(findings.len(), 2);
        assert!(findings.iter().any(|f| f.rule == "github-token"));
        assert!(findings.iter().any(|f| f.rule == "slack-token"));

        let redacted = redact(&combined, &findings);
        assert_eq!(
            redacted,
            "GitHub: «redacted:github-token»\nSlack: «redacted:slack-token»"
        );
    }

    #[test]
    fn shannon_entropy_bounds_and_allowlist_precedence() {
        // Repeated single char has 0.0 entropy
        assert_eq!(shannon_per_char(b""), 0.0);
        assert_eq!(shannon_per_char(b"AAAAAAAAAAAA"), 0.0);

        // High entropy tokens with allowlisted terms (e.g. changeme / example / your_) are suppressed
        let placeholder = "key=your_random_high_entropy_token_xyz123456";
        assert!(scan_ok(placeholder).is_empty());

        let changeme = "secret=changeme_987654321_abc_xyz_qwert";
        assert!(scan_ok(changeme).is_empty());
    }

    #[test]
    fn scan_and_redact_connection_strings_with_ipv6_and_various_schemes() {
        let uri1 = "postgres://admin:pass123@[2001:db8::1]:5432/mydb";
        let uri2 = "mongodb+srv://root:s3cret@[::1]/test";
        let uri3 = "redis://default:mypassword@localhost:6379/0";

        let text = format!("DB1: {uri1}\nDB2: {uri2}\nDB3: {uri3}");
        let findings = scan_ok(&text);
        assert_eq!(findings.len(), 3);
        for f in &findings {
            assert_eq!(f.rule, "connection-string");
        }

        let redacted = redact(&text, &findings);
        assert!(!redacted.contains("admin:pass123"));
        assert!(!redacted.contains("root:s3cret"));
        assert!(!redacted.contains("default:mypassword"));
        assert!(redacted.contains("«redacted:connection-string»@[2001:db8::1]:5432/mydb"));
        assert!(redacted.contains("«redacted:connection-string»@[::1]/test"));
        assert!(redacted.contains("«redacted:connection-string»@localhost:6379/0"));
    }

    #[test]
    fn scan_handles_null_bytes_and_whitespace_padding() {
        let token = t("ghp", "_0123456789abcdefghijklmnopqrstuvwxyz");
        let text = format!("\0\t  \n{token}\r\n \0");
        let findings = scan_ok(&text);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].rule, "github-token");
        let redacted = redact(&text, &findings);
        assert_eq!(redacted, "\0\t  \n«redacted:github-token»\r\n \0");
    }
}
