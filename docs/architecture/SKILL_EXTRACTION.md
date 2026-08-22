# Skill / Knowledge Extraction

> **Status: design for V0.5** (deferred from V0 for privacy reasons, *not* naming — see PRD §5). This is the feature where Lore competes most directly with SpecStory's "Lore," so the design must earn the comparison. Naming decision (2026-08-10): **keep "Lore"** and win by being **git-evidenced**. Companion: `DATA_MODEL.md` (Skill/SkillSource), `SECURITY.md` (LLM data-flow), `GIT_INTEGRATION.md`, `SEARCH.md`. Tags: **DECISION** / **INFERENCE** / **OPINION** / **OPEN**.

## 1. The job

Turn a good session (or a cluster of related sessions) into **reusable, trustworthy knowledge** — a `SKILL.md` (or a lighter "knowledge note") the user can drop into any agent — so they stop re-teaching the same thing every session.

This is the brief's original headline. We defer it past V0 (the archive is valuable first; synthesis needs an LLM that strains the privacy promise) but we intend to **out-execute** the incumbent, not skip it.

## 2. How this differs from SpecStory's "Lore" (the whole point)

| Dimension | SpecStory "Lore" (FACT/INFERENCE) | Lore's approach (DESIGN) |
|---|---|---|
| Evidence | Mines `.specstory/history` Markdown for demonstrated patterns | **Git-evidenced:** every claim links to a specific `Message`/`ToolCall`/`FileEvent`/**diff** with the branch+commit it happened on |
| Source of truth | Per-project Markdown files | Normalized cross-agent DB with git identity → can synthesize from **multiple sessions across agents/worktrees** about the same code path |
| Verifiability | Generated skill text, user-approved | Each bullet is **traceable and re-openable** in the archive; "show me the session/diff this came from" |
| Selection | User invokes on a session/history | Lore can **suggest candidates** (repeated fixes on the same path, recurring tool sequences, high-signal sessions) |
| Privacy | Local; cloud opt-in | **Explicit per-run data-flow**: template-only / local model / BYO-key with preview-before-send |

**OPINION:** "git-evidenced + cross-session + suggested candidates + verifiable provenance" is the defensible wedge over a Markdown miner. If we can't be materially more trustworthy/precise, we shouldn't ship this feature at all.

## 3. Pipeline

```
CANDIDATE           SELECT              SYNTHESIZE            REVIEW              EXPORT
sessions ──▶ rank/cluster ──▶ assemble ──▶ (optional model) ──▶ user edits ──▶ SKILL.md
(evidence)   (heuristics)     evidence      draft w/ citations   + approves       + provenance
                              bundle
```

### 3.1 Candidate selection (no LLM — pure heuristics + search)
Surface promotion-worthy material from the archive:
- **Repeated work on the same code path:** ≥N sessions with `FileEvent.path` overlap on one repo (e.g. "you've fixed `billing/verify.ts` 4 times").
- **Recurring tool sequences / commands** that ended in success (a reusable procedure).
- **High-signal single sessions:** long, tool-heavy, ended with passing tests / a commit, low error-rate tail.
- **User-initiated:** "promote this session" from the session view, or a multi-select cluster.
- Ranked, explained ("why suggested"), dismissible. All computed from existing indexes — cheap, local, private.

### 3.2 Evidence bundle assembly (no LLM)
For a chosen candidate, build a structured, **bounded** evidence object — the input to synthesis and the provenance record:
```jsonc
EvidenceBundle {
  repo: { identity_key, display_name, identity_confidence },
  scope: { paths: [...], branch?, commit_range? },
  sessions: [{ id, agent, when, title }],
  steps: [                        // ordered, deduped, high-signal
    { kind: "prompt"|"decision"|"edit"|"command"|"result",
      ref: { session_id, message_id?, tool_call_id?, file_event_id? },  // ← re-openable
      summary, diff_snippet?, redacted: bool }
  ],
  secrets_present: bool           // gate synthesis if true (see §5)
}
```
- Uses `GitObservation`/`FileEvent` so each claim carries the strongest available evidence and its provenance: agent-recorded patch/commit when present, otherwise explicitly labeled capture-time context. A skill must not upgrade retrospective Lore capture into session-time fact.
- **Secret-scanned and redaction-applied before it can reach any model** (`SECURITY.md`).
- Size-bounded (token budget) — select the highest-signal steps, log what was dropped (no silent truncation).

### 3.3 Synthesis (LLM optional — the privacy fork)
Three modes the user picks per run (**OPEN — which ship in V0.5; recommend template + BYO-key first**):
1. **Template-only (no LLM, always available, 100% local):** deterministically render the evidence bundle into a structured `SKILL.md` skeleton (title, when-to-use, steps with citations, gotchas pulled from error→fix transitions). Lower prose quality, zero data leaves.
2. **Local model:** run a small local LLM (e.g. via a bundled/known runtime) over the bundle. Fully local; heavier install; quality between template and frontier.
3. **BYO-key (cloud):** user supplies their own API key; **before any send**, show a **preview of the exact payload** (post-redaction) and require explicit consent. This is the one path where data leaves the machine — treated as a first-class SECURITY data-flow (its own review, off by default, never triggered by observed content).

Whatever the mode, output is **provenance-tagged**: which mode/model, what was sent (or "nothing — local/template"), timestamps.

### 3.4 Review & approve
- Two-pane editor (see `docs/design/WIREFRAMES.md` *(internal)* §7): editable `SKILL.md` on the left; **Evidence** on the right where every citation is clickable → jumps to the exact message/tool-call/diff in the archive.
- User edits freely; **claims without a citation are visually flagged** (encourage evidence-backing).
- Status: `draft → approved → exported`.

### 3.5 Export
- Write a plain **`SKILL.md`** to a user-chosen path or a **canonical skills folder**; let **Skillsync** fan it out into each agent's format (don't reimplement per-agent formatting — see `docs/research/OPEN_SOURCE_PROJECTS.md` *(internal)*).
- Optionally also emit the `EvidenceBundle` as a sidecar (JSON) for auditability.
- Export is **redaction-aware** (never ship flagged secrets).

## 4. `SKILL.md` output shape (proposed)
```markdown
---
title: Verifying Stripe webhook signatures on prod
repo: ipay-prod
scope: [billing/stripe/verify.ts]
provenance: 3 sessions (2 Codex, 1 Claude), branch billing, commits 3ab9f1…c72d
generated_by: byo-key:<model>   # or "template" / "local:<model>"
lore_evidence: .lore/evidence/<id>.json
---

## When to use
… (one-liner conditions)

## Steps
1. Use the `stripe-signature` header (casing differs behind the proxy). — evidence: session#1 · Edit verify.ts (+18 −6)
2. Verify before parsing the body. — evidence: session#2 · thinking
3. Regression test in `verify.test.ts`. — evidence: session#3 · Bash npm test ✔

## Gotchas
- 400s on prod trace to header casing, not the secret. — evidence: session#1 · tool_result
```
Every substantive line ends in a **traceable citation**. That traceability is the product.

## 5. Privacy contract (this feature is the main watchlist item)
- Modes 1–2 are **airgap-clean**; only Mode 3 (BYO-key) sends data, and only after an explicit **preview-before-send** of the redacted payload.
- **Hard gate:** if `EvidenceBundle.secrets_present`, block cloud synthesis until the user redacts/acknowledges.
- Never send `encrypted_regions` or `thinking` (unless the user explicitly opts in for a specific run).
- Shipping any of this requires the **SECURITY.md data-flow review + an ADR** (per `AGENTS.md` constraints). Off by default.
- No telemetry on skill content, ever.

## 6. Data model touchpoints
Uses `Skill`, generic `SkillSource{source_kind,source_id}`, `GitObservation`, and `SecretFinding` from `DATA_MODEL.md`. Any evidence-bundle blob or generation metadata added in V0.5 requires a migration and canonical data-model update.

## 7. Risks
- **Hallucinated/incorrect skills** (esp. cloud/local LLM) → mandatory citations + user review + flag-uncited-claims; template mode as a trustworthy floor.
- **Over-generalization** from one repo's quirk → scope to repo/paths; show evidence breadth (N sessions) so thin evidence is visible.
- **Privacy slip via the model** → the §5 gates; preview-before-send; default off.
- **Competing on prose vs SpecStory** → don't; compete on **evidence + verifiability + cross-session reach**.
- **Scope creep toward "AI notes app"** → skills are always evidence-bound to sessions (NON_GOALS).

## 8. Open questions (OPEN)
- Which synthesis modes ship first? (Recommend **template + BYO-key**; add local model once a runtime is chosen.)
- Do we auto-suggest candidates proactively (a "Promotable" inbox) or only on user action? (Recommend suggest, but quietly.)
- Knowledge notes vs full `SKILL.md` — one type with a length spectrum, or two? (Lean: one `Skill` entity, variable depth.)
- How much diff context is safe/useful to embed by default? (Bounded; user-expandable; redaction-first.)
