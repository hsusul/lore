//! `lorectl` — the command-line surface over a Lore archive.
//!
//! A second binary, installable without the desktop app, over the same
//! `lore-core` and the same archive directory (`lore_core::paths`). It is
//! subject to the same guarantees as the archive it reads: no network
//! capability (`tests/no_network_in_lorectl.rs`, `scripts/egress-check.sh`) and
//! read-only on agent files.
//!
//! This build implements `scan` — the CLI's only writer, and the first surface
//! other than the desktop app that writes to the archive at all. It runs
//! synchronously under an advisory writer lock ([`lore_core::lock`]) and stamps
//! when it finished. A command the parser knows but this build does not
//! implement is refused with a usage error, and `--help` marks it, so the
//! listing cannot flatter the binary. Every command is implemented today.

mod cli;
mod connect;
mod enforce_check;
mod eval;
mod exit;
mod help;
mod hook;
mod init;
mod mcp;
mod mem;
mod mine;
mod query;
mod receipt;
mod report;
mod review;
mod scan;
mod skills;
mod status;

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
        Action::Help(topic) => {
            print!("{}", help::usage_for(topic));
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
        Action::Run(command) if !command.is_implemented() => {
            // Resolve the archive location before refusing the command, so a
            // bad `--archive` is reported as the bad `--archive` it is rather
            // than being masked by "not implemented".
            invocation.archive_dir()?;
            Err(CliError::NotImplemented(command.name()))
        }
        Action::Run(cli::Command::Init) => init::run(&invocation),
        Action::Run(cli::Command::Scan) => scan::run(&invocation),
        Action::Run(cli::Command::Status) => status::run(&invocation),
        Action::Run(cli::Command::Hook) => hook::run(&invocation),
        Action::Run(cli::Command::Mcp) => mcp::run(&invocation),
        Action::Run(cli::Command::Mem) => mem::run(&invocation),
        Action::Run(cli::Command::Search) => query::search(&invocation),
        Action::Run(cli::Command::Inspect) => query::inspect(&invocation),
        Action::Run(cli::Command::Skills) => skills::run(&invocation),
        Action::Run(cli::Command::Connect) => Ok(connect::run()),
        Action::Run(cli::Command::Report) => report::run(&invocation),
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

    /// The dispatch decision for commands this build does **not** implement.
    ///
    /// Deliberately narrow. An earlier version handled every `Action::Run` as
    /// unimplemented, which made it a fiction the moment a command was
    /// implemented: a test could assert `status` was unimplemented and pass,
    /// while the real `run` implemented it. Implemented commands are exercised
    /// as real processes in `tests/`, because their contracts are about
    /// archives, locks, and exit codes.
    fn dispatch_unimplemented(args: &[&str]) -> Result<u8, CliError> {
        let invocation = cli::parse(args.iter().copied())?;
        match invocation.action {
            Action::Help(_) | Action::Version => Ok(exit::OK),
            Action::NoCommand => Ok(exit::USAGE),
            Action::Run(command) => {
                assert!(
                    !command.is_implemented(),
                    "`{}` is implemented; test it as a process, not through this helper",
                    command.name()
                );
                invocation.archive_dir()?;
                Err(CliError::NotImplemented(command.name()))
            }
        }
    }

    #[test]
    fn help_and_version_succeed() {
        assert_eq!(
            dispatch_unimplemented(&["--help"]).expect("help succeeds"),
            exit::OK
        );
        assert_eq!(
            dispatch_unimplemented(&["--version"]).expect("version succeeds"),
            exit::OK
        );
    }

    #[test]
    fn a_bare_invocation_is_a_usage_failure() {
        assert_eq!(
            dispatch_unimplemented(&[]).expect("prints usage"),
            exit::USAGE
        );
    }

    #[test]
    fn unimplemented_commands_report_it_rather_than_faking_output() {
        for command in cli::COMMANDS.iter().filter(|c| !c.is_implemented()) {
            let error = dispatch_unimplemented(&["--archive", "/tmp/lore-test", command.name()])
                .expect_err("this command does not work yet");
            assert!(matches!(error, CliError::NotImplemented(name) if name == command.name()));
            assert_eq!(exit::exit_code(&error), exit::USAGE);
        }
    }

    #[test]
    fn help_is_still_help_with_a_bad_archive_flag() {
        // Documentation must be reachable when the invocation is otherwise
        // wrong; that is when people need it most.
        assert_eq!(
            dispatch_unimplemented(&["--archive", "relative", "--help"]).expect("help succeeds"),
            exit::OK
        );
    }

    #[test]
    fn a_not_implemented_error_names_the_command_the_user_typed() {
        let error = CliError::NotImplemented(Command::Status.name());
        assert!(error.to_string().contains("lorectl status"), "{error}");
    }
}
