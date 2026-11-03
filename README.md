<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="assets/brand/lore-mark-light.svg">
    <img alt="Lore" src="assets/brand/lore-mark-dark.svg" width="88">
  </picture>
</p>

# Lore

Lore keeps a local, searchable archive of your Claude Code and Codex sessions.
It gives coding agents long-term memory in a vault of plain markdown notes that you review.
It checks the skill folders your agents load for duplicates, drift, and broken references.

Three rules hold the design together.

- The vault is the truth. The index, the archive, and anything an agent sees are derived and rebuildable.
- Agents only propose. A person accepts. Agents never get delete.
- Every recalled note says whether the code it points at still matches. A stale note is labelled, not silently served.

## Get started

Lore is early. There is no signed build, so build from source. You need Rust and git. `rust-toolchain.toml` pins the toolchain, and rustup selects it.

```bash
cargo build --release -p lorectl
export PATH="$PWD/target/release:$PATH"
lorectl init
```

`lorectl init` makes the vault at `~/Lore`, scans the sessions of the last 30 days, and shows the lessons it found in your corrections. It reads your Claude Code and Codex logs and never changes them. Put `target/release` on your `PATH` for good so agents can run `lorectl` too.

Then, in Claude Code, install the plugin and let the agent write your lessons:

```text
/plugin marketplace add hsusul/lore
/plugin install lore@lore
/lore:lore-lessons
```

The agent reads every candidate Lore mined, groups the ones that mean the same thing, and files each rule with its receipts, the sessions it came from. Nothing becomes a note until you accept it:

```bash
lorectl init --review
```

Accept, edit, or skip each one. From the next session on, the plugin shows the accepted notes for the repository at session start, each with a freshness label. `lorectl connect` says whether Claude Code and Codex are connected, and prints what to add for one that is not. `lorectl help mem` prints the memory commands alone.

The desktop app is a second front end over the same archive. It needs Node.js 20, the Xcode command-line tools, and the Tauri 2 prerequisites for macOS. Run it with `npm run tauri dev`.

## The archive

`lorectl scan` reads the session logs Claude Code and Codex already keep on disk. It looks in `~/.claude/projects`, `~/.codex/sessions`, and `~/.codex/archived_sessions`. `CLAUDE_CONFIG_DIR` and `CODEX_HOME` move those roots. Lore opens them read only and never changes them. A second scan of unchanged files ingests nothing. Every scan ingests the newest files first.

A large history takes a while to scan the first time. `lorectl scan --since 30d` ingests only the files modified in the last 30 days (`h` for hours and `m` for minutes work too), so you can start with recent work and run a full `lorectl scan` later. A `--since` scan never marks older files missing.

The archive is a SQLite database named `lore.db` with `blobs` and `backups` beside it. It lives in the platform data directory under `dev.lore.app`. `LORE_ARCHIVE_DIR` or `--archive <DIR>` moves it. The path must be absolute.

```bash
lorectl scan
lorectl search "retry agent:codex"
lorectl inspect <SESSION_ID> --patch
lorectl status
```

The query is one argument, so quote it. Plain words are searched as text. Filters narrow the search.

| Filter | Matches |
| --- | --- |
| `agent:codex` | Sessions from one agent (`claude-code` or `codex`) |
| `path:src/auth/` | Sessions that touched a file under this path |
| `tool:Bash`, `model:sonnet` | Sessions that called this tool, or ran on a matching model |
| `repo:lore`, `branch:main`, `commit:1a2b3c` | A repository, a git branch, or a commit sha prefix |
| `after:2026-07-01`, `before:2026-08-01` | Sessions started on or after, or before, a date |
| `has:error`, `has:patch` | A tool call failed, or the session recorded a patch |

Lore links each session to the git repository it worked in. It records which branch and commit the session saw. For files an agent wrote, it checks whether that exact content reached a commit. `lorectl status` flags agent-written files that are staged and not committed. `lorectl hook session-start` prints the same warning for an agent at the start of a session, and prints nothing when there is nothing at risk.

`lorectl --help` lists every command, option, and exit code.

## Memory

Memory lives in a vault. The vault is a git repository of markdown notes that Obsidian can open. It is at `~/Lore` unless `LORE_VAULT_DIR` or `--vault <DIR>` says otherwise. Every write is one git commit.

```bash
lorectl mem init
```

### Add a note

A note is a markdown file with frontmatter. The `type` is one of `convention`, `decision`, `fact`, `howto`, or `pitfall`. Lore fills in `id`, `created`, `status`, and `scope` when you leave them out. A note with no scope is global.

```bash
cat > note.md <<'EOF'
---
type: convention
title: Quote every query argument
---
lorectl takes the query as one argument, so quote it.
EOF
lorectl mem add note.md
```

`-` reads the note from stdin. Global notes land in `global/`. A note for one repository has `scope: repo:<key>`. The key is a hash of the repository's `origin` URL. `lorectl mem recall --json` prints it for the repository you are in. Anchors tie a note to lines of code. Pass `--cwd <DIR>` so Lore pins each anchor's hash and commit from that repository.

```markdown
---
type: pitfall
title: Line 3 is load bearing
scope: repo:83ce96d0c231c106
anchors:
  - path: src/lib.rs
    lines: 3
---
Do not reorder line 3.
```

### Recall

```bash
lorectl mem recall                 # global notes and notes for this repository
lorectl mem recall "worktree"      # only notes that match the words
lorectl mem recall --cwd ~/code/app --limit 5 --json
lorectl mem search "index lock"    # BM25 ranking over every note
lorectl mem show <NOTE>            # by id, file name, or vault path
```

Each recalled note carries a freshness label.

| Label | Meaning |
| --- | --- |
| `fresh` | The anchored lines are where the note says. |
| `moved` | The lines are unchanged but now somewhere else. Lore prints the new place. |
| `changed` | The file is there but the anchored lines are not. |
| `gone` | The file is gone and no rename of it was found. |
| `unanchored` | The note has no anchors, so there is nothing to check. |

You can edit, rename, or delete notes by hand, in an editor or in Obsidian. Search and show pick up the change. `lorectl mem reindex` rebuilds the index from the vault files alone.

### The inbox and review

Agents do not write notes. They file proposals into the vault's `_inbox/`, which git ignores. Lore redacts secrets in a proposal's title, body, and anchor paths before it is stored. A person reviews the inbox.

```bash
lorectl mem inbox                          # pending proposals
lorectl mem accept <ID>                    # write it as a note, one commit
lorectl mem accept <ID> --as <slug>        # with a file name you choose
lorectl mem reject <ID> --reason "<TEXT>"  # decline; the vault is untouched
lorectl mem retire <NOTE> --reason "<TEXT>"
```

Under each proposal, `mem inbox` prints its receipt: the archived session it came from, that session's date, and a short redacted excerpt of the passage that shares the most words with the proposal. With no archive, or a session the archive does not hold, it says so in one line. `--json` carries the same as `receipt`.

`mem inbox` marks each proposal's source `(verified)` when the archive held that agent's session at the time the proposal was filed, and `(unverified)` otherwise. Proposals filed with `mem propose`, or before Lore recorded this, are unverified.

`lorectl mem mine` reads the archive for corrections you made to an agent, such as "no, use pnpm not npm", standing rules such as "make sure the docs are never pushed", and instructions you had to repeat. It groups the ones that say the same thing into lessons and keeps those that state a rule. Lessons rank first by how much they read like a standing rule: "from now on", "always", "never", and "make sure" raise one ("make sure" less when the same turn also says to push, merge, or the like), and a PR or issue number, a branch name, a URL, a masked secret, a one-off order like "push this branch", or a chore with a rule tacked on with "and" ("delete the runtimes and build them on my ssd from now on") lowers it. Then lessons you gave in more conversations rank higher, then newer ones. A resumed or forked copy of a conversation counts once. Each lesson prints with its receipts: the sessions it came from, the date, what you said, and what the agent had just done. Under a lesson, `may conflict with` lists up to three later turns in the same repository about the same command (PR, push, commit, branch, merge, rebase, tests, mocks, docs, permission). Lore does not judge them: read them, or let the agent judge them through `mine_candidates`, before you accept the lesson. A lesson that scores below zero reads as a one-off order and is left out; the first line counts them, and `--all` lists them too. It is a dry run: it writes nothing. `--limit N` (default 20) and `--json` are optional. Two limits tune what it finds: `--min-overlap PERCENT` (default 50) is how much of their words two corrections must share to be one lesson, and `--max-chars N` (default 300) is the longest turn still read as a correction. Pass the same limits with `--propose`, since they decide which lessons exist.

`lorectl mem mine --propose <ID>...` files the lessons you name into the inbox, each with its receipts in the body, for you to accept or reject like any proposal. It never files the same lesson twice.

`lorectl mem eval` scores a policy for accepting proposals without you against the proposals you already accepted and rejected. It reports how many it would have taken off your hands, lists every one it would have accepted that you rejected, and says whether it clears a fixed bar. It accepts nothing and writes nothing. `--policy` is `never` (the default) or `starting-bar`.

A script or an agent without MCP can file a proposal with `lorectl mem propose`. The body is read from stdin.

```bash
echo "Run cargo with CARGO_BUILD_JOBS=4 on this host." | \
  lorectl mem propose --agent claude-code --session <SESSION> --type pitfall --title "Bound cargo jobs"
```

`--scope`, `--anchor PATH[:LINES]`, and one of `--updates`, `--extends`, or `--derives <ID>` are optional.

## Skills

`lorectl skills` reports on the skill folders your agents load. It reads `~/.agents/skills`, `~/.claude/skills`, `~/.codex/skills`, `~/.cursor/skills`, and plugin caches. It never writes.

```bash
lorectl skills ls       # every skill, with where it is installed and its source
lorectl skills doctor   # problems, each with a command to fix it or a hint
```

`skills doctor` reports duplicates, drift between copies, unknown sources, broken references, name mismatches, bad frontmatter, and Codex's skill budget. It exits 0 whenever the scan ran. Add `--json` to gate a script on the findings. `HOME`, `CLAUDE_CONFIG_DIR`, and `CODEX_HOME` set where it looks.

## Using Lore from an agent

`lorectl mcp` is a Model Context Protocol server on stdin and stdout. An agent runs it. Give the agent the absolute path to the binary. Run `lorectl scan` first, or the session tools report that there is no archive yet.

Claude Code:

```bash
claude mcp add lore -- /absolute/path/to/lorectl mcp
```

Codex, in `~/.codex/config.toml`:

```toml
[mcp_servers.lore]
command = "/absolute/path/to/lorectl"
args = ["mcp"]
```

The vault is `~/Lore` by default. For another vault, set `LORE_VAULT_DIR` to an absolute path in the server's environment. For another archive, add `--archive /absolute/dir` to the arguments. Lore names the agent behind a proposal from `LORE_AGENT` in the server's environment, else from the name the client sends. If proposals fail with `agent_unknown`, set `LORE_AGENT=claude-code` or `LORE_AGENT=codex`.

### Plugins

`plugins/` holds a plugin for each agent. Each one runs `lorectl hook session-start` when a session starts. The agent passes its hook JSON on stdin, and the hook's output becomes context the session starts with: the repository's memory notes with their freshness labels, and a warning about agent work at risk. Each plugin also registers `lorectl mcp` as the `lore` MCP server, with `LORE_AGENT` set to that agent, so installing a plugin wires both the hook and the tools. `lorectl` must be on `PATH`. To use another vault or archive, set `LORE_VAULT_DIR` or `LORE_ARCHIVE_DIR` in the environment the agent starts in.

In Claude Code, run `/plugin marketplace add hsusul/lore`, then `/plugin install lore@lore`. For Codex, `plugins/codex` holds `.codex-plugin/plugin.json` and `hooks/hooks.json`.

Each plugin also ships the `lore-lessons` skill (`/lore:lore-lessons` in Claude Code). The agent reads every `mine_candidates` page, groups your past corrections by meaning, and files each rule once with its receipts as a proposal. Then run `lorectl init --review` to accept or reject them.

### Tools

The server offers six tools. All but `propose_memory` are read only.

- `search_sessions` searches the archive with the filters above. It returns up to 20 hits per call, each with a session id and a redacted snippet.
- `read_session` returns one session in order, paged at 24,000 characters. It leaves out thinking and raw tool payloads.
- `recall_memory` returns accepted notes for the directory the agent is working in, each with its freshness label.
- `read_note` returns one note in full, with its freshness when the agent passes its working directory.
- `mine_candidates` returns the miner's candidate lessons in the same order as `mem mine`, each with up to 5 receipts: session, agent, date, the repository with the `repo:<key>` scope a note for it takes, the correction, and what the agent had just done. Pass `cwd` to also get the current directory's repository as `cwd_repo`. Replies are paged: each carries `total` and a `next_cursor` to pass back until it is null, and the description asks the agent to read every page first. It also asks the agent to group candidates by meaning, write each group as one short rule, and file it with `propose_memory`, citing the sessions. Lore makes no model calls; the agent does the grouping, and you still accept or reject each proposal.
- `propose_memory` files one proposal into the vault's `_inbox/<agent>-<session>.jsonl`. It answers with the proposal id and the inbox file.

`propose_memory` is the only tool that writes. It never accepts, edits, or deletes notes, and retiring a note needs a person. You review its proposals with `lorectl mem inbox`, `mem accept`, and `mem reject`. Secrets in a proposal are masked before it is stored. `title`, `body`, `type`, and `scope` are required. `type` is one of `convention`, `decision`, `fact`, `howto`, or `pitfall`. `scope` is `global` or `repo`. A `repo` proposal needs `cwd` and may carry up to 8 `anchors`, each a `path` relative to the repo root with optional `lines`. `updates` names a note id to supersede. `session_id` is the agent's own session id. Lore records the source as unverified unless it can match it.

```json
{"title": "Bound cargo jobs", "type": "pitfall", "scope": "repo",
 "cwd": "/absolute/path/to/repo",
 "body": "Run cargo with CARGO_BUILD_JOBS=4 on this host.",
 "anchors": [{"path": "Cargo.toml"}]}
```

## Privacy

Everything stays on your machine. Lore has no accounts, no telemetry, and no server. The archive code, the memory and skills code, `lorectl`, and the desktop shell have no network code. CI scans their sources for networking APIs and runs their tests with the network denied (`scripts/egress-check.sh`). The app does not check for updates.

Before text is indexed, Lore scans it for secrets with fixed rules for known key formats plus an entropy check. Flagged spans are masked. Search, snippets, exports, and MCP results use the masked text. The archive still holds the original text, and so does the original log. Proposals are redacted before they reach the inbox. Redaction is a safeguard, not a promise that nothing secret remains.

When you connect `lorectl mcp` to an agent, the redacted text it returns goes to that agent's model provider, as any tool result does.

## Development

`npm run tauri dev` runs the desktop app. `npm run dev` serves the UI with a built-in sample archive and reads no files. `scripts/synth-archive.sh` writes a fake Claude Code and Codex profile to test against. Never point a development run at your real history. Set `CLAUDE_CONFIG_DIR`, `CODEX_HOME`, `LORE_ARCHIVE_DIR`, and `LORE_VAULT_DIR` first.

```text
crates/lore-core/    archive: discovery, parsing, ingest, secret scanning, search, git evidence
crates/lore-memory/  vault, notes, index, anchors, recall, inbox, review
crates/lore-skills/  skill inventory and doctor
crates/lore-ipc/     IPC types and generated TypeScript bindings
crates/lorectl/      the lorectl command line and MCP server
src-tauri/, src/     Tauri 2 desktop shell and React UI
```

CI runs `cargo fmt`, `cargo clippy -D warnings`, and `cargo test` per package, plus `scripts/egress-check.sh` and `scripts/toolchain-pin-check.sh`. See [`.github/workflows`](.github/workflows) for the full list.

## License

[Apache License 2.0](LICENSE)

Lore is not affiliated with Anthropic or OpenAI.
