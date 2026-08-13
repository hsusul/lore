# Lore

Lore is a local desktop app for browsing and searching your coding-agent history.

It reads the session files Claude Code and Codex already save on your computer, connects them to their repositories and Git history, and puts everything in one searchable archive. It does not wrap your agents or change their files.

## How it works

```text
Claude Code and Codex logs
        ↓
read-only adapters
        ↓
local SQLite archive + Git evidence
        ↓
search and browse in the desktop app
```

Lore keeps different kinds of evidence separate. A commit recorded during a session is not treated as the same thing as repository state observed later during ingestion.

Everything in the archive stays on the machine. V0 has no accounts, telemetry, cloud database, or LLM calls.

## Development

You will need Rust, Node.js, the Xcode command-line tools, and the Tauri 2 prerequisites for macOS.

```bash
npm install
npm run build
npm test
npm run lint

cargo test --workspace
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo fmt --all -- --check
```

Run the web UI with `npm run dev`. Run the desktop app with `cargo tauri dev` once the Tauri CLI is installed.

## Repository layout

```text
crates/lore-core/   ingestion, storage, Git, search, and safety
crates/lore-ipc/    Rust IPC types and generated TypeScript bindings
src-tauri/          Tauri application layer
src/                React interface
```

## Scope

Lore currently focuses on Claude Code and Codex. It is an archive, not an IDE, agent runtime, or cloud memory service. More integrations and generated skills are deliberately deferred until the core archive is finished.

## License

[Apache License 2.0](LICENSE)

Lore is not affiliated with Anthropic or OpenAI.
