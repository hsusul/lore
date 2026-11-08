//! `lorectl --help` end to end: the banner says what Lore is today.
#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::process::Command;

#[test]
fn help_banner_describes_the_archive_memory_and_skills() {
    let output = Command::new(env!("CARGO_BIN_EXE_lorectl"))
        .arg("--help")
        .env_remove("LORE_ARCHIVE_DIR")
        .output()
        .expect("spawn lorectl");
    assert_eq!(output.status.code(), Some(0));
    let stdout = String::from_utf8(output.stdout).unwrap();
    let banner: Vec<&str> = stdout.lines().take(5).collect();
    assert_eq!(
        banner,
        [
            format!("lorectl {}", lore_core::version()).as_str(),
            "",
            "Lore keeps a local, searchable archive of your Claude Code and Codex sessions.",
            "It gives coding agents long-term memory and checks the skills they load.",
            "",
        ]
    );
    assert!(!stdout.contains("ledger of coding-agent work"));
}

fn help_text(args: &[&str]) -> String {
    let output = Command::new(env!("CARGO_BIN_EXE_lorectl"))
        .args(args)
        .env_remove("LORE_ARCHIVE_DIR")
        .output()
        .expect("spawn lorectl");
    assert_eq!(output.status.code(), Some(0), "{args:?}");
    String::from_utf8(output.stdout).unwrap()
}

/// The usage line of the full `--help`, which no topic page may repeat.
const FULL_HELP_USAGE: &str = "    lorectl [OPTIONS] <COMMAND>";

fn assert_topic_page(topic: &str, first_line: &str, names: &[&str]) {
    let page = help_text(&["help", topic]);
    assert_eq!(page, help_text(&[topic, "--help"]), "`{topic} --help`");
    assert_eq!(page, help_text(&[topic, "-h"]), "`{topic} -h`");
    assert_eq!(page.lines().next(), Some(first_line), "`help {topic}`");
    assert!(
        !page.contains(FULL_HELP_USAGE),
        "`help {topic}` printed the full help:\n{page}"
    );
    for name in names {
        assert!(
            page.contains(name),
            "`help {topic}` omits `{name}`:\n{page}"
        );
    }
}

#[test]
fn help_skills_prints_the_skills_page() {
    assert_topic_page(
        "skills",
        "Skills (lorectl skills <SUBCOMMAND> [--json]):",
        &[
            "    ls ",
            "    doctor ",
            "--json",
            "duplicate",
            "drift",
            "unknown-source",
            "broken-ref",
            "name-mismatch",
            "bad-frontmatter",
            "codex-budget",
            "HOME",
            "CLAUDE_CONFIG_DIR",
            "CODEX_HOME",
        ],
    );
}

#[test]
fn help_mcp_prints_the_mcp_page() {
    assert_topic_page(
        "mcp",
        "MCP server (lorectl mcp [--archive <DIR>]):",
        &[
            "    search_sessions ",
            "    read_session ",
            "    recall_memory ",
            "    read_note ",
            "    mine_candidates ",
            "    propose_memory ",
            "LORE_VAULT_DIR",
            "LORE_AGENT",
            "LORE_ARCHIVE_DIR",
        ],
    );
}

#[test]
fn help_hook_prints_the_hook_page() {
    assert_topic_page(
        "hook",
        "Hooks (lorectl hook <EVENT> [--archive <DIR>]):",
        &[
            "    session-start ",
            "    prompt ",
            "    pre-edit ",
            "    pre-tool ",
        ],
    );
}

#[test]
fn help_hook_describes_the_memory_block_the_session_and_the_hook_json() {
    assert_eq!(
        help_text(&["help", "hook"]),
        "\
Hooks (lorectl hook <EVENT> [--archive <DIR>]):
    session-start     One line when files agents wrote in this repository are
                      staged and not committed. Then up to 10 accepted
                      memory notes, each with its freshness label, and
                      how many more recall_memory lists. In a repository,
                      its notes and global notes share the 10, with at
                      least 4 for global notes when there are that many.
                      Of two notes with near-identical titles, only one
                      is shown. Then `Lore session: <ID>` when the archive
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
"
    );
}

#[test]
fn help_mem_is_still_the_memory_section_of_the_full_help() {
    let page = help_text(&["help", "mem"]);
    assert_eq!(page, help_text(&["mem", "--help"]));
    assert_eq!(
        page.lines().next(),
        Some("Memory (lorectl mem <SUBCOMMAND> [--vault <DIR>]):")
    );
    assert!(!page.contains(FULL_HELP_USAGE));
    // The same text the full help embeds, byte for byte.
    assert!(help_text(&["--help"]).contains(&page));
}

#[test]
fn the_version_is_set_and_matches_the_desktop_app_and_ui() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let json = |rel: &str| -> serde_json::Value {
        serde_json::from_str(&std::fs::read_to_string(root.join(rel)).unwrap()).unwrap()
    };
    let version = env!("CARGO_PKG_VERSION");
    assert!(!version.is_empty() && version != "0.0.0", "{version}");
    assert_eq!(help_text(&["--version"]), format!("lorectl {version}\n"));
    assert_eq!(json("src-tauri/tauri.conf.json")["version"], version);
    assert_eq!(json("package.json")["version"], version);
    assert_eq!(json("package-lock.json")["version"], version);
}

/// Every `lorectl` command in the README's "Get started" section, and each of
/// its flags, is one `lorectl --help` lists, and the first run's steps come
/// in an order that works: the archive and vault exist before the agent mines.
#[test]
fn the_readme_first_run_uses_only_commands_the_cli_lists() {
    let readme =
        std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../../README.md")).unwrap();
    let start = readme
        .find("\n## Get started\n")
        .expect("README has a Get started section");
    let section = &readme[start + 1..];
    let section = &section[..section[3..]
        .find("\n## ")
        .map_or(section.len(), |end| end + 3)];
    let help = help_text(&["--help"]);
    let commands: Vec<&str> = section
        .lines()
        .map(str::trim)
        .filter(|l| l.starts_with("lorectl "))
        .collect();
    assert!(!commands.is_empty(), "{section}");
    for command in &commands {
        let words: Vec<&str> = command
            .split_whitespace()
            .take_while(|w| *w != "#")
            .collect();
        let sub = words[1];
        assert!(
            help.lines()
                .any(|l| l.trim_start().starts_with(&format!("{sub} "))),
            "`{command}`: no `{sub}` in --help"
        );
        for flag in words.iter().filter(|w| w.starts_with("--")) {
            assert!(help.contains(flag), "`{command}`: no `{flag}` in --help");
        }
    }
    let at = |needle: &str| {
        section
            .find(needle)
            .unwrap_or_else(|| panic!("no `{needle}`:\n{section}"))
    };
    // Install comes first: a release download, with building from source as the
    // alternative, both before the first `lorectl init`.
    assert!(at("releases/download") < at("lorectl init\n"));
    assert!(at("cargo build --release -p lorectl") < at("lorectl init --review"));
    assert!(at("lorectl init\n") < at("/plugin install lore@lore"));
    assert!(at("/plugin install lore@lore") < at("/lore:lore-lessons"));
    assert!(at("/lore:lore-lessons") < at("lorectl init --review"));
}
