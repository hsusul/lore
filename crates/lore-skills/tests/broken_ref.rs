//! Broken reference attribution prefers user-owned (unmanaged) copies so that
//! the suggested fix links the user's copy rather than refusing to link a
//! managed agent copy.
#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::path::{Path, PathBuf};

use lore_skills::{report, Agent, Finding, Fix, Roots};

struct TempHome(PathBuf);

impl TempHome {
    fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "lore-skills-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }

    fn write_skill(&self, relative_dir: &str, name: &str, body: &str) {
        let dir = self.0.join(relative_dir).join(name);
        std::fs::create_dir_all(&dir).unwrap();
        let content = format!("---\nname: {name}\ndescription: {name} skill\n---\n{body}\n");
        std::fs::write(dir.join("SKILL.md"), content).unwrap();
    }
}

impl Drop for TempHome {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn broken_ref_prefers_unmanaged_cursor_copy_over_managed_codex_system() {
    let home = TempHome::new("unmanaged-cursor");
    home.write_skill(".claude/skills", "caller", "Run /helper to assist.");
    home.write_skill(".codex/skills/.system", "helper", "Managed helper.");
    home.write_skill(".cursor/skills", "helper", "User helper.");

    let inventory = lore_skills::inventory(&Roots::from_home(home.path()));
    let findings = lore_skills::doctor(&inventory);
    let broken = findings
        .iter()
        .find(|f| matches!(f, Finding::BrokenRef { missing, agent, .. } if missing == "helper" && *agent == Agent::ClaudeCode))
        .expect("finding for broken ref helper");

    match broken {
        Finding::BrokenRef { found_at, .. } => {
            assert_eq!(found_at, &home.path().join(".cursor/skills/helper"));
        }
        _ => unreachable!(),
    }

    let entries = report::report(&inventory, &findings);
    let entry = entries
        .iter()
        .find(|e| matches!(&e.finding, Finding::BrokenRef { missing, agent, .. } if missing == "helper" && *agent == Agent::ClaudeCode))
        .expect("entry for broken ref");
    assert!(
        matches!(&entry.fix, Fix::Command(cmd) if cmd.contains(".cursor/skills/helper")),
        "expected Fix::Command referencing cursor copy, got {:?}",
        entry.fix
    );
}

#[test]
fn broken_ref_prefers_unmanaged_vendor_copy_over_managed_codex_system() {
    let home = TempHome::new("unmanaged-vendor");
    home.write_skill(".claude/skills", "caller", "Run /helper to assist.");
    home.write_skill(".codex/skills/.system", "helper", "Managed helper.");
    home.write_skill(".agents/vendor/pack/skills", "helper", "Vendor helper.");

    let inventory = lore_skills::inventory(&Roots::from_home(home.path()));
    let findings = lore_skills::doctor(&inventory);
    let broken = findings
        .iter()
        .find(|f| matches!(f, Finding::BrokenRef { missing, agent, .. } if missing == "helper" && *agent == Agent::ClaudeCode))
        .expect("finding for broken ref helper");

    match broken {
        Finding::BrokenRef { found_at, .. } => {
            assert_eq!(
                found_at,
                &home.path().join(".agents/vendor/pack/skills/helper")
            );
        }
        _ => unreachable!(),
    }

    let entries = report::report(&inventory, &findings);
    let entry = entries
        .iter()
        .find(|e| matches!(&e.finding, Finding::BrokenRef { missing, agent, .. } if missing == "helper" && *agent == Agent::ClaudeCode))
        .expect("entry for broken ref");
    assert!(
        matches!(&entry.fix, Fix::Command(cmd) if cmd.contains(".agents/vendor/pack/skills/helper")),
        "expected Fix::Command referencing vendor copy, got {:?}",
        entry.fix
    );
}

#[test]
fn broken_ref_attributes_to_managed_copy_when_no_unmanaged_copy_exists() {
    let home = TempHome::new("managed-only");
    home.write_skill(".claude/skills", "caller", "Run /helper to assist.");
    home.write_skill(".codex/skills/.system", "helper", "Managed helper.");

    let inventory = lore_skills::inventory(&Roots::from_home(home.path()));
    let findings = lore_skills::doctor(&inventory);
    let broken = findings
        .iter()
        .find(|f| matches!(f, Finding::BrokenRef { missing, agent, .. } if missing == "helper" && *agent == Agent::ClaudeCode))
        .expect("finding for broken ref helper");

    match broken {
        Finding::BrokenRef { found_at, .. } => {
            assert_eq!(found_at, &home.path().join(".codex/skills/.system/helper"));
        }
        _ => unreachable!(),
    }

    let entries = report::report(&inventory, &findings);
    let entry = entries
        .iter()
        .find(|e| matches!(&e.finding, Finding::BrokenRef { missing, agent, .. } if missing == "helper" && *agent == Agent::ClaudeCode))
        .expect("entry for broken ref");
    assert!(
        matches!(&entry.fix, Fix::Hint(hint) if hint.contains("managed by codex")),
        "expected Fix::Hint for managed copy, got {:?}",
        entry.fix
    );
}
