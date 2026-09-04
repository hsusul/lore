//! The hand-rolled `--help` text.
//!
//! Hand-rolling it is the price of not depending on `clap` (see the manifest).
//! Paying it here, in one function with a test, keeps the price visible and
//! keeps the dependency audit trivial.
//!
//! The exit-code table interpolates the constants from [`crate::exit`] rather
//! than restating the numbers, so documentation and behaviour cannot drift.

use crate::cli::COMMANDS;
use crate::exit;

/// The full usage text.
#[must_use]
pub fn usage() -> String {
    let mut commands = String::new();
    for command in COMMANDS {
        commands.push_str(&format!(
            "    {:<9} {}\n",
            command.name(),
            command.summary()
        ));
    }

    format!(
        "\
lorectl {version} — a local ledger of coding-agent work, reconciled against Git.

Usage:
    lorectl [OPTIONS] <COMMAND>

Options may appear before or after the command.

Commands (recognized; none are implemented in this build):
{commands}
Options:
    --archive <DIR>   Archive directory to use. Must be absolute; it names the
                      directory, not the database file.
    --json            Machine-readable output where a command supports it.
    -h, --help        Print this help.
    -V, --version     Print the version.

Environment:
    LORE_ARCHIVE_DIR  Archive directory (absolute) used when --archive is
                      absent. Otherwise the platform data directory is used.

Exit codes:
    {ok}  ok
    {usage}  usage: unknown flag or command, a relative --archive, or a command
       this build does not implement
    {no_archive}  no archive at that location (run `lorectl scan` to build one)
    {not_a_repo}  not a Git repository (reserved; unused in this build)
    {unreadable}  archive present but unreadable: not a Lore archive, a schema
       from a newer or older build, an inconsistent migration ledger, or an
       I/O failure
    5-9  reserved
",
        version = lore_core::version(),
        commands = commands,
        ok = exit::OK,
        usage = exit::USAGE,
        no_archive = exit::NO_ARCHIVE,
        not_a_repo = exit::NOT_A_REPO,
        unreadable = exit::ARCHIVE_UNREADABLE,
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
        ] {
            assert!(
                text.contains(&format!("\n    {code}  ")),
                "exit code {code} is assigned but not in the help table"
            );
        }
    }

    #[test]
    fn help_is_honest_that_no_command_works_yet() {
        // The commands are listed so the destination is visible; the text must
        // not let a reader believe any of them does something today.
        assert!(usage().contains("none are implemented in this build"));
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
