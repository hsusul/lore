//! `lorectl` — the command-line surface over a Lore archive.
//!
//! A second binary, installable without the desktop app, over the same
//! `lore-core` and the same archive directory (`lore_core::paths`). It is
//! subject to the same guarantees as the archive it reads: no network
//! capability (`tests/no_network_in_lorectl.rs`, `scripts/egress-check.sh`) and
//! read-only on agent files.
//!
//! This build resolves the archive location, parses the full command surface,
//! and maps failures onto the exit-code taxonomy in [`exit`]. It does not yet
//! open the archive: opening it for writing needs an advisory scan lock, and
//! opening it for reading needs the query wrappers, both of which are their own
//! slices. Recognizing commands without implementing them is deliberate — the
//! parser and the exit codes are the contract that later slices fill in.

mod cli;
mod exit;
mod help;

use std::io::Write;
use std::process::ExitCode;

use cli::Action;
use exit::CliError;

fn main() -> ExitCode {
    match run() {
        Ok(code) => ExitCode::from(code),
        Err(error) => {
            report(&error);
            ExitCode::from(exit::exit_code(&error))
        }
    }
}

fn run() -> Result<u8, CliError> {
    let invocation = cli::parse_env()?;

    match invocation.action {
        Action::Help => {
            print!("{}", help::usage());
            Ok(exit::OK)
        }
        Action::Version => {
            println!("{}", help::version());
            Ok(exit::OK)
        }
        Action::NoCommand => {
            // Usage on stderr, not stdout: a bare `lorectl` in a script is a
            // mistake, and a mistake must not look like output.
            eprint!("{}", help::usage());
            Ok(exit::USAGE)
        }
        Action::Run(command) => {
            // Resolve the archive location before refusing the command, so a
            // bad `--archive` is reported as the bad `--archive` it is rather
            // than being masked by "not implemented".
            invocation.archive_dir()?;
            Err(CliError::NotImplemented(command.name()))
        }
    }
}

/// Print an error the way a CLI should: one line to stderr, plus the fix when
/// there is one. Never the archive's contents.
fn report(error: &CliError) {
    let mut stderr = std::io::stderr().lock();
    let _ = writeln!(stderr, "lorectl: {error}");
    if let Some(advice) = error.advice() {
        let _ = writeln!(stderr, "  {advice}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::Command;

    /// The dispatch decision, without the process: what `run` would do for an
    /// already-parsed invocation.
    fn dispatch(args: &[&str]) -> Result<u8, CliError> {
        let invocation = cli::parse(args.iter().copied())?;
        match invocation.action {
            Action::Help | Action::Version => Ok(exit::OK),
            Action::NoCommand => Ok(exit::USAGE),
            Action::Run(command) => {
                invocation.archive_dir()?;
                Err(CliError::NotImplemented(command.name()))
            }
        }
    }

    #[test]
    fn help_and_version_succeed() {
        assert_eq!(dispatch(&["--help"]).expect("help succeeds"), exit::OK);
        assert_eq!(
            dispatch(&["--version"]).expect("version succeeds"),
            exit::OK
        );
    }

    #[test]
    fn a_bare_invocation_is_a_usage_failure() {
        assert_eq!(dispatch(&[]).expect("prints usage"), exit::USAGE);
    }

    #[test]
    fn every_command_reports_not_implemented_rather_than_faking_output() {
        for command in cli::COMMANDS {
            let error = dispatch(&["--archive", "/tmp/lore-test", command.name()])
                .expect_err("no command works yet");
            assert!(matches!(error, CliError::NotImplemented(name) if name == command.name()));
            assert_eq!(exit::exit_code(&error), exit::USAGE);
        }
    }

    #[test]
    fn json_does_not_conjure_an_implementation() {
        let error = dispatch(&["--json", "scan"]).expect_err("still not implemented");
        assert!(matches!(error, CliError::NotImplemented("scan")));
        assert_eq!(exit::exit_code(&error), exit::USAGE);
    }

    #[test]
    fn a_bad_archive_flag_outranks_not_implemented() {
        // Otherwise the first thing a user fixes would be the wrong thing.
        let error = dispatch(&["--archive", "relative", "scan"]).expect_err("relative is refused");
        assert!(matches!(
            error,
            CliError::Path(lore_core::paths::PathError::RelativeOverride)
        ));
    }

    #[test]
    fn help_is_still_help_with_a_bad_archive_flag() {
        // Documentation must be reachable when the invocation is otherwise
        // wrong; that is when people need it most.
        assert_eq!(
            dispatch(&["--archive", "relative", "--help"]).expect("help succeeds"),
            exit::OK
        );
    }

    #[test]
    fn a_not_implemented_error_names_the_command_the_user_typed() {
        let error = CliError::NotImplemented(Command::Status.name());
        assert!(error.to_string().contains("lorectl status"), "{error}");
    }
}
