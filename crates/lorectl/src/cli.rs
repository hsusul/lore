//! Argument parsing: argv in, a fully-decided [`Invocation`] out.
//!
//! Parsing is separated from doing so the command surface can be tested without
//! spawning a process. The parser already knows every command name Lore will
//! grow into, even though this build implements none of them, for two reasons:
//! `--help` can then be honest about the destination, and a later slice can
//! replace one command's behaviour without touching argument parsing — which is
//! where flag-compatibility regressions come from.

use std::ffi::OsString;
use std::path::PathBuf;

use lexopt::prelude::*;
use lore_core::paths;

use crate::exit::CliError;

/// A command name `lorectl` recognizes.
///
/// Recognizing a name is not implementing it; see [`Command::run`]'s absence.
/// Every variant here is a promise about the *surface*, not the behaviour.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Command {
    /// Ingest new agent sessions into the archive.
    Scan,
    /// Summarize archived agent work against the current repository.
    Status,
    /// Show one archived session in detail.
    Inspect,
    /// Full-text search across archived sessions.
    Search,
    /// Agent hook entry points.
    Hook,
    /// Longer-form summary of archived work.
    Report,
}

/// Every command, in the order `--help` lists them.
pub const COMMANDS: &[Command] = &[
    Command::Scan,
    Command::Status,
    Command::Inspect,
    Command::Search,
    Command::Hook,
    Command::Report,
];

impl Command {
    /// The name as typed on the command line.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Command::Scan => "scan",
            Command::Status => "status",
            Command::Inspect => "inspect",
            Command::Search => "search",
            Command::Hook => "hook",
            Command::Report => "report",
        }
    }

    /// One-line description for `--help`.
    #[must_use]
    pub fn summary(self) -> &'static str {
        match self {
            Command::Scan => "Ingest new agent sessions into the archive",
            Command::Status => "Summarize archived work against the current repository",
            Command::Inspect => "Show one archived session in detail",
            Command::Search => "Full-text search across archived sessions",
            Command::Hook => "Agent hook entry points (session-start)",
            Command::Report => "Longer-form summary of archived work",
        }
    }

    /// Whether this build actually does the thing. `--help` reads this, so the
    /// listing cannot drift from what dispatch does.
    #[must_use]
    pub fn is_implemented(self) -> bool {
        match self {
            Command::Scan | Command::Search | Command::Inspect | Command::Status => true,
            Command::Hook | Command::Report => false,
        }
    }

    /// The single operand this command requires, if any — the thing it acts on.
    ///
    /// Declared here rather than checked inside each command so a missing or
    /// stray operand is one usage error from the parser, reported before any
    /// archive is opened.
    #[must_use]
    pub fn operand(self) -> Option<&'static str> {
        match self {
            Command::Search => Some("QUERY"),
            Command::Inspect => Some("SESSION_ID"),
            Command::Scan | Command::Status | Command::Hook | Command::Report => None,
        }
    }

    fn from_name(name: &str) -> Option<Self> {
        COMMANDS.iter().copied().find(|c| c.name() == name)
    }
}

/// What the process should do, once argv has been understood.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// `--help` was asked for: usage on stdout, exit 0.
    Help,
    /// `--version` was asked for.
    Version,
    /// A recognized command name was given.
    Run(Command),
    /// Nothing to do. Usage goes to *stderr* and the exit code is non-zero,
    /// because a bare `lorectl` in a script is a mistake, not a request for
    /// documentation.
    NoCommand,
}

/// A parsed command line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Invocation {
    /// `--archive`, exactly as given. Stored raw: turning a directory into a
    /// database path is `lore_core::paths`' job, and duplicating that join here
    /// is precisely how two surfaces end up opening different files.
    pub archive: Option<PathBuf>,
    /// `--json`. Machine-readable output where the command supports it.
    pub json: bool,
    /// `--patch`. Only meaningful for `inspect`.
    pub patch: bool,
    /// The command's operand, when it takes one (a search query, a session id).
    pub operand: Option<String>,
    /// What to do.
    pub action: Action,
}

impl Invocation {
    /// The archive directory this invocation targets.
    ///
    /// Resolution only — it neither creates nor opens anything. A relative
    /// `--archive` fails here rather than being resolved against the working
    /// directory, where the answer would depend on where the binary was run.
    pub fn archive_dir(&self) -> Result<PathBuf, CliError> {
        Ok(paths::archive_dir(self.archive.as_deref())?)
    }
}

/// Parse an argument list that does **not** include the binary name.
///
/// Global flags are accepted on either side of the command (`lorectl --json
/// scan` and `lorectl scan --json` both work), because both orders are what
/// people actually type and a parser that rejects one is a papercut forever.
pub fn parse<I>(args: I) -> Result<Invocation, CliError>
where
    I: IntoIterator,
    I::Item: Into<OsString>,
{
    let mut parser = lexopt::Parser::from_args(args);
    let mut archive = None;
    let mut json = false;
    let mut patch = false;
    let mut help = false;
    let mut version = false;
    let mut command: Option<Command> = None;
    let mut operand: Option<String> = None;

    while let Some(arg) = parser.next().map_err(usage)? {
        match arg {
            Short('h') | Long("help") => help = true,
            Short('V') | Long("version") => version = true,
            Long("json") => json = true,
            Long("patch") => patch = true,
            Long("archive") => archive = Some(PathBuf::from(parser.value().map_err(usage)?)),
            Value(value) if command.is_none() => {
                let name = value.to_string_lossy();
                command = Some(
                    Command::from_name(&name)
                        .ok_or_else(|| CliError::Usage(format!("unknown command `{name}`")))?,
                );
            }
            // One operand, after the command. A second is refused rather than
            // ignored: silently dropping an argument someone typed is how a
            // search for two words quietly searches for one.
            Value(value) if operand.is_none() && command.is_some() => {
                operand = Some(value.to_string_lossy().into_owned());
            }
            other => return Err(usage(other.unexpected())),
        }
    }

    // `--help` outranks `--version`, and both outrank a command: someone who
    // asks what a command does must not have it run at them.
    let action = if help {
        Action::Help
    } else if version {
        Action::Version
    } else {
        match command {
            Some(command) => {
                // Checked here, before anything opens an archive, so "you forgot
                // the query" is not reported as an archive problem.
                match (command.operand(), operand.is_some()) {
                    (Some(name), false) => {
                        return Err(CliError::Usage(format!(
                            "`lorectl {}` needs a {name}",
                            command.name()
                        )))
                    }
                    (None, true) => {
                        return Err(CliError::Usage(format!(
                            "`lorectl {}` takes no argument",
                            command.name()
                        )))
                    }
                    _ => {}
                }
                Action::Run(command)
            }
            None => Action::NoCommand,
        }
    };

    Ok(Invocation {
        archive,
        json,
        patch,
        operand,
        action,
    })
}

fn usage(error: lexopt::Error) -> CliError {
    CliError::Usage(error.to_string())
}

/// Convenience for the binary: parse the real process arguments.
pub fn parse_env() -> Result<Invocation, CliError> {
    parse(std::env::args_os().skip(1))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::exit::{self, USAGE};
    use lore_core::paths::{PathError, ARCHIVE_DB_FILENAME};
    use std::path::Path;

    fn parsed(args: &[&str]) -> Invocation {
        parse(args.iter().copied()).expect("parses")
    }

    #[test]
    fn a_bare_invocation_has_nothing_to_do() {
        assert_eq!(parsed(&[]).action, Action::NoCommand);
    }

    #[test]
    fn help_and_version_are_recognized_in_both_spellings() {
        for args in [vec!["--help"], vec!["-h"]] {
            assert_eq!(parse(args).expect("parses").action, Action::Help);
        }
        for args in [vec!["--version"], vec!["-V"]] {
            assert_eq!(parse(args).expect("parses").action, Action::Version);
        }
    }

    #[test]
    fn help_wins_over_version_and_over_a_command() {
        // Asking what something does must never run it.
        assert_eq!(parsed(&["--help", "--version"]).action, Action::Help);
        assert_eq!(parsed(&["scan", "--help"]).action, Action::Help);
        assert_eq!(parsed(&["--version", "scan"]).action, Action::Version);
    }

    #[test]
    fn the_implemented_commands_are_exactly_what_dispatch_handles() {
        // Keeps the help listing and the dispatch table honest about each other.
        let implemented: Vec<&str> = COMMANDS
            .iter()
            .filter(|c| c.is_implemented())
            .map(|c| c.name())
            .collect();
        assert_eq!(implemented, ["scan", "status", "inspect", "search"]);
    }

    #[test]
    fn every_reserved_command_name_parses() {
        for command in COMMANDS {
            let args: Vec<&str> = match command.operand() {
                Some(_) => vec![command.name(), "operand"],
                None => vec![command.name()],
            };
            assert_eq!(
                parsed(&args).action,
                Action::Run(*command),
                "the parser must already know `{}`",
                command.name()
            );
        }
    }

    #[test]
    fn a_command_that_needs_an_operand_refuses_to_run_without_one() {
        // Caught in the parser, so "you forgot the query" is never reported as
        // an archive problem later.
        for command in COMMANDS.iter().filter(|c| c.operand().is_some()) {
            let error = parse([command.name()]).unwrap_err();
            assert_eq!(exit::exit_code(&error), USAGE);
            assert!(error.to_string().contains(command.name()), "{error}");
        }
    }

    #[test]
    fn a_command_that_takes_no_operand_refuses_a_stray_one() {
        // Ignoring it would let `lorectl scan something` look like it worked.
        for command in COMMANDS.iter().filter(|c| c.operand().is_none()) {
            let error = parse([command.name(), "stray"]).unwrap_err();
            assert_eq!(exit::exit_code(&error), USAGE);
        }
    }

    #[test]
    fn an_operand_is_captured_verbatim() {
        let invocation = parsed(&["search", "needle in haystack"]);
        assert_eq!(invocation.operand.as_deref(), Some("needle in haystack"));
    }

    #[test]
    fn a_second_operand_is_refused_rather_than_dropped() {
        // Silently ignoring it is how a two-word search quietly searches for one.
        let error = parse(["search", "one", "two"]).unwrap_err();
        assert_eq!(exit::exit_code(&error), USAGE);
    }

    #[test]
    fn patch_is_parsed_and_defaults_off() {
        assert!(!parsed(&["inspect", "sid"]).patch);
        assert!(parsed(&["inspect", "--patch", "sid"]).patch);
        assert!(parsed(&["--patch", "inspect", "sid"]).patch);
    }

    #[test]
    fn global_flags_work_on_either_side_of_the_command() {
        let before = parsed(&["--json", "--archive", "/tmp/a", "scan"]);
        let after = parsed(&["scan", "--json", "--archive", "/tmp/a"]);
        assert_eq!(before, after);
        assert!(before.json);
        assert_eq!(before.action, Action::Run(Command::Scan));
    }

    #[test]
    fn an_absolute_archive_is_stored_exactly_as_given() {
        // Not joined with the database filename here: `paths` owns that, so the
        // CLI and the desktop shell cannot drift onto different files.
        let invocation = parsed(&["--archive", "/tmp/lore-archive", "status"]);
        assert_eq!(
            invocation.archive.as_deref(),
            Some(Path::new("/tmp/lore-archive"))
        );
        assert_eq!(
            invocation.archive_dir().expect("absolute resolves"),
            PathBuf::from("/tmp/lore-archive")
        );
        // The database filename is derived by `paths`, from the directory the
        // CLI passed through untouched — never joined on by the CLI itself.
        assert_eq!(
            paths::archive_db_path(invocation.archive.as_deref()).expect("absolute resolves"),
            PathBuf::from("/tmp/lore-archive").join(ARCHIVE_DB_FILENAME)
        );
    }

    #[test]
    fn a_relative_archive_is_a_usage_error_not_a_working_directory_guess() {
        let invocation = parsed(&["--archive", "relative/archive", "status"]);
        // It parses — the flag was well-formed — and fails on resolution.
        let error = invocation.archive_dir().unwrap_err();
        assert!(matches!(error, CliError::Path(PathError::RelativeOverride)));
        assert_eq!(exit::exit_code(&error), USAGE);
    }

    #[test]
    fn an_archive_flag_without_a_value_is_a_usage_error() {
        let error = parse(["--archive"]).unwrap_err();
        assert_eq!(exit::exit_code(&error), USAGE);
    }

    #[test]
    fn an_unknown_flag_is_a_usage_error() {
        let error = parse(["--wat"]).unwrap_err();
        assert_eq!(exit::exit_code(&error), USAGE);
        let error = parse(["-x"]).unwrap_err();
        assert_eq!(exit::exit_code(&error), USAGE);
    }

    #[test]
    fn an_unknown_command_is_a_usage_error_that_names_it() {
        let error = parse(["frobnicate"]).unwrap_err();
        assert_eq!(exit::exit_code(&error), USAGE);
        assert!(error.to_string().contains("frobnicate"), "{error}");
    }

    #[test]
    fn a_third_positional_argument_is_a_usage_error() {
        // `inspect` takes exactly one operand; anything further is a typo, and
        // ignoring it would be worse than refusing it.
        let error = parse(["inspect", "some-session-id", "extra"]).unwrap_err();
        assert_eq!(exit::exit_code(&error), USAGE);
    }

    #[test]
    fn json_does_not_change_which_command_was_asked_for() {
        // The flag exists so the commands that will honour it need no parser
        // change; today it must not alter dispatch.
        let with = parsed(&["--json", "scan"]);
        assert!(with.json);
        assert_eq!(with.action, Action::Run(Command::Scan));
    }

    #[test]
    fn command_names_are_unique_and_round_trip() {
        let mut names: Vec<&str> = COMMANDS.iter().map(|c| c.name()).collect();
        let count = names.len();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), count, "two commands share a name");
        for command in COMMANDS {
            assert_eq!(Command::from_name(command.name()), Some(*command));
        }
    }
}
