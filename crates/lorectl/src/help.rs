//! The hand-rolled `--help` text.
//!
//! Hand-rolling it is the price of not depending on `clap` (see the manifest).
//! Paying it here, in one function with a test, keeps the price visible and
//! keeps the dependency audit trivial.
//!
//! The exit-code table interpolates the constants from [`crate::exit`] rather
//! than restating the numbers, so documentation and behaviour cannot drift.

use crate::cli::{Command, COMMANDS};
use crate::exit;

/// The help for `topic`: `lorectl help mem` and `lorectl mem --help` print the
/// `mem` section alone, and likewise `skills`, `mcp`, and `hook`. Every other
/// command shares the full text.
#[must_use]
pub fn usage_for(topic: Option<Command>) -> String {
    match topic {
        Some(Command::Mem) => crate::mem::USAGE.to_string(),
        Some(Command::Skills) => SKILLS_USAGE.to_string(),
        Some(Command::Mcp) => MCP_USAGE.to_string(),
        Some(Command::Hook) => HOOK_USAGE.to_string(),
        Some(Command::Connect) => CONNECT_USAGE.to_string(),
        Some(Command::Report) => crate::report::USAGE.to_string(),
        _ => usage(),
    }
}

/// `lorectl help skills`. The kinds are the `kind` strings `--json` carries.
const SKILLS_USAGE: &str = "\
Skills (lorectl skills <SUBCOMMAND> [--json]):
    ls                Every skill folder each agent loads, with its source
    doctor            Problems, each with a quoted command or a plain hint;
                      nothing is suggested for a copy an agent manages
    --json            Machine-readable output, to gate a script on findings
    Finding kinds: duplicate, drift, unknown-source, broken-ref,
    name-mismatch, bad-frontmatter, codex-budget.
    Reads ~/.agents/skills, ~/.claude/skills, ~/.codex/skills,
    ~/.cursor/skills, and plugin caches. HOME, CLAUDE_CONFIG_DIR, and
    CODEX_HOME set where it looks. Never writes; exits 0 when the scan ran.
";

/// `lorectl help mcp`. The tool names are the ones `tools/list` answers with.
const MCP_USAGE: &str = "\
MCP server (lorectl mcp [--archive <DIR>]):
    A Model Context Protocol server on stdin and stdout, run by an agent, not
    a person. Run `lorectl scan` first, or the session tools report that
    there is no archive yet.
    Tools:
    search_sessions   Full-text search of archived sessions; redacted snippets
    read_session      One archived session's text in order, paged
    recall_memory     Accepted notes for a repository, each with its freshness
    read_note         One note in full
    mine_candidates   Corrections and rules the person gave agents, newest
                      first, with receipts, for the agent to group and propose
    propose_memory    File a proposal in the vault's inbox, the only tool that
                      writes; review it with `lorectl mem inbox`
    Environment:
    LORE_VAULT_DIR    Vault directory (absolute); otherwise ~/Lore
    LORE_AGENT        claude-code or codex; names the agent behind a proposal
                      ahead of the name the client sends
    LORE_ARCHIVE_DIR  Archive directory (absolute) when --archive is absent
";

/// `lorectl help hook`.
const HOOK_USAGE: &str = "\
Hooks (lorectl hook <EVENT> [--archive <DIR>]):
    session-start     One line when files agents wrote in this repository are
                      staged and not committed. Then up to 10 accepted
                      memory notes for the repository, each with its
                      freshness label, and how many more recall_memory
                      lists. Then `Lore session: <ID>` when the archive
                      holds the hook JSON's `session_id`; pass that ID to
                      propose_memory. Prints nothing when there is none.
    prompt            When a person sends `prompt`, up to 3 accepted notes
                      that share at least two of its content words (or its
                      only one), each with its freshness label, as
                      UserPromptSubmit `additionalContext` JSON. Skips notes
                      the session was already shown. Prints nothing for a
                      prompt like `yes` or when no note matches.
    pre-edit          Before a tool edits `tool_input.file_path`, or the
                      files a Codex `apply_patch` names, up to 5 accepted
                      notes anchored to them, each with its freshness
                      label, as PreToolUse `additionalContext` JSON. It
                      never allows or blocks the edit. Prints nothing when
                      no note is anchored there.
    pre-tool          Before a shell command or file write, checks the
                      `enforce` rules of the active notes in scope. The
                      first rule the call matches prints a PreToolUse
                      `deny` naming the note. Otherwise prints what
                      pre-edit would. An unreadable vault or a rule that
                      does not parse blocks nothing.
    Notes are for `cwd` in the hook JSON on stdin, else the current
    directory. LORE_VAULT_DIR names the vault; otherwise ~/Lore.
    Run by an agent, not a person. It never scans, and it writes nothing
    but the vault's rebuildable index and, under .lore/shown/, which notes
    each `session_id` was shown. Every failure, and any other EVENT,
    exits 0 in silence, so a session never stalls on Lore.
";

/// `lorectl help connect`.
const CONNECT_USAGE: &str = "\
Connect (lorectl connect):
    Whether Claude Code and Codex run `lorectl mcp` and the `lorectl hook`
    events session-start, prompt, and pre-tool, and the exact command or
    config to add each one missing. A pre-edit hook is reported as
    outdated, since it enforces no note.
    Reads .claude.json and settings.json, and Codex's config.toml and
    hooks.json. CLAUDE_CONFIG_DIR and CODEX_HOME set where it looks;
    otherwise ~/.claude and ~/.codex.
    Never writes any agent config and never runs claude or codex. Exits 0.
";

/// The full usage text.
#[must_use]
pub fn usage() -> String {
    // Pad the summaries to a common width so the "not implemented" notes line
    // up in a column instead of straggling after sentences of different lengths.
    let widest = COMMANDS
        .iter()
        .map(|c| c.summary().len())
        .max()
        .unwrap_or_default();
    // Commands that take an operand advertise it, so `--help` shows the shape
    // of the invocation rather than just the verb.
    let spelled = |c: &crate::cli::Command| match c.operand() {
        Some(operand) => format!("{} <{operand}>", c.name()),
        None => c.name().to_string(),
    };
    let name_width = COMMANDS.iter().map(|c| spelled(c).len()).max().unwrap_or(0);

    let mut commands = String::new();
    for command in COMMANDS {
        // Mark what does not work yet, rather than listing all six as if they
        // were equals. A reader must be able to tell at a glance.
        if command.is_implemented() {
            commands.push_str(&format!(
                "    {:<name_width$}  {}\n",
                spelled(command),
                command.summary()
            ));
        } else {
            commands.push_str(&format!(
                "    {:<name_width$}  {:<widest$}   (not implemented in this build)\n",
                spelled(command),
                command.summary(),
            ));
        }
    }

    format!(
        "\
lorectl {version}

Lore keeps a local, searchable archive of your Claude Code and Codex sessions.
It gives coding agents long-term memory and checks the skills they load.

Usage:
    lorectl [OPTIONS] <COMMAND>

Options may appear before or after the command.

Commands:
{commands}
{memory}
Options:
    --archive <DIR>   Archive directory to use. Must be absolute; it names the
                      directory, not the database file.
    --json            Machine-readable output where a command supports it.
    --patch           With `inspect`: also print each recorded patch.
    --since <DUR>     With `scan`: only sources modified in the last DUR,
                      such as 30d, 12h, or 90m, newest first. With `init`:
                      the window to scan, default 30d. With `report`: the
                      window, or a start date; default 7d.
    --limit <N>       With `init`: lessons to show, default 5.
    --scan-seconds <N> With `init`: stop scanning after N seconds, newest
                      files first, and show lessons; default 60. The rest
                      waits for the next scan.
    --review          With `init`: offer each lesson to keep, skip, or edit.
                      On by default when stdin is a terminal; `--no-review`
                      turns it off.
    --vault <DIR>     With `init`, `mem`, and `report`: the vault directory.
    -h, --help        Print this help.
    -V, --version     Print the version.

Skills:
    skills ls         Every skill folder each agent loads, with its source.
    skills doctor     Duplicates, drift, unknown sources, broken references,
                      name mismatches, bad frontmatter, and Codex's budget.
                      Each gets a quoted command or a plain hint; nothing
                      is suggested for a copy an agent manages. Never
                      writes; exits 0.

Environment:
    LORE_VAULT_DIR    Vault directory (absolute) for `mem` when --vault is
                      absent. Otherwise ~/Lore.
    LORE_ARCHIVE_DIR  Archive directory (absolute) used when --archive is
                      absent. Otherwise the platform data directory is used.
    HOME              Home root `skills` scans (~/.agents, ~/.cursor).
    CLAUDE_CONFIG_DIR Claude Code's folder for `skills`; else ~/.claude.
    CODEX_HOME        Codex's folder for `skills`; else ~/.codex.

Exit codes:
    {ok}  ok
    {usage}  usage: unknown flag or command, a relative --archive, or a command
       this build does not implement
    {no_archive}  no archive at that location (run `lorectl init`, or `lorectl scan`); for
       `mem`, no vault there (run `lorectl mem init`)
    {not_a_repo}  not inside a Git repository
    {unreadable}  archive present but unreadable: not a Lore archive, a schema
       from a newer or older build, an inconsistent migration ledger, or an
       I/O failure; for `mem`, a vault file, the index, or git failed and the
       message says which
    {busy}  another Lore process is writing to this archive or vault; the one
       failure worth retrying unchanged
    {scan_failed}  the scan did not complete (the archive itself is fine)
    {refused}  `mem` refused a note, path, date, or id in use and wrote nothing, or
       `mem reindex` found one id in two files
    8-9  reserved
",
        version = lore_core::version(),
        commands = commands,
        memory = crate::mem::USAGE,
        ok = exit::OK,
        usage = exit::USAGE,
        no_archive = exit::NO_ARCHIVE,
        not_a_repo = exit::NOT_A_REPO,
        unreadable = exit::ARCHIVE_UNREADABLE,
        busy = exit::ARCHIVE_BUSY,
        scan_failed = exit::SCAN_FAILED,
        refused = exit::NOTE_REFUSED,
    )
}

/// The `--version` line.
#[must_use]
pub fn version() -> String {
    format!("lorectl {}", lore_core::version())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn help_lists_every_command_the_parser_knows() {
        let text = usage();
        for command in COMMANDS {
            assert!(
                text.contains(command.name()),
                "`{}` is parseable but undocumented",
                command.name()
            );
        }
    }

    #[test]
    fn help_documents_every_assigned_exit_code() {
        let text = usage();
        for code in [
            exit::OK,
            exit::USAGE,
            exit::NO_ARCHIVE,
            exit::NOT_A_REPO,
            exit::ARCHIVE_UNREADABLE,
            exit::ARCHIVE_BUSY,
            exit::SCAN_FAILED,
            exit::NOTE_REFUSED,
        ] {
            assert!(
                text.contains(&format!("\n    {code}  ")),
                "exit code {code} is assigned but not in the help table"
            );
        }
    }

    #[test]
    fn help_marks_exactly_the_unimplemented_commands() {
        // The commands are listed so the destination is visible; the text must
        // not let a reader believe an unfinished one does something today.
        let text = usage();
        for line in text.lines() {
            for command in COMMANDS {
                if line.trim_start().starts_with(command.name()) {
                    assert_eq!(
                        line.contains("not implemented"),
                        !command.is_implemented(),
                        "`{}` is mislabelled in help: {line}",
                        command.name()
                    );
                }
            }
        }
    }

    #[test]
    fn help_names_the_environment_override_and_the_absolute_path_rule() {
        let text = usage();
        assert!(text.contains(lore_core::paths::ARCHIVE_DIR_ENV));
        assert!(text.contains("absolute"));
    }

    #[test]
    fn version_reports_the_core_version() {
        assert_eq!(version(), format!("lorectl {}", lore_core::version()));
    }
}
