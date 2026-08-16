# Lore — Product Requirements (V0)

> Status: draft for build. Owner: founding team. Companion docs: `VISION.md`, `ROADMAP.md`, `NON_GOALS.md`, and the architecture set under `docs/architecture/`.
> Tags used below: **DECISION** (a committed choice), **OPEN** (needs human sign-off — see `RESEARCH_SUMMARY.md`).

## 1. Product thesis

> **Your coding agents forget. Lore doesn't.**

Coding agents (Claude Code, Codex, and others) do real, valuable work inside your repos every day — decisions, fixes, dead-ends, and the *reasoning* behind them — and then that knowledge is stranded in per-tool log files no human ever reopens. **Lore is a local-first desktop app that ingests the sessions your agents already wrote to disk, anchors each one to the strongest provenance-labeled Git evidence available, and makes your entire agent history instantly searchable — with zero setup, no account, and no archive content leaving your machine.**

**Positioning (sharpened after competitive Scans #2–#3 — see `docs/research/COMPETITIVE_LANDSCAPE.md`):** Lore is **"git memory for coding agents"** — not "another session browser." Cross-agent local viewing is table stakes (CCHV and others do it); Lore's reason to exist is **git *depth***: preserve session-recorded commits and patches when present, label Lore's later ingest-time observation when they are absent, re-verify commits over rebases, and reconcile worktrees without hiding identity ambiguity. Git-evidenced knowledge sits on that trustworthy evidence base. The deliberate bet is to go **narrower and deeper** — Claude Code + Codex done with real git intelligence — rather than race breadth.

## 2. Primary user

**The individual, AI-heavy developer on macOS** who:
- runs Claude Code and/or Codex daily (often *many* sessions/day, increasingly across git worktrees),
- works across several repositories,
- has already lost useful solutions/decisions in agent scrollback, and
- is privacy-conscious about source code and prompts leaving their machine.

Secondary (later): small teams who want to *individually* mine their own history; power users orchestrating parallel agents (Superset/Conductor/Claude Squad) who generate lots of sessions.

**Not** the target for V0: enterprises needing SSO/RBAC/hosted search; non-coding "AI memory" users; people who want the agent to auto-remember facts going forward (that's forward-memory tools — see `COMPETITIVE_LANDSCAPE.md`).

## 3. Jobs-to-be-done

1. **"Find that thing."** *"Where did the agent figure out the Stripe webhook signing bug?"* — full-text search across all sessions, filtered by repo/branch/agent/date.
2. **"What happened in this repo?"** Open a repo and see a timeline of every agent session that touched it, newest first, grouped/rolled-up sensibly.
3. **"What code context can I prove?"** For any session, see recorded branch/commit/patch evidence, the repository state Lore observed at ingest, and a visible confidence/provenance label so historical context is useful without being overstated.
4. **"Recover the reasoning, not just the answer."** Read a clean, navigable transcript (prompts, tool calls, edits, results, token cost) without wading through raw JSONL.
5. **"Don't leak my secrets."** Be warned if a session/index contains API keys or credentials; never ship raw content anywhere by accident.
6. *(V0.5+)* **"Turn this into something reusable."** Promote a good session (or a cluster) into a curated, git-evidenced note or `SKILL.md`.

## 4. Core user loop

```
Agent writes a session to disk (Claude Code / Codex — already happening)
        │
        ▼
Lore DISCOVERS it (filesystem scan + FSEvents watch, zero config)
        │
        ▼
Lore NORMALIZES it (agent adapter → unified schema) and ANCHORS it to Git
    (repo/worktree identity evidence + recorded, captured, and reverified Git observations)
        │
        ▼
Lore INDEXES it (SQLite + FTS5) — incrementally, in the background
        │
        ▼
Developer RECALLS later: search / browse-by-repo / read-in-context
        │
        ▼  (V0.5+)
Developer PROMOTES a session into reusable knowledge / a SKILL.md
```

Improvement over the brief's loop: **normalization + git-anchoring are one explicit stage** (the differentiator), and **skill promotion is downstream of a fully useful archive** — the loop delivers value at "RECALL" even if "PROMOTE" never runs.

## 5. Killer feature (pick ONE)

**DECISION — Git-anchored retrospective recall from a zero-config first scan.**

The install-me moment: you download Lore, it scans the configured Claude Code and Codex roots, and within seconds you're looking at **months of your own agent history organized by repository, anchored to recorded Git/patch evidence where available and honestly labeled ingest-time context otherwise, searchable across both agents — with no archive content leaving your machine and no account to create.**

Why this one (OPINION, calibrated after Scans #2–#3):
- The intended **git depth** goes beyond branch filtering: an evidence model that separates agent-recorded values and patches from Lore observations, re-verifies recorded commits after rebase/GC, and reconciles worktrees using multiple identity signals. The current competitor scan found no equivalent end-to-end model, but treats that absence as a dated inference, not a universal fact. See `COMPETITIVE_LANDSCAPE.md` §6.
- Archive ingest/search needs **no LLM or network**. The separately bounded update check is off by default, so privacy is an enforceable architecture claim rather than an unqualified slogan.
- It produces an immediate "whoa" from data the user *already has*, and it's the on-ramp to the real payoff (git-evidenced skills).

**Explicitly NOT the V0 killer feature: "sessions → SKILL.md."** Rationale (DECISION to defer):
1. **Privacy tension (primary reason).** Good skill synthesis wants an LLM. A cloud LLM breaks "nothing leaves the machine"; a local LLM is a heavy V0 dependency. Better to ship the fully-local archive first and add skill promotion once we can do it *git-evidenced and privacy-preserving*.
2. **Value ordering.** The archive is useful on day one; skills are a payoff on top of it.
3. **Prior art.** SpecStory ships a feature named "Lore" doing sessions→skills. **Naming decision (2026-08-10): keep "Lore" for the product and this feature**, and win the comparison by being **git-evidenced** (skills trace to sessions/tool-calls/diffs) inside a git-aware archive — not by avoiding the feature. This is a *sequencing* deferral (privacy), not a retreat. Design: `docs/architecture/SKILL_EXTRACTION.md`.

## 6. V0 scope (must be realistically buildable)

**In:**
- **Agent adapters:** Claude Code + Codex (native log parsers). Adapter interface designed for extension (see `AGENT_ADAPTERS.md`).
- **Discovery + watch:** scan known locations; watch via `notify`/FSEvents for new/changed sessions; incremental re-index.
- **Normalization:** unified schema (`AgentSession/Message/ToolCall/FileEvent/...` — see `DATA_MODEL.md`).
- **Git anchoring:** ambiguity-aware repo/worktree identity; session-recorded branch/commit/patch evidence where present; capture-time branch/commit/dirty/file summary with observation timestamp and confidence; later commit re-verification. Enrichment via `gix` + a hardened, read-only `git` fallback.
- **Storage + search:** SQLite + FTS5 full-text search over messages/tool I/O with filters (repo, agent, branch, date, has-error, tool used).
- **UI:** desktop app (Tauri) with: onboarding/first-scan, Repositories view, Sessions list, Session detail (readable timeline), global search + command palette, Settings, Agents/integration status, graceful unsupported/error states.
- **Safety:** local secret scanning at index time with in-app warnings; DB in an app-private, permission-restricted location; **zero telemetry by default**; Markdown export of a single session (redaction-aware); Lore-owned local SQLite online backup create/restore wiring, reverse-order recovery fallback, and user-configurable backup retention and cadence controls.

**Out of V0** (see `NON_GOALS.md` and ROADMAP for where they land): skill extraction, semantic/embedding search, Cursor/Gemini/OpenCode adapters, cloud/sync/teams, Windows/Linux builds, any LLM calls.

## 7. V0.5 (natural follow-ups)
- Additional adapters: **Gemini CLI**, **OpenCode** (documented-ish formats).
- **SpecStory `.specstory/history` fallback adapter** (breadth for agents we don't natively parse).
- **Saved searches / smart filters**; cross-repo "when did I ever…" search.
- **Cost & activity analytics** (tokens/cost per repo/day/agent — cheap, loved).
- **Skill/knowledge promotion v1**, git-evidenced, **BYO/local model, opt-in** (see NON_GOALS for the privacy contract).

## 8. V1 (utility → serious tool)
- **Cursor adapter** (best-effort, experimental) once we accept the maintenance cost.
- **Semantic/hybrid search** (local embeddings, `sqlite-vec`) — only if keyword search proves insufficient (see `SEARCH.md`).
- **Read-only local MCP endpoint**: let an agent *query the Lore archive* ("have we solved this before?") — Lore as retrieval source, not injector.
- **Worktree/parallel-agent intelligence**: reconcile N worktrees ↔ 1 repo; ingest orchestrator-generated sessions with correct attribution.
- **Windows/Linux** builds (portability was preserved by the Tauri choice).

## 9. Success criteria (V0)
- **Time-to-wow < 60s:** from first launch to a searchable, repo-grouped archive of existing history, no config.
- **Fidelity:** a Claude Code / Codex session round-trips into a faithful, readable timeline (prompts, tool calls, edits, results, tokens) with correct repo/branch attribution.
- **Scale:** responsive (<200 ms search, smooth scroll) at **10k sessions / ~1M messages** on a typical laptop (see `docs/development/TESTING.md` perf targets).
- **Privacy:** verifiably zero outbound network in the default configuration; update checks are off until explicitly invoked or enabled.
- **Robustness:** malformed/partial/unknown-version sessions never crash ingest or the app.

## 10. Non-goals (summary — full list in `NON_GOALS.md`)
Not an IDE. Not an agent runtime/orchestrator. Not a forward-memory/RAG injector. Not a generic note-taking app. Not a hosted SaaS / team platform (V0). No accounts, no cloud, no telemetry by default. No LLM calls in V0.

## 11. Open decisions requiring human input
- **DECIDED — Naming (2026-08-10).** Keep "Lore" everywhere (product + skill feature); differentiate via git-evidenced skills; trademark/SEO check before public launch. (See `RESEARCH_SUMMARY.md` §9.1.)
- **OPEN — Skill-promotion privacy model** (BYO-key cloud LLM vs bundled local model vs template-only, no-LLM). Affects V0.5.
- **OPEN — Desktop framework** is provisionally Tauri (ADR-0001) — confirm given team skills.
