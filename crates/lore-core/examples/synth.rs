//! Write a deterministic synthetic agent profile for demos and checks.
//!
//! Usage: `synth <out_dir> <claude_count> <codex_count> <seed>`
//!
//! Writes `<out_dir>/claude/projects` and `<out_dir>/codex/sessions`, so no
//! check ever needs to read a real `~/.claude` or `~/.codex`. The same
//! arguments always produce the same files.

use std::path::Path;
use std::process::ExitCode;

use lore_core::synthetic::{generate, ProfileSpec};

const USAGE: &str = "usage: synth <out_dir> <claude_count> <codex_count> <seed>";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [out_dir, claude, codex, seed] = args.as_slice() else {
        eprintln!("{USAGE}");
        return ExitCode::from(2);
    };
    let (Ok(claude_sessions), Ok(codex_sessions), Ok(seed)) =
        (claude.parse(), codex.parse(), seed.parse())
    else {
        eprintln!("{USAGE} (counts and seed must be non-negative integers)");
        return ExitCode::from(2);
    };

    let spec = ProfileSpec {
        claude_sessions,
        codex_sessions,
        max_extra_turns: 2,
        seed,
    };
    match generate(Path::new(out_dir), &spec) {
        Ok(profile) => {
            println!("{out_dir}");
            println!(
                "claude {} codex {}",
                profile.claude_files, profile.codex_files
            );
            ExitCode::SUCCESS
        }
        Err(err) => {
            eprintln!("synth: {err}");
            ExitCode::FAILURE
        }
    }
}
