//! The doctor's exact findings for the fixture home, which holds at least one
//! of every variant. Read-only: the fixture is never written.
#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::path::{Path, PathBuf};

use lore_skills::{Agent, DriftCopy, Finding, Inventory, Roots, CODEX_BUDGET_CHARS};

fn home() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/home")
}

fn hash_of(inventory: &Inventory, path: &Path) -> lore_skills::ContentHash {
    inventory
        .records
        .iter()
        .flat_map(|r| &r.installs)
        .find(|i| i.path == path)
        .map(|i| i.hash.clone())
        .expect("installed")
}

#[test]
fn the_fixture_home_yields_exactly_these_findings_worst_first() {
    let home = home();
    let inventory = lore_skills::inventory(&Roots::from_home(&home));
    let at = |p: &str| home.join(p);
    let agents_cgc = at(".agents/skills/clean-git-commit");
    let claude_cgc = at(".claude/skills/clean-git-commit");
    let broken = |missing: &str, agent, found: &str| Finding::BrokenRef {
        skill: "referrer".into(),
        missing: missing.into(),
        agent,
        path: at(".agents/skills/referrer"),
        found_at: at(found),
    };
    let unknown = |skill: &str, path: &str| Finding::UnknownSource {
        skill: skill.into(),
        path: at(path),
    };
    let off = ".codex/plugins/cache/mkt/off/1.0/skills/off-skill";
    let plugin = ".claude/plugins/cache/market/demo/1.0.0/skills/plugin-skill";
    let claude_only = ".claude/skills/claude-only";
    let unclosed = "frontmatter `description` has an unclosed `\"`";

    // No NameMismatch for hf-style: a vendor's name in a managed cache is
    // the vendor's choice, and nothing the user runs can change it.
    let expected = vec![
        Finding::BadFrontmatter {
            skill: "bad-yaml".into(),
            path: at(".agents/skills/bad-yaml"),
            reason: unclosed.into(),
        },
        unknown("bad-yaml", ".agents/skills/bad-yaml"),
        Finding::BadFrontmatter {
            skill: "sys-bad".into(),
            path: at(".codex/skills/.system/sys-bad"),
            reason: unclosed.into(),
        },
        Finding::NameMismatch {
            skill: "mislabeled".into(),
            declared: "other-name".into(),
            path: at(".agents/skills/mislabeled"),
        },
        unknown("mislabeled", ".agents/skills/mislabeled"),
        broken("claude-only", Agent::Codex, claude_only),
        broken("claude-only", Agent::Cursor, claude_only),
        broken("off-skill", Agent::Codex, off),
        broken("off-skill", Agent::Cursor, off),
        broken("plugin-skill", Agent::Codex, plugin),
        broken("plugin-skill", Agent::Cursor, plugin),
        unknown("referrer", ".agents/skills/referrer"),
        Finding::Drift {
            skill: "clean-git-commit".into(),
            copies: vec![
                DriftCopy {
                    path: agents_cgc.clone(),
                    hash: hash_of(&inventory, &agents_cgc),
                    lines: 5,
                },
                DriftCopy {
                    path: claude_cgc.clone(),
                    hash: hash_of(&inventory, &claude_cgc),
                    lines: 7,
                },
            ],
        },
        Finding::Duplicate {
            skill: "clean-git-commit".into(),
            paths: vec![agents_cgc.clone(), claude_cgc.clone()],
        },
        unknown("clean-git-commit", ".agents/skills/clean-git-commit"),
        unknown("clean-git-commit", ".claude/skills/clean-git-commit"),
        Finding::CodexBudget {
            chars: 8349,
            limit: CODEX_BUDGET_CHARS,
            skills: 10,
            largest: at(".codex/skills/big"),
        },
        Finding::Duplicate {
            skill: "gpt-taste".into(),
            paths: vec![
                at(".agents/skills/gpt-taste"),
                at(".claude/skills/gpt-taste"),
            ],
        },
        Finding::Duplicate {
            skill: "tool-skill".into(),
            paths: vec![
                at(".claude/skills/tool-skill"),
                at(".codex/plugins/cache/mkt/tool/1.0/skills/tool-skill"),
            ],
        },
        unknown("big", ".codex/skills/big"),
        unknown("claude-only", claude_only),
    ];
    assert_eq!(lore_skills::doctor(&inventory), expected);
}

#[test]
fn a_link_to_one_folder_is_one_copy_not_a_duplicate() {
    let inventory = lore_skills::inventory(&Roots::from_home(&home()));
    let linked: Vec<_> = inventory
        .records
        .iter()
        .filter(|r| r.name == "linked")
        .collect();
    assert_eq!(linked.len(), 1);
    assert!(matches!(
        linked[0].installs[0].mode,
        lore_skills::Mode::Symlink { .. }
    ));
    assert!(lore_skills::doctor(&inventory)
        .iter()
        .all(|f| f.skill() != Some("linked")));
}

#[test]
fn skills_no_agent_loads_are_known_but_not_reported() {
    let inventory = lore_skills::inventory(&Roots::from_home(&home()));
    assert!(inventory.elsewhere.contains_key("off-skill"));
    assert!(inventory.elsewhere.contains_key("linked"));
    assert!(inventory.records.iter().all(|r| r.name != "off-skill"));
}

#[test]
fn a_missing_home_is_an_empty_report_not_an_error() {
    let inventory = lore_skills::inventory(&Roots::from_home(Path::new(
        "/nonexistent/lore-skills-home",
    )));
    assert!(inventory.records.is_empty());
    assert!(lore_skills::doctor(&inventory).is_empty());
}

#[test]
fn unclosed_frontmatter_fence_is_reported_without_double_frontmatter() {
    let tmp = std::env::temp_dir().join(format!("lore-skills-unclosed-{}", std::process::id()));
    let skill_dir = tmp.join(".agents/skills/unclosed-skill");
    std::fs::create_dir_all(&skill_dir).unwrap();
    std::fs::write(skill_dir.join("SKILL.md"), "---\nname: unclosed-skill\n").unwrap();
    let inventory = lore_skills::inventory(&Roots::from_home(&tmp));
    let _ = std::fs::remove_dir_all(&tmp);
    let bad = inventory
        .records
        .iter()
        .find(|r| r.name == "unclosed-skill")
        .expect("record for unclosed skill");
    assert_eq!(
        bad.frontmatter_error.as_deref(),
        Some("frontmatter has no closing `---`")
    );
}
