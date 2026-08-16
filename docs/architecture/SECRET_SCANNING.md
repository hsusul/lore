# Secret Scanning — Ruleset & Design

> The concrete design for detecting secrets in ingested sessions. This is **load-bearing for preventing amplification**, not a claim that detection is perfect or that the canonical local archive contains no secrets. Parent: `SECURITY.md` §4 and ADR-0005. `DATA_MODEL.md` owns `Blob`, `SecretFinding`, and `SearchDocument`. Tags: **DECISION** / **OPINION** / **OPEN**.

## 1. Goals & non-goals

**Goals:** (1) high recall on **high-signal** provider secrets; (2) keep flagged spans out of FTS, default exports, and logs; (3) scan every cleartext byte that Lore will store, index, render in derived caches, or export; (4) quarantine content whose complete scan fails; (5) tune false positives without storing allowlisted values.

**Non-goals:** we are **not** a vault, a rotation tool, or a proof that content is secret-free. The canonical local archive preserves faithful cleartext where the source is cleartext, so it may duplicate a secret already present in an agent log. We warn and avoid amplifying exposure. Scanning is local and never decodes opaque/encrypted agent fields.

## 2. Detection model — hybrid (DECISION)

Two complementary detectors, results merged and de-overlapped:

1. **Rule matchers (regex + fixed prefixes)** — high precision for known provider formats. The backbone.
2. **Entropy heuristic** — Shannon entropy over candidate tokens to catch *unknown* high-entropy secrets (generic API keys/passwords) the rules miss. Gated by context to limit noise.

**Why hybrid:** rules alone miss bespoke/opaque secrets; entropy alone is noisy (hashes, UUIDs, base64 blobs). Together: rules give precision on the important stuff, entropy gives recall on the rest.

## 3. Rule set (initial)

> Patterns below are **well-known public formats** (the same shapes used by open scanners like gitleaks/trufflehog). Treat as a **starting point to validate**, not final. If we vendor a third-party rule *file* wholesale, do a license check first and record it here (see `OPEN_SOURCE_PROJECTS.md`). Regexes are illustrative; finalize + fixture-test each.

| Rule id | Target | Pattern sketch | Severity |
|---|---|---|---|
| `aws-access-key-id` | AWS access key | `\b(AKIA|ASIA|AGPA|AIDA|AROA|ANPA)[A-Z0-9]{16}\b` | high |
| `aws-secret-key` | AWS secret | 40-char base64 near `aws`/`secret` context + entropy | high |
| `gcp-api-key` | Google API key | `\bAIza[0-9A-Za-z\-_]{35}\b` | high |
| `gcp-sa-key` | GCP service-account JSON | `"type"\s*:\s*"service_account"` + `"private_key"` | critical |
| `github-token` | GitHub PAT/OAuth/app | `\b(ghp|gho|ghu|ghs|ghr)_[A-Za-z0-9]{36,}\b` | high |
| `github-fine-grained-pat` | GitHub fine-grained PAT | `\bgithub_pat_[A-Za-z0-9_\-]{20,}\b` | high |
| `gitlab-pat` | GitLab PAT | `\bglpat-[A-Za-z0-9\-_]{20,}\b` | high |
| `slack-token` | Slack | `\bxox[baprs]-[A-Za-z0-9-]{10,}\b` | high |
| `stripe-key` | Stripe live/restricted | `\b(sk|rk)_live_[A-Za-z0-9]{16,}\b` | critical |
| `openai-key` | OpenAI | `\bsk-[A-Za-z0-9]{20,}\b` (+ `proj`/`svcacct` variants) | high |
| `anthropic-key` | Anthropic | `\bsk-ant-[A-Za-z0-9\-_]{20,}\b` | high |
| `google-oauth-secret` | OAuth client secret | `\bGOCSPX-[A-Za-z0-9\-_]{20,}\b` | high |
| `npm-token` | npm | `\bnpm_[A-Za-z0-9]{36}\b` | high |
| `jwt` | JWT | `\beyJ[A-Za-z0-9_-]{10,}\.[A-Za-z0-9_-]{10,}\.[A-Za-z0-9_-]{10,}\b` | medium |
| `private-key-block` | PEM private key | `-----BEGIN (RSA|EC|OPENSSH|PGP|DSA)? ?PRIVATE KEY-----` | critical |
| `generic-assignment` | `password=`, `api_key=`, `token:` … | `(?i)(password|passwd|secret|api[_-]?key|access[_-]?token|auth[_-]?token)\s*[:=]\s*['"]?[^\s'"]{8,}` + entropy gate | medium |
| `slack-webhook` / `discord-webhook` | webhook URLs | `hooks.slack.com/services/…` / `discord(app)?.com/api/webhooks/…` | medium |
| `connection-string` | DB URIs w/ creds | `(?i)\b(postgres|mysql|mongodb(\+srv)?|redis|amqp)://[^:@\s]+:[^@\s]+@` | high |

Extensible: rules are data (a versioned rules file), not code, so adding one is a fixture + a row.

## 4. Entropy heuristic (DECISION)
- Tokenize candidate strings (length ≥ 20, restricted alphabets: base64/base64url/hex).
- Compute **Shannon entropy per char**; flag candidates above a single threshold (4.0 bits/char today). Hex-only tokens are skipped outright (git SHAs, UUIDs, hex hashes) rather than given a separate lower threshold.
- **Context gating to cut noise:** boost if near assignment keywords (`key`, `token`, `secret`, `password`); **suppress** obvious non-secrets — git SHAs (40/64 hex), UUIDs, content hashes, known blob/data-URI prefixes, file paths.
- Entropy hits are **lower severity** than rule hits and more aggressively allow-listable.

## 5. False-positive control (DECISION)
- **Allowlist:** documented **test/example keys** (e.g. Stripe's public test keys, `AKIAIOSFODNN7EXAMPLE`, `wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY`), placeholder patterns. Today it is a **compile-time constant** in `secrets.rs` (assembled from split literals); a settings-driven allowlist is planned.
- **Context suppressors:** fenced code marked as sample; obvious hashes/UUIDs; the `.env.example` shape.
- **Per-rule enable/disable + severity floor** in settings: planned (no settings backend yet).
- **Never store the raw secret in the allowlist** — store a fingerprint + rule id. Findings and allowlist entries match on a keyed FNV-1a fingerprint (non-cryptographic; used for matching/dedup only, never to reconstruct a value), not a salted hash.

## 6. What gets scanned (DECISION)
- Every persisted cleartext MessagePart, FileEvent patch, and cleartext Blob is scanned, including thinking/reasoning even when it is not searchable. Tool-call content is scanned through the MessagePart projection that mirrors it (the tool-use part's `content_json` and the tool-result part's `text`); the denormalized `tool_call.input_json`/`output_text` columns are persistence-only mirrors with no search/export/UI read path, and are never indexed or exported.
- Scanning happens **before** a SearchDocument, exportable derived artifact, rendered cache, or content-bearing log entry can be produced.
- Large fields are streamed through the complete detector. There is no head+tail shortcut. Until completion, `Blob.scan_state=pending` and the content is unavailable to search/export; a scanner failure becomes `failed_quarantined` with a content-free diagnostic.
- **Excluded from scanning and all derived surfaces:** Codex `encrypted_content` and other opaque regions. They are stored only as `opaque_excluded` blobs if fidelity requires it and are never decoded, indexed, previewed, or exported.

## 7. Outputs & behavior
- Emits `SecretFinding{session_id, source_kind, source_id, field, rule, span_start, span_end, severity, value_fingerprint, disposition}`. Store offsets and a keyed fingerprint, never another cleartext copy of the value.
- **Default posture (from SECURITY):**
  - **Index:** SearchDocument contains a deterministic mask, never the flagged raw span. Because detection is heuristic, the UI describes this as “flagged secrets are redacted,” not “secrets never resurface.”
  - **Export:** redaction-aware; flagged spans masked as `«redacted:<rule>»` unless the user explicitly overrides ("include secrets").
  - **Logs:** application logs never accept raw archive fields at any level; diagnostics use ids, sizes, rules, and content-free errors.
  - **UI:** currently a count badge ("N secrets detected" per session) with canonical text rendered faithfully; the **planned** inline per-span badge (🔑/amber) + reveal-on-explicit-action (with a warning) is not yet implemented. Flagged content is excluded from search/export regardless (see §6); masking the *rendered* session view needs new IPC fields + a reveal toggle and is a product decision, not yet shipped.
- **Optional value-add (Claudoscope-style):** "your session logs contain N secrets — consider rotating / hardening `~/.claude`."

## 8. Performance and backpressure (DECISION)
- Target: a few milliseconds for typical fields, measured separately from large blobs. Compile rules once, use prefix/context prefilters, and scan streaming chunks with overlap sufficient for the longest matcher.
- Bound memory and worker concurrency, **not coverage**. Large fields may slow their own availability while the rest of the session ingests.
- Scanner/rule versions are recorded. A rule update schedules a complete rescan of canonical cleartext before rebuilding affected SearchDocuments.

## 9. Testing (ties to `TESTING.md` §7)
- **Positive corpus:** one **known-fake** secret per rule (documented test keys / synthetic-but-format-valid), asserted flagged with correct rule + span.
- **Negative corpus:** git SHAs, UUIDs, base64 images/data-URIs, `.env.example` placeholders, high-entropy hashes → asserted **not** flagged (precision guard).
- **Leakage tests:** after ingest, inspect SearchDocument/FTS, default export, rendered cache, and logs for each planted value → **must be absent**. The canonical archive is expected to retain the planted cleartext and must be protected by the filesystem threat boundary.
- **Coverage tests:** place a planted secret across streaming chunk boundaries and in the middle of a multi-megabyte blob; both must be found. A forced scanner failure must quarantine the blob from search/export.
- **Allowlist test:** an allow-listed example key is not flagged.
- Regressions: every false-pos/neg reported becomes a corpus entry.

## 10. Open questions (OPEN)
- Ship our own minimal Rust scanner vs. vendor a rule set (license + maintenance tradeoff)? Recommend: **own engine, curated rules seeded from public formats**, license-checked.
- Do we offer a "scan-only, no redaction" mode for users who want full-fidelity local search and accept the risk? (Default stays redact-on.)
- Auto-suggest rotation links per provider? (Nice-to-have; keep local, no network.)
- Confidence scoring per finding (rule=high, entropy=variable) surfaced in UI?
