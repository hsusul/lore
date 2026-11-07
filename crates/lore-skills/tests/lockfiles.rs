//! The provenance files, read from a redacted copy of real ones.
#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::path::{Path, PathBuf};

use lore_skills::lockfiles;
use lore_skills::{Roots, Source};

fn home() -> Roots {
    Roots::from_home(&Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/home"))
}

#[test]
fn lockfiles_v3_skill_lock_gives_url_and_folder_hash_per_skill() {
    let locks = lockfiles::read(&home());
    assert_eq!(
        locks.tracked.get("brandkit"),
        Some(&Source::Git {
            url: "https://github.com/leonxlnx/taste-skill.git".into(),
            rev: "d278ff748cafab01e4ae5dd7758fc0b53258621c".into(),
            subpath: "skills/brandkit/SKILL.md".into(),
        })
    );
    assert_eq!(
        locks.tracked.get("gpt-taste"),
        Some(&Source::Git {
            url: "https://github.com/leonxlnx/taste-skill.git".into(),
            rev: "c87d529ad945cd35985b19475bf53f41682d4f69".into(),
            subpath: "skills/gpt-tasteskill/SKILL.md".into(),
        })
    );
}

#[test]
fn lockfiles_opik_manifest_tracks_its_listed_skills() {
    let locks = lockfiles::read(&home());
    assert_eq!(
        locks.tracked.get("opik"),
        Some(&Source::Git {
            url: "comet-ml/opik-skills".into(),
            rev: "main".into(),
            subpath: String::new(),
        })
    );
}

#[test]
fn lockfiles_plugin_manifests_name_what_is_installed_or_enabled() {
    let locks = lockfiles::read(&home());
    assert_eq!(
        locks.claude_plugins,
        vec![(
            "market/demo/1.0.0".into(),
            Source::Plugin {
                marketplace: "market".into(),
                plugin: "demo".into(),
                version: "1.0.0".into(),
            }
        )]
    );
    assert_eq!(
        locks.codex_enabled.into_iter().collect::<Vec<_>>(),
        ["tool@mkt"]
    );
}

#[test]
fn lockfiles_that_are_missing_or_malformed_record_nothing() {
    let locks = lockfiles::read(&Roots::from_home(Path::new(
        "/nonexistent/lore-skills-home",
    )));
    assert_eq!(locks, lockfiles::Lockfiles::default());
}

struct TempHome(std::path::PathBuf);

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
}

impl Drop for TempHome {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn lockfiles_claude_plugins_splits_scoped_plugin_at_last_at_sign() {
    let temp = TempHome::new("scoped-plugin");
    let claude = temp.path().join("claude");
    let cache = claude.join("plugins/cache");
    let install = cache.join("market/@scope/pkg/1.0.0");
    std::fs::create_dir_all(&install).unwrap();
    std::fs::write(
        claude.join("plugins/installed_plugins.json"),
        serde_json::json!({
            "version": 2,
            "plugins": {
                "@scope/pkg@market": [{
                    "installPath": install,
                    "version": "1.0.0"
                }]
            }
        })
        .to_string(),
    )
    .unwrap();

    let roots = Roots::resolve(temp.path(), Some(claude), None);
    let locks = lockfiles::read(&roots);
    assert_eq!(
        locks.claude_plugins,
        vec![(
            "market/@scope/pkg/1.0.0".into(),
            Source::Plugin {
                marketplace: "market".into(),
                plugin: "@scope/pkg".into(),
                version: "1.0.0".into(),
            }
        )]
    );
}

#[test]
fn lockfiles_v3_skill_lock_tracks_local_source_type() {
    let temp = TempHome::new("local-skill");
    let agents = temp.path().join(".agents");
    std::fs::create_dir_all(&agents).unwrap();
    std::fs::write(
        agents.join(".skill-lock.json"),
        serde_json::json!({
            "version": 3,
            "skills": {
                "local-tool": {
                    "source": "/path/to/tool",
                    "sourceType": "local"
                }
            }
        })
        .to_string(),
    )
    .unwrap();

    let roots = Roots::from_home(temp.path());
    let locks = lockfiles::read(&roots);
    assert_eq!(
        locks.tracked.get("local-tool"),
        Some(&Source::Local {
            path: PathBuf::from("/path/to/tool"),
        })
    );
}
