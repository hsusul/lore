//! `lorectl skills` as a process, against the lore-skills fixture home. HOME is
//! always pointed at a fixture or a temp dir, never the real one, and the
//! agent root overrides are cleared so a developer's own never leak in.
#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::SystemTime;

fn fixture_home() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../lore-skills/fixtures/home")
}

fn command(home: &Path, args: &[&str]) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_lorectl"));
    command
        .args(args)
        .env("HOME", home)
        .env_remove("CLAUDE_CONFIG_DIR")
        .env_remove("CODEX_HOME");
    command
}

fn lorectl(home: &Path, args: &[&str]) -> Output {
    command(home, args).output().expect("lorectl runs")
}

fn stdout(output: &Output) -> String {
    String::from_utf8(output.stdout.clone()).expect("utf-8")
}

fn json(output: &Output) -> Vec<serde_json::Value> {
    let value: serde_json::Value = serde_json::from_str(&stdout(output)).unwrap();
    value.as_array().unwrap().clone()
}

/// `(kind, skill, fix_kind, fix)` for each finding, in report order.
fn fixes(findings: &[serde_json::Value]) -> Vec<(String, String, String, String)> {
    let text = |v: &serde_json::Value, key: &str| v[key].as_str().unwrap_or("").to_string();
    findings
        .iter()
        .map(|f| {
            (
                text(f, "kind"),
                text(f, "skill"),
                text(f, "fix_kind"),
                text(f, "fix"),
            )
        })
        .collect()
}

fn write(path: &Path, text: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, text).unwrap();
}

#[test]
fn doctor_reports_and_still_exits_zero() {
    let output = lorectl(&fixture_home(), &["skills", "doctor"]);
    assert_eq!(output.status.code(), Some(0));
    let text = stdout(&output);
    let summary = text.lines().last().unwrap();
    assert_eq!(
        summary,
        "3 duplicates, 1 drift, 7 unknown sources, 6 broken refs, 1 name mismatches, \
         2 bad frontmatter, 1 codex budget; 21 findings; 12 skills found"
    );
    assert!(text.starts_with("bad-yaml\n  bad-frontmatter"), "{text}");
    assert!(
        text.contains("\n  hint            Managed by codex."),
        "{text}"
    );
}

#[test]
fn every_fix_is_pinned_and_none_touches_a_managed_copy() {
    let home = fixture_home();
    let h = home.display().to_string();
    assert!(
        !h.contains('\''),
        "the expected strings below assume no quote"
    );
    let cmd = |kind: &str, skill: &str, fix: String| {
        (
            kind.to_string(),
            skill.to_string(),
            "command".to_string(),
            fix,
        )
    };
    let hint = |kind: &str, skill: &str, fix: String| {
        (kind.to_string(), skill.to_string(), "hint".to_string(), fix)
    };
    let edit = |rel: &str| format!("${{EDITOR:-vi}} '{h}/{rel}/SKILL.md'");
    let unknown = |skill: &str, rel: &str| {
        hint(
            "unknown-source",
            skill,
            format!(
                "No lockfile or plugin records '{h}/{rel}'. If you wrote it, there is \
                 nothing to do. If it came from a repo, back up any local edits, then \
                 reinstall it with the skills CLI so a lockfile records it."
            ),
        )
    };
    let link = cmd(
        "broken-ref",
        "referrer",
        format!("ln -s '{h}/.claude/skills/claude-only' '{h}/.agents/skills/claude-only'"),
    );
    let managed_ref = |agent: &str, missing: &str, rel: &str, manager: &str| {
        hint(
            "broken-ref",
            "referrer",
            format!(
                "{agent} cannot load '{missing}'. A copy is at '{h}/{rel}', managed by \
                 {manager}. Install it where {agent} loads skills. Do not link the \
                 managed copy, since its path changes on update."
            ),
        )
    };
    let off = ".codex/plugins/cache/mkt/off/1.0/skills/off-skill";
    let plugin = ".claude/plugins/cache/market/demo/1.0.0/skills/plugin-skill";
    let diff = |a: &str, b: &str| format!("diff -ru '{h}/{a}' '{h}/{b}'");
    let cgc = diff(
        ".agents/skills/clean-git-commit",
        ".claude/skills/clean-git-commit",
    );

    let expected = vec![
        cmd(
            "bad-frontmatter",
            "bad-yaml",
            edit(".agents/skills/bad-yaml"),
        ),
        unknown("bad-yaml", ".agents/skills/bad-yaml"),
        hint(
            "bad-frontmatter",
            "sys-bad",
            format!(
                "Managed by codex. Report the bad frontmatter in \
                 '{h}/.codex/skills/.system/sys-bad/SKILL.md' to its maintainer. Do not \
                 edit the managed copy."
            ),
        ),
        cmd(
            "name-mismatch",
            "mislabeled",
            edit(".agents/skills/mislabeled"),
        ),
        unknown("mislabeled", ".agents/skills/mislabeled"),
        link.clone(),
        link,
        managed_ref("codex", "off-skill", off, "codex"),
        managed_ref("cursor", "off-skill", off, "codex"),
        managed_ref("codex", "plugin-skill", plugin, "claude-code"),
        managed_ref("cursor", "plugin-skill", plugin, "claude-code"),
        unknown("referrer", ".agents/skills/referrer"),
        cmd("drift", "clean-git-commit", cgc.clone()),
        cmd("duplicate", "clean-git-commit", cgc),
        unknown("clean-git-commit", ".agents/skills/clean-git-commit"),
        unknown("clean-git-commit", ".claude/skills/clean-git-commit"),
        cmd("codex-budget", "", edit(".codex/skills/big")),
        cmd(
            "duplicate",
            "gpt-taste",
            diff(".agents/skills/gpt-taste", ".claude/skills/gpt-taste"),
        ),
        cmd(
            "duplicate",
            "tool-skill",
            diff(
                ".claude/skills/tool-skill",
                ".codex/plugins/cache/mkt/tool/1.0/skills/tool-skill",
            ),
        ),
        unknown("big", ".codex/skills/big"),
        unknown("claude-only", ".claude/skills/claude-only"),
    ];
    let output = lorectl(&home, &["skills", "doctor", "--json"]);
    assert_eq!(output.status.code(), Some(0));
    assert_eq!(fixes(&json(&output)), expected);
}

/// Folder names a shell would read as syntax, with the quoting a reader must
/// see in the fix. Written out by hand so the test does not trust the quoter.
const HOSTILE: &[(&str, &str)] = &[
    ("a b", "a b"),
    ("it's", r"it'\''s"),
    ("x'; touch pwned; '", r"x'\''; touch pwned; '\''"),
    ("amp&er", "amp&er"),
    ("$(touch pwned)", "$(touch pwned)"),
    ("back`touch pwned`tick", "back`touch pwned`tick"),
];

#[cfg(unix)]
#[test]
fn hostile_folder_names_are_quoted_in_every_fix_and_run_as_data() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    for (name, _) in HOSTILE {
        for (root, text) in [
            (".agents/skills", "agents copy"),
            (".claude/skills", "claude copy, edited"),
        ] {
            write(
                &home.join(root).join(name).join("SKILL.md"),
                &format!("---\nname: other\ndescription: {text}\n---\nBody.\n"),
            );
        }
    }
    let h = home.display().to_string();
    let output = lorectl(&home, &["skills", "doctor", "--json"]);
    assert_eq!(output.status.code(), Some(0));
    let got = fixes(&json(&output));

    let mut expected = Vec::new();
    for (name, quoted) in HOSTILE {
        let agents = format!("'{h}/.agents/skills/{quoted}'");
        let claude = format!("'{h}/.claude/skills/{quoted}'");
        let diff = format!("diff -ru {agents} {claude}");
        let edit = |p: &str| format!("${{EDITOR:-vi}} '{h}/{p}/{quoted}/SKILL.md'");
        let unknown = |p: &str| {
            format!(
                "No lockfile or plugin records '{h}/{p}/{quoted}'. If you wrote it, there \
                 is nothing to do. If it came from a repo, back up any local edits, then \
                 reinstall it with the skills CLI so a lockfile records it."
            )
        };
        for (kind, fix_kind, fix) in [
            ("name-mismatch", "command", edit(".agents/skills")),
            ("name-mismatch", "command", edit(".claude/skills")),
            ("drift", "command", diff.clone()),
            ("duplicate", "command", diff),
            ("unknown-source", "hint", unknown(".agents/skills")),
            ("unknown-source", "hint", unknown(".claude/skills")),
        ] {
            expected.push((kind.into(), (*name).into(), fix_kind.into(), fix));
        }
    }
    let mut got_sorted = got.clone();
    got_sorted.sort();
    expected.sort();
    assert_eq!(got_sorted, expected);

    // Run every command with a stand-in editor: each must name exactly one
    // existing path and nothing else may happen.
    for (kind, name, fix_kind, fix) in &got {
        if fix_kind != "command" {
            continue;
        }
        let out = Command::new("sh")
            .arg("-c")
            .arg(fix)
            .current_dir(dir.path())
            .env("HOME", dir.path())
            .env("EDITOR", "printf %s\\n")
            .output()
            .unwrap();
        if kind == "name-mismatch" {
            let printed = String::from_utf8(out.stdout).unwrap();
            let path = Path::new(printed.trim_end_matches('\n'));
            assert!(path.is_file(), "{name}: `{fix}` printed {printed:?}");
            assert_eq!(path.parent().unwrap().file_name().unwrap(), &**name);
        } else {
            // diff exits 1 when the copies differ and 2 when a path is wrong.
            assert_eq!(out.status.code(), Some(1), "{name}: `{fix}`");
        }
    }
    assert!(!dir.path().join("pwned").exists());
    assert!(!home.join("pwned").exists());
}

#[test]
fn claude_config_dir_and_codex_home_move_the_agent_roots() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    let claude = dir.path().join("claude-cfg");
    let codex = dir.path().join("codex-home");
    fs::create_dir_all(&home).unwrap();
    let skill = |name: &str| format!("---\nname: {name}\ndescription: d\n---\nBody.\n");
    write(&claude.join("skills/alpha/SKILL.md"), &skill("alpha"));
    let install = claude.join("plugins/cache/mkt/p/1.0");
    write(&install.join("skills/beta/SKILL.md"), &skill("beta"));
    write(
        &claude.join("plugins/installed_plugins.json"),
        &serde_json::json!({
            "version": 2,
            "plugins": {"p@mkt": [{"installPath": install, "version": "1.0"}]}
        })
        .to_string(),
    );
    write(&codex.join("skills/gamma/SKILL.md"), &skill("gamma"));
    // Ignored: these sit where the defaults would look, not where the
    // overrides point.
    write(&home.join(".claude/skills/stale/SKILL.md"), &skill("stale"));
    write(
        &home.join(".codex/skills/stale2/SKILL.md"),
        &skill("stale2"),
    );

    let output = command(&home, &["--json", "skills", "ls"])
        .env("CLAUDE_CONFIG_DIR", &claude)
        .env("CODEX_HOME", &codex)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(0));
    let got: Vec<(String, String, String)> = json(&output)
        .iter()
        .map(|r| {
            (
                r["name"].as_str().unwrap().to_string(),
                r["installs"][0]["location"].as_str().unwrap().to_string(),
                r["source"]["kind"].as_str().unwrap().to_string(),
            )
        })
        .collect();
    let row = |n: &str, l: &str, s: &str| (n.to_string(), l.to_string(), s.to_string());
    assert_eq!(
        got,
        vec![
            row("alpha", "claude", "unknown"),
            row("beta", "claude-plugins", "plugin"),
            row("gamma", "codex", "unknown"),
        ]
    );
}

/// The content hash `skills ls --json` reports for the one skill in `home`.
fn only_hash(home: &Path) -> String {
    let records = json(&lorectl(home, &["--json", "skills", "ls"]));
    assert_eq!(records.len(), 1);
    records[0]["hash"].as_str().unwrap().to_string()
}

#[cfg(unix)]
#[test]
fn hashing_stays_inside_the_skill_folder() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    let skill = home.join(".claude/skills/s");
    write(
        &skill.join("SKILL.md"),
        "---\nname: s\ndescription: d\n---\n",
    );
    write(&dir.path().join("outside/secret.txt"), "one");
    std::os::unix::fs::symlink(dir.path().join("outside"), skill.join("out")).unwrap();
    let before = only_hash(&home);
    write(&dir.path().join("outside/secret.txt"), "two, longer");
    assert_eq!(
        only_hash(&home),
        before,
        "a link out of the folder was read"
    );
}

#[test]
fn hashing_reads_a_bounded_prefix_of_large_files() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    let skill = home.join(".claude/skills/s");
    write(
        &skill.join("SKILL.md"),
        "---\nname: s\ndescription: d\n---\n",
    );
    let mut bytes = vec![0_u8; 4 << 20];
    fs::write(skill.join("big.bin"), &bytes).unwrap();
    let before = only_hash(&home);
    // Same length, one byte changed past the cap: not read, so not seen.
    *bytes.last_mut().unwrap() = 1;
    fs::write(skill.join("big.bin"), &bytes).unwrap();
    assert_eq!(only_hash(&home), before);
    // A different length is seen.
    bytes.push(0);
    fs::write(skill.join("big.bin"), &bytes).unwrap();
    assert_ne!(only_hash(&home), before);
}

#[test]
fn doctor_json_is_the_same_findings_each_with_a_fix() {
    let output = lorectl(&fixture_home(), &["skills", "doctor", "--json"]);
    assert_eq!(output.status.code(), Some(0));
    let findings = json(&output);
    assert_eq!(findings.len(), 21);
    assert_eq!(findings[0]["kind"], "bad-frontmatter");
}

#[test]
fn ls_json_lists_one_record_per_distinct_content() {
    let output = lorectl(&fixture_home(), &["--json", "skills", "ls"]);
    assert_eq!(output.status.code(), Some(0));
    assert_eq!(json(&output).len(), 13);
}

#[test]
fn an_empty_home_finds_nothing_and_exits_zero() {
    let home = tempfile::tempdir().unwrap();
    let output = lorectl(home.path(), &["skills", "doctor"]);
    assert_eq!(output.status.code(), Some(0));
    assert!(stdout(&output)
        .trim_end()
        .ends_with("0 findings; 0 skills found"));
}

/// Every path below `root` with its modification time and length.
fn snapshot(root: &Path) -> Vec<(PathBuf, SystemTime, u64)> {
    fn walk(dir: &Path, out: &mut Vec<(PathBuf, SystemTime, u64)>) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            let meta = std::fs::symlink_metadata(&path).unwrap();
            out.push((path.clone(), meta.modified().unwrap(), meta.len()));
            if meta.is_dir() {
                walk(&path, out);
            }
        }
    }
    let mut out = Vec::new();
    walk(root, &mut out);
    out.sort();
    out
}

#[test]
fn skills_commands_write_nothing_under_home() {
    let home = fixture_home();
    let before = snapshot(&home);
    for args in [
        &["skills", "doctor"][..],
        &["skills", "doctor", "--json"],
        &["skills", "ls"],
    ] {
        assert_eq!(lorectl(&home, args).status.code(), Some(0));
    }
    assert_eq!(snapshot(&home), before);
}

#[test]
fn a_missing_or_unknown_subcommand_is_a_usage_error() {
    let home = fixture_home();
    for args in [&["skills"][..], &["skills", "fix"]] {
        let output = lorectl(&home, args);
        assert_eq!(output.status.code(), Some(1), "{args:?}");
    }
}

#[test]
fn a_relative_home_is_a_usage_error() {
    let output = lorectl(Path::new("relative/home"), &["skills", "doctor"]);
    assert_eq!(output.status.code(), Some(1));
}
