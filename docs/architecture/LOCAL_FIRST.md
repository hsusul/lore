# Local-First Architecture

> Why "local-first" is a hard architectural constraint for Lore, what it buys us, and the rules that keep us honest. Companion: `SECURITY.md`, ADR-0005.

## 1. Definition (for this project)

Local-first here means, concretely:
1. **All primary data lives on the user's machine** (SQLite + blobs under app-data). No hosted DB, no server of record.
2. **The app is fully functional offline**, forever, with no account.
3. **No data crosses the network in the default configuration.** The only V0 network capability is a signed update check, off by default and invoked manually or enabled explicitly.
4. **The user owns and can inspect/delete everything.**

This is stronger than "local-first sync apps" (which assume an eventual server). Lore V0 has **no** server component at all.

## 2. Why this is the right architecture (not just a value)

- **The data is uniquely sensitive** (source + secrets). The safest place for it is where it already is.
- **The data is already local.** Agents write logs to disk; Lore reads them in place. A cloud would mean *copying* sensitive data off-machine to do something you can do locally.
- **It's a differentiator.** Claudoscope and SpecStory both lead with local/zero-telemetry; the market rewards it (`COMPETITIVE_LANDSCAPE.md` §6). Being *account-free and offline* is a sharper version of that stance.
- **It's simpler and cheaper.** No infra, no auth, no multi-tenant security, no uptime, no data-processing liability. One laptop is the whole system (global product rule).

## 3. What we explicitly avoid (global product rules, restated)
No Kubernetes. No microservices. No cloud backend. No authentication system. No hosted database. No mandatory server. Nothing here is justified by "scalability" for a single-user desktop app.

## 4. Consequences & how we handle them

| Local-first consequence | Handling |
|---|---|
| No server to run heavy compute | Do it locally in Rust; keep it cheap (FTS not embeddings in V0); background jobs with backpressure |
| No cloud search across devices | Per-machine archive in V0; cross-device is a *future* opt-in, E2E-encrypted sync (own ADR), never a default |
| No central telemetry to learn from | Accept it. Improve via OSS issues, local repro, fixtures — not by watching users |
| Data durability is local | Lore's canonical archive may outlive source logs; maintain bounded local backups and recovery/salvage, while warning that external backup/sync copies are outside Lore's control |
| Updates need a channel | Signed Tauri updater over a documented release endpoint; off by default; request contains only version/platform/architecture/channel |
| Multi-agent formats change | Adapters + fixtures; degrade gracefully; re-scan surviving source logs when possible, but never assume they are retained |

## 5. The single allowed network call
`update check` → a documented HTTPS release-manifest endpoint, sending only app version, platform, architecture, and release channel. Scheduled checks are off until explicitly enabled; a manual check is an explicit network action. The updater is capability-separated from archive modules. Everything else in V0 is airgap-clean.

## 6. Portability without lock-in
Local-first ≠ platform-locked. We deliberately chose **Tauri** over native Swift (which Claudoscope used) precisely so the local-first app can run on Windows/Linux later (ADR-0001). "Local" is about *where data lives*, not *which OS*.

## 7. How a reviewer verifies the claim
- Launch offline → full functionality (scan, browse, search) works.
- Network monitor (Little Snitch / `lsof -i`) → no connection in the default workflow; an explicit update check reaches only the documented endpoint.
- CI capability/dependency guard (static call-site scan of `lore-core/src` today) is green; the OS-level deny-egress integration test is planned (ROADMAP M7) and the updater is the only network-capable module.
- App-data dir is `0700`, DB `0600`, outside any repo and outside known cloud-sync roots (or a warning is shown).
