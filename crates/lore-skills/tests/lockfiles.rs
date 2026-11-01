//! The provenance files, read from a redacted copy of real ones.

use std::path::Path;

use lore_skills::lockfiles;
use lore_skills::Source;

fn home() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/home")
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
    let locks = lockfiles::read(Path::new("/nonexistent/lore-skills-home"));
    assert_eq!(locks, lockfiles::Lockfiles::default());
}
