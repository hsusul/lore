<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="assets/brand/lore-mark-light.svg">
    <img alt="Lore" src="assets/brand/lore-mark-dark.svg" width="88">
  </picture>
</p>

# Lore

Lore is a local, private archive of your Claude Code and Codex sessions.

It reads the session logs those agents already keep on disk. It links each session to the git repository it worked in. You search and read the archive in a desktop app, or from the command line with `lorectl`. Agents can search their own past sessions through `lorectl mcp`.

The longer goal is long-term memory for AI agents. Today Lore is the archive that memory would be built on.

## Status

Lore is early. There is no signed build. The newest GitHub release, v0.1.1, predates the current app, and its macOS `.dmg` is unsigned.

Memory features are planned, not built. `lorectl mcp` can search and read past sessions. It has no memory store, no extraction, and no write tools.

## How it works

```text
~/.claude and ~/.codex session logs   (read only)
        |
scan: parse each session, scan its text for secrets, read git evidence
        |
local archive: SQLite database, blobs, backups
        |
redacted search index, full-text, with filters
        |
desktop app  ·  lorectl  ·  lorectl mcp
```

- **Scan.** Lore finds session files in `~/.claude/projects` and in `~/.codex/sessions` and `~/.codex/archived_sessions`. `CLAUDE_CONFIG_DIR` and `CODEX_HOME` move those roots, and you can add folders in Settings. It never writes to them. A second scan of unchanged files ingests nothing, and a session that grew by appending is parsed from the new lines only.
- **Archive.** The archive is a SQLite database named `lore.db`, with `blobs` and `backups` directories beside it. By default it lives in the platform data directory under `dev.lore.app`. `LORE_ARCHIVE_DIR` or `--archive` moves it. The desktop app and `lorectl` open the same archive.
- **Redacted search index.** Before text is indexed, Lore scans it for secrets with fixed rules for known provider key formats plus an entropy check. Flagged spans are masked. Search, snippets, and default exports use the masked text only. The archive itself still holds the original text so you can read a session, and so does the original log. Redaction is a safeguard, not a promise that nothing secret remains.
- **Git evidence.** Lore records which branch and commit a session saw, and keeps agent-recorded facts apart from facts Lore observed in the repository itself. Each is labeled with its source. For files an agent wrote, Lore checks whether the exact content reached a commit, using git object ids, so the answer survives a rebase or squash. It reports what it found and what it could not check. It never fetches, checks out, or changes a repository. Remote URLs are stored without credentials.
- **Nothing leaves the machine.** The archive code, `lorectl`, and the desktop shell have no network code. CI scans the sources of all three for networking APIs. For the archive code and `lorectl` it also audits the dependency graph for networking crates and runs their tests with the network denied. The app does not check for updates. One thing to know. When you connect `lorectl mcp` to an agent, the redacted text it returns goes to that agent's model provider, as any tool result does.

## The desktop app

On first launch the app lists the Claude Code and Codex folders it found and offers a scan. After that it scans at launch, watches those folders, and adds new sessions as they are written. If a `lorectl scan` is running, the app opens read-only and does not ingest until it finishes. Restart it then.

**Search.** The query line sits at the top. Plain words are searched as text. Filters narrow the search. The app needs at least one word. A query of filters alone, such as `agent:codex`, lists the matching sessions newest first in `lorectl search` and `lorectl mcp`, but not yet in the app.

| Filter | Matches |
| --- | --- |
| `agent:codex` | Sessions from one agent (`claude-code` or `codex`) |
| `path:src/auth/` | Sessions that touched a file under this path |
| `tool:Bash` | Sessions that called this tool |
| `model:sonnet` | Sessions that ran on a matching model |
| `branch:main` | A git branch seen in the session |
| `commit:1a2b3c` | A commit sha prefix |
| `worktree:feat` | A git worktree id |
| `git-source:lore_captured` | Who saw the git evidence |
| `repo:lore` | A repository, by id, display name, or the last part of its path |
| `after:2026-07-01`, `before:2026-08-01` | Sessions started on or after, or before, a date |
| `has:error` | A tool call failed |
| `has:patch` | The session recorded a patch |

Results sort by best match, newest, or oldest.

**Browse and scope.** The scope chip on the query line picks what the list shows. It can be all sessions, one repository, or one folder. With a repository chosen, a search covers that repository only. A folder does not narrow a search yet. Folders are yours. Create one from the command palette and file sessions into it from the reader.

**Read.** A session opens in a reader with three tabs. Conversation shows the messages, with tool calls collapsed, and highlights your search words. Files shows each recorded file change as a diff. Git shows the evidence with its source and timing. The header shows the agent, model, length, message and tool call counts, tokens, and estimated cost, and how many secrets were flagged.

**Act on a session.** Copy it as Markdown, save it as a file, file it in a folder, or forget it. Exports mask flagged secrets. Forgetting removes the session and what was derived from it from the archive. The original log stays where it is.

**Settings.** Sources lists each agent's folders and lets you add or remove one. Backups sets an automatic schedule (off, daily, or weekly), how many copies to keep, and a button to back up now. Privacy states the local-only boundary. Forget everything removes every session, repository, finding, and backup from Lore.

**Keyboard.**

| Keys | Action |
| --- | --- |
| `/` | Focus the query line, when you are not typing in a field |
| `Cmd+K` (`Ctrl+K`) | Open or close the command palette |
| `Cmd+,` (`Ctrl+,`) | Open Settings |
| `Down` or `Enter` in the query line | Move to the results list |
| `Esc` in the query line | Clear the query, or move to the list when it is empty |
| `Up`, `Down` or `k`, `j` in the list | Move through sessions |
| `PageUp`, `PageDown`, `Home`, `End` in the list | Jump through the list |
| `Enter` in the list | Open the session in the reader |
| `k` or `Up` on the first result | Return to the query line |
| `Left`, `Right` on the reader tabs | Switch between Conversation, Files, and Git |
| `Esc` in the reader | Return to the list |

The command palette jumps to a repository, folder, or session, runs a full-text search of the archive, and runs actions. The actions are rescan, search, browse all, new folder, and settings.

## lorectl

`lorectl` is a second front end over the same archive. It is a separate binary, so you can use it without the desktop app.

```bash
cargo build --release -p lorectl
target/release/lorectl --help
```

| Command | What it does |
| --- | --- |
| `lorectl scan` | Ingest new agent sessions into the archive. It is the only command that writes. |
| `lorectl status` | Summarize archived work against the current repository. It counts the sessions linked to it, says when the last scan finished, and flags files an agent wrote that are staged and not committed. |
| `lorectl inspect <SESSION_ID>` | Show one archived session in detail. `--patch` also prints each recorded patch. |
| `lorectl search <QUERY>` | Full-text search across archived sessions, with the same filters as the app. |
| `lorectl hook <EVENT>` | An entry point an agent runs, not a person. `session-start` prints one line when agent-written files are staged and not committed, and nothing otherwise. It does not scan, and it always exits 0. |
| `lorectl mcp` | An MCP server on stdin and stdout, run by an agent, not a person. |

`lorectl report` is a reserved name. This build does not implement it.

Options may come before or after the command.

| Option | Meaning |
| --- | --- |
| `--archive <DIR>` | Archive directory to use. It must be absolute, and it names the directory, not the database file. |
| `--json` | Machine-readable output where a command supports it. |
| `--patch` | With `inspect`, also print each recorded patch. |
| `-h`, `--help` | Print help. |
| `-V`, `--version` | Print the version. |

Without `--archive`, `lorectl` uses `LORE_ARCHIVE_DIR`, then the platform data directory. Exit codes are 0 for ok, 1 for a usage error, 2 for no archive at that location, 3 for not inside a Git repository, 4 for an unreadable archive, 5 for an archive another Lore process is writing to (the one failure worth retrying), and 6 for a scan that did not complete. `lorectl --help` has the full table.

The query is one argument, so quote it. The examples assume `target/release/lorectl` is on your `PATH`.

```bash
lorectl scan
lorectl search "retry agent:codex"
lorectl inspect <SESSION_ID> --patch
lorectl status
```

### Let an agent search its own past sessions

`lorectl mcp` speaks the Model Context Protocol over stdio. To add it to Claude Code, give the absolute path to the binary.

```bash
claude mcp add lore -- /absolute/path/to/lorectl mcp
```

This is the form `claude mcp add --help` documents for a stdio server. Run `lorectl scan` first, or the tools report that there is no archive yet. To use a non-default archive, put `--archive /absolute/dir` before `mcp`. Other MCP clients that start a stdio server can run the same command. Only the Claude Code form has been checked.

The server offers two tools.

- `search_sessions` searches the archive with the filters above. It returns up to 20 hits per call, each with a session id and a redacted snippet, and takes `sort` and `cursor` arguments.
- `read_session` returns one session in order. It gives the title, each message labeled by role, and the recorded patches. It is paged at 24,000 characters, and a cursor fetches the next page. It leaves out thinking and raw tool payloads.

Both tools are read-only and read only the redacted index. A message longer than one page is cut, and the rest cannot be fetched. No tool returns the cleartext title, because a fallback title can echo the first user message.

## Development

You need Rust, Node.js 20, the Xcode command-line tools, and the Tauri 2 prerequisites for macOS. `rust-toolchain.toml` pins the Rust toolchain, so rustup selects it. CI lints with `-D warnings`, and the pin keeps a new clippy lint from breaking an unrelated commit.

Run the desktop app with `npm run tauri dev`. Build the web UI alone with `npm run build`.

These are the commands CI runs on each pull request.

```bash
# Rust core
./scripts/toolchain-pin-check.sh
cargo fmt --all -- --check
cargo clippy -p lore-core -p lore-ipc -p lorectl --all-targets --all-features -- -D warnings
cargo test --locked -p lore-core -p lore-ipc -p lorectl --all-targets
cargo test -p lore-core --test no_network_in_archive
cargo test -p lorectl --test no_network_in_lorectl
./scripts/egress-check.sh
cargo test -p lore-ipc
git diff --exit-code crates/lore-ipc/bindings

# UI
npm ci
npm run lint
npm run typecheck
npm test

# Desktop shell
npm run build
cargo clippy -p lore-app --all-targets -- -D warnings
cargo build --locked -p lore-app
cargo test -p lore-app --test no_egress
```

`./scripts/egress-check.sh` needs a sandbox wrapper for its network-denied run (`sandbox-exec` on macOS, `unshare` or `bwrap` on Linux). A weekly CI job also runs a search benchmark at about a million messages.

### Run with sample data

`npm run dev` serves the UI at http://localhost:5173 in a plain browser. There is no Tauri shell behind it, so the page uses a built-in sample archive of 122 made-up sessions in 4 repositories. Search, the reader, folders, settings, backups, and rescan all work against that sample. It reads no files and writes none. A production build contains none of it.

### Run against a fake archive

`scripts/synth-archive.sh` writes a deterministic fake Claude Code and Codex profile.

```bash
scripts/synth-archive.sh /tmp/lore-fake 30 20 7   # <out_dir> <claude_count> <codex_count> <seed>
```

The same arguments always produce the same files. Agent homes land in `<out_dir>/claude/projects` and `<out_dir>/codex/sessions`. Point the tools at them and at a scratch archive.

```bash
export CLAUDE_CONFIG_DIR=/tmp/lore-fake/claude
export CODEX_HOME=/tmp/lore-fake/codex
export LORE_ARCHIVE_DIR=/tmp/lore-fake-archive
lorectl scan
lorectl search health
```

To run the desktop app against the same profile, set the archive directory and the two debug-only roots. `npm run tauri dev` reads `LORE_DEV_CLAUDE_ROOT` and `LORE_DEV_CODEX_ROOT` in debug builds only. Release builds ignore them.

```bash
LORE_ARCHIVE_DIR=/tmp/lore-fake-archive \
LORE_DEV_CLAUDE_ROOT=/tmp/lore-fake/claude/projects \
LORE_DEV_CODEX_ROOT=/tmp/lore-fake/codex/sessions \
npm run tauri dev
```

Never point a development run at your real history. Without these variables the app and `lorectl` read `~/.claude` and `~/.codex` and write to your real archive. Set the variables first, and check the path before you scan.

## Repository layout

```text
crates/lore-core/   archive library: discovery, parsing, ingest, secret scanning, search, git evidence
crates/lore-ipc/    Rust IPC types and generated TypeScript bindings
crates/lorectl/     the lorectl command line and MCP server
src-tauri/          Tauri 2 desktop shell (crate lore-app)
src/                React UI (src/mock holds the sample archive for npm run dev)
scripts/            synth-archive.sh, egress-check.sh, toolchain-pin-check.sh, p1-restore.sh
assets/brand/       logo files
```

`scripts/p1-restore.sh` is the one-off script that restored the archive app. It is kept so reviewers can rerun it.

## Scope

Lore archives, searches, and reads. It does not run agents, control them, or edit your code. It opens agent logs read-only and never changes them. It has no accounts, no telemetry, and no server.

## License

[Apache License 2.0](LICENSE)

Lore is not affiliated with Anthropic or OpenAI.
