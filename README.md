<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="assets/brand/lore-mark-light.svg">
    <img alt="Lore" src="assets/brand/lore-mark-dark.svg" width="88">
  </picture>
</p>

# Lore

Lore is a local desktop app for running Claude Code and Codex in parallel.

Each task gets its own git worktree and branch, so agents never touch your checkout or each other. You watch live activity and diffs, continue a task or hand it to the other agent, then commit and merge when you are ready.

> **Status: early and unreleased.** The orchestrator and workbench are built and tested against a fake agent. They have not yet been proven end to end with signed-in agents, and there is no signed build.

## How it works

```text
you write a task
        ↓
Lore creates a worktree on branch lore/<task>
        ↓
claude -p / codex exec runs headless in that worktree
        ↓
live activity, diff, overlap warnings, owns-paths
        ↓
continue · hand off · commit · merge (or a merge queue)
```

- Agents launch with their own permission settings. Lore never passes permission-bypass flags; Claude runs in `acceptEdits` by default (opt-in `auto` per task), Codex in its `workspace-write` sandbox.
- You can pin a model and effort per task. Auto-handoff can move a run to the other agent when a usage limit hits.
- The overview groups every agent by what it needs from you. When Lore is in the background, an agent that finishes, fails, or needs you posts a local notification, and the dock badge counts the ones waiting on you.
- Your checkout only changes when you confirm a merge. Lore refuses to merge into a dirty checkout and aborts on conflict. A merge queue can land several finished tasks in order.
- Everything runs on your machine. Lore has no accounts, telemetry, or server; the agents talk to their own providers as they normally do.

## Development

You will need Rust, Node.js, the Xcode command-line tools, and the Tauri 2 prerequisites for macOS.

The Rust toolchain is pinned in `rust-toolchain.toml`, so rustup selects the right compiler automatically. CI lints with `-D warnings`, and pinning keeps a new clippy lint in a stable release from breaking the build on an unrelated commit.

```bash
npm install
npm run build
npm test
npm run lint

cargo test --workspace
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo fmt --all -- --check
```

Run the web UI with sample data using `npm run dev`. Run the desktop app with `npm run tauri dev`.

## Repository layout

```text
crates/lore-orchestrator/  tasks, worktrees, agent processes, handoff, merge
crates/lore-ipc/           Rust IPC types and generated TypeScript bindings
src-tauri/                 Tauri application layer
src/                       React workbench (npm run dev uses sample data)
crates/lore-core/          session-archive library (not used by the app)
crates/lorectl/            archive CLI (not used by the app)
```

## Scope

Lore supervises Claude Code and Codex. It is not a code editor; editing stays in your own editor. The workbench is an IDE-style shell for that job: agents and files in a sidebar, the selected run in the editor, diffs and a merge queue when you need them.

## License

[Apache License 2.0](LICENSE)

Lore is not affiliated with Anthropic or OpenAI.
