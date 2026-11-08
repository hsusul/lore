//! Skills in agents' plugin caches count as loadable only for the agent that
//! owns the cache, only while the plugin is installed and enabled, and only
//! at the version that agent loads. Temp homes follow the real cache layout
//! with made-up names.
#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::path::{Path, PathBuf};

use lore_skills::lockfiles;
use lore_skills::{Agent, Finding, Roots};

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

    fn write(&self, relative: &str, content: &str) {
        let path = self.0.join(relative);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, content).unwrap();
    }

    fn skill(&self, dir: &str, name: &str, body: &str) {
        self.write(
            &format!("{dir}/{name}/SKILL.md"),
            &format!("---\nname: {name}\ndescription: {name} skill\n---\n{body}\n"),
        );
    }

    /// A plugin installed from a remote catalog: Codex writes a marker next
    /// to the version folders and nothing in `config.toml`.
    fn remote_marker(&self, marketplace: &str, plugin: &str) {
        self.write(
            &format!(
                ".codex/plugins/cache/{marketplace}/{plugin}/.codex-remote-plugin-install.json"
            ),
            r#"{"schema_version": 1, "remote_plugin_id": "plugin_connector_x"}"#,
        );
    }

    fn findings(&self) -> Vec<Finding> {
        lore_skills::doctor(&lore_skills::inventory(&Roots::from_home(self.path())))
    }
}

impl Drop for TempHome {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn broken(findings: &[Finding]) -> Vec<(String, String, Agent)> {
    let mut out: Vec<_> = findings
        .iter()
        .filter_map(|f| match f {
            Finding::BrokenRef {
                skill,
                missing,
                agent,
                ..
            } => Some((skill.clone(), missing.clone(), *agent)),
            _ => None,
        })
        .collect();
    out.sort();
    out
}

const CACHE: &str = ".codex/plugins/cache";

/// The layout on a real machine: two plugins enabled in `config.toml`, two
/// installed from the remote catalog, one of the enabled ones referencing
/// skills of a remote one.
fn codex_home(name: &str) -> TempHome {
    let home = TempHome::new(name);
    home.write(
        ".codex/config.toml",
        "[plugins.\"maker@runtime-mkt\"]\nenabled = true\n\n[plugins.\"chart@bundled-mkt\"]\nenabled = true\n",
    );
    home.skill(
        &format!("{CACHE}/runtime-mkt/maker/26.1.2/skills"),
        "maker",
        "Use /drive or /sheet to fetch the source.",
    );
    home.skill(
        &format!("{CACHE}/bundled-mkt/chart/1.0.10/skills"),
        "chart",
        "Publish with /pages.",
    );
    home.remote_marker("remote-mkt", "drive");
    home.skill(
        &format!("{CACHE}/remote-mkt/drive/0.1.22/skills"),
        "drive",
        "Drive.",
    );
    home.skill(
        &format!("{CACHE}/remote-mkt/drive/0.1.22/skills"),
        "sheet",
        "Sheet.",
    );
    home.remote_marker("remote-mkt", "pages");
    home.skill(
        &format!("{CACHE}/remote-mkt/pages/1.1.4/skills"),
        "pages",
        "Pages.",
    );
    home
}

#[test]
fn codex_loads_skills_of_remote_installed_plugins() {
    let home = codex_home("codex-remote");
    let locks = lockfiles::read(&Roots::from_home(home.path()));
    assert_eq!(
        locks
            .codex_enabled
            .get("drive@remote-mkt")
            .map(String::as_str),
        Some("0.1.22")
    );
    assert_eq!(broken(&home.findings()), []);
}

#[test]
fn other_agents_still_cannot_load_a_codex_plugin_skill() {
    let home = codex_home("codex-other-agents");
    home.skill(".claude/skills", "relish", "Draw it with /chart.");
    home.skill(".cursor/skills", "relish", "Draw it with /chart.");
    assert_eq!(
        broken(&home.findings()),
        [
            ("relish".into(), "chart".into(), Agent::ClaudeCode),
            ("relish".into(), "chart".into(), Agent::Cursor),
        ]
    );
}

#[test]
fn codex_config_disabling_a_remote_plugin_keeps_the_finding() {
    let home = codex_home("codex-remote-disabled");
    home.write(
        ".codex/config.toml",
        "[plugins.\"maker@runtime-mkt\"]\nenabled = true\n\n[plugins.\"chart@bundled-mkt\"]\nenabled = true\n\n[plugins.\"drive@remote-mkt\"]\nenabled = false\n",
    );
    assert_eq!(
        broken(&home.findings()),
        [
            ("maker".into(), "drive".into(), Agent::Codex),
            ("maker".into(), "sheet".into(), Agent::Codex),
        ]
    );
}

#[test]
fn codex_cached_plugin_neither_enabled_nor_remote_installed_keeps_the_finding() {
    let home = codex_home("codex-not-installed");
    std::fs::remove_file(
        home.path()
            .join(CACHE)
            .join("remote-mkt/pages/.codex-remote-plugin-install.json"),
    )
    .unwrap();
    assert_eq!(
        broken(&home.findings()),
        [("chart".into(), "pages".into(), Agent::Codex)]
    );
}

#[test]
fn codex_loads_only_the_active_version_of_a_plugin() {
    let home = codex_home("codex-versions");
    // An older version still cached holds a skill the active one dropped,
    // and a `latest` link must not count as a version of its own.
    home.skill(
        &format!("{CACHE}/bundled-mkt/chart/1.0.9/skills"),
        "chart",
        "Old.",
    );
    home.skill(
        &format!("{CACHE}/bundled-mkt/chart/1.0.9/skills"),
        "legacy",
        "Old.",
    );
    home.skill(
        &format!("{CACHE}/runtime-mkt/maker/26.1.2/skills"),
        "maker",
        "Use /drive, /sheet, or /legacy.",
    );
    #[cfg(unix)]
    std::os::unix::fs::symlink(
        home.path().join(CACHE).join("bundled-mkt/chart/1.0.9"),
        home.path().join(CACHE).join("bundled-mkt/chart/latest"),
    )
    .unwrap();

    let locks = lockfiles::read(&Roots::from_home(home.path()));
    assert_eq!(
        locks
            .codex_enabled
            .get("chart@bundled-mkt")
            .map(String::as_str),
        Some("1.0.10")
    );
    assert_eq!(
        broken(&home.findings()),
        [("maker".into(), "legacy".into(), Agent::Codex)]
    );
}

#[test]
fn codex_active_version_follows_semver_and_prefers_local() {
    let home = TempHome::new("codex-active-version");
    let plugin = home.path().join("plugin");
    for version in ["0.1.13-abc", "0.1.9", "0.1.13", "0.1.13-77c"] {
        std::fs::create_dir_all(plugin.join(version)).unwrap();
    }
    assert_eq!(
        lockfiles::codex_active_version(&plugin).as_deref(),
        Some("0.1.13")
    );
    std::fs::create_dir_all(plugin.join("local")).unwrap();
    assert_eq!(
        lockfiles::codex_active_version(&plugin).as_deref(),
        Some("local")
    );
}

fn claude_home(name: &str, enabled: bool) -> TempHome {
    let home = TempHome::new(name);
    let install = home.path().join(".claude/plugins/cache/mkt/toolkit/1.2.0");
    home.skill(
        ".claude/plugins/cache/mkt/toolkit/1.2.0/skills",
        "deploy",
        "Deploy.",
    );
    home.write(
        ".claude/plugins/installed_plugins.json",
        &serde_json::json!({
            "version": 2,
            "plugins": {
                "toolkit@mkt": [{
                    "scope": "user",
                    "installPath": install,
                    "version": "1.2.0"
                }]
            }
        })
        .to_string(),
    );
    home.write(
        ".claude/settings.json",
        &serde_json::json!({ "enabledPlugins": { "toolkit@mkt": enabled } }).to_string(),
    );
    home.skill(".claude/skills", "ship", "Then run /deploy.");
    home
}

#[test]
fn claude_loads_skills_of_enabled_plugins() {
    let home = claude_home("claude-enabled", true);
    assert_eq!(broken(&home.findings()), []);
}

#[test]
fn claude_plugin_disabled_in_settings_keeps_the_finding() {
    let home = claude_home("claude-disabled", false);
    assert!(lockfiles::read(&Roots::from_home(home.path()))
        .claude_plugins
        .is_empty());
    assert_eq!(
        broken(&home.findings()),
        [("ship".into(), "deploy".into(), Agent::ClaudeCode)]
    );
}
