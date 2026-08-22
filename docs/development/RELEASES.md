# Releases & Distribution

> How Lore ships. macOS-first, open-source, avoiding platform lock-in. **Pre-implementation** — this is the intended strategy. Companion: ADR-0001, `SECURITY.md`, `LOCAL_FIRST.md`. Tags: **DECISION** / **OPINION**.

## 1. Channels (DECISION)
| Channel | When | Notes |
|---|---|---|
| **GitHub Releases** | primary from V0 | signed `.dmg` + checksums + release notes; source tarball |
| **Homebrew cask** | shortly after first tagged release | `brew install --cask lore` (own tap initially) |
| **Tauri update check** | V0, **off by default** | manual or explicitly enabled scheduled check; signed manifest; documented fields (see SECURITY §7) |
| Windows / Linux | V1 | Tauri already keeps this open (ADR-0001) |

**OPINION:** mirror how comparable OSS Tauri/Electron apps ship — GitHub Releases as source of truth, Homebrew for discoverability, signed auto-update for retention. No app store in V0 (sandboxing would fight our filesystem reads).

## 2. macOS signing & notarization (DECISION)
- **Developer ID Application** signing + **notarization** (`notarytool`) + **stapling** so Gatekeeper is happy without right-click-open.
- Hardened runtime; request only the entitlements we need (filesystem access to read agent dirs).
- Reading protected dirs may trigger **TCC** prompts — onboarding explains why; degrade gracefully if denied.
- Secrets (signing cert, notarization creds) live in CI secrets, never in the repo.

## 3. Auto-update security (DECISION)
- Tauri updater with a **pinned public key**; only signature-verified bundles install.
- Update manifest at the documented release endpoint; request carries app version, platform, architecture, and channel only.
- Scheduled checks are off on first run. Manual "Check for updates" is labeled as an explicit network action; archive functionality remains fully offline.

## 4. Versioning & branches
- **SemVer**; pre-1.0 while under development (breaking changes allowed, noted).
- `main` always releasable (ROADMAP milestone discipline); tags trigger release CI.
- **Migrations:** a release that changes the schema ships a forward migration + migration and backup/restore tests; never assume agent logs still exist to rebuild an archive.

## 5. Release pipeline (intended)
```
tag vX.Y.Z
  → CI: fmt/clippy/test + capability/dependency guard + OS-level egress tests + license audit + perf smoke
  → build signed+notarized .dmg (macOS)
  → generate updater manifest + checksums
  → publish GitHub Release (notes: features, migrations, known format risks)
  → update Homebrew cask
```

## 6. Release checklist (per release)
- [ ] All CI green incl. **privacy guards** (capability/dependency check, OS-level egress tests, secret-leakage, deletion, permissions).
- [ ] Fresh-machine install + first-run scan on a real profile (manual).
- [ ] Auto-update from previous version verified.
- [ ] Migrations tested up from the last release's DB.
- [ ] Notarization stapled; Gatekeeper opens cleanly.
- [ ] Release notes state **honest status** ("under development"), new adapters, and any known agent-format risks.
- [ ] Docs updated (`README` status, `DOCS_INDEX` if structure changed).

## 7. Honesty in distribution (OPINION)
Until V0 is real, the README/site must **not** fake screenshots or claim features exist (per brief). Mark clearly as under development; show real status. Trust is the product.
