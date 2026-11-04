//! The skills doctor, read for the desktop app.

use std::collections::BTreeMap;
use std::path::Path;

use lore_ipc::{SkillFindingDto, SkillFolderDto, SkillsReportDto};
use lore_skills::{report, Inventory, Roots};

/// What the skills doctor finds on this machine, as `lorectl skills doctor`
/// reports it: every finding worst first, each with the fix it suggests, and
/// how many skills each folder holds. Read only, and `async`: it walks every
/// skill folder an agent loads.
#[tauri::command]
pub async fn skills_report() -> Result<SkillsReportDto, String> {
    let roots = Roots::from_env().ok_or("HOME must be set to an absolute path")?;
    Ok(report_for(&lore_skills::inventory(&roots)))
}

fn report_for(inventory: &Inventory) -> SkillsReportDto {
    let findings = lore_skills::doctor(inventory);
    let home = &inventory.roots.home;
    let mut folders: BTreeMap<String, i64> = BTreeMap::new();
    for record in &inventory.records {
        for install in &record.installs {
            if let Some(dir) = install.path.parent() {
                *folders.entry(tilde(home, dir)).or_default() += 1;
            }
        }
    }
    SkillsReportDto {
        skills: i64::try_from(report::distinct_names(inventory)).unwrap_or(i64::MAX),
        folders: folders
            .into_iter()
            .map(|(path, skills)| SkillFolderDto { path, skills })
            .collect(),
        findings: report::report(inventory, &findings)
            .into_iter()
            .map(|entry| SkillFindingDto {
                kind: entry.finding.kind().to_owned(),
                skill: entry.finding.skill().map(str::to_owned),
                description: entry.description,
                fix: entry.fix.text().to_owned(),
                fix_kind: entry.fix.kind().to_owned(),
            })
            .collect(),
    }
}

fn tilde(home: &Path, path: &Path) -> String {
    match path.strip_prefix(home) {
        Ok(rest) => format!("~/{}", rest.display()),
        Err(_) => path.display().to_string(),
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    fn skill(dir: &Path, folder: &str, declared: &str) {
        let at = dir.join(folder);
        std::fs::create_dir_all(&at).unwrap();
        std::fs::write(
            at.join("SKILL.md"),
            format!("---\nname: {declared}\ndescription: A test skill.\n---\nBody.\n"),
        )
        .unwrap();
    }

    #[test]
    fn a_name_mismatch_is_reported_with_its_fix_and_folders_are_counted() {
        let home = tempfile::tempdir().unwrap();
        let skills = home.path().join(".claude/skills");
        skill(&skills, "taste-skill", "taste");
        skill(&skills, "review", "review");
        let report = report_for(&lore_skills::inventory(&Roots::from_home(home.path())));

        assert_eq!(report.skills, 2);
        assert_eq!(
            report.folders,
            [SkillFolderDto {
                path: "~/.claude/skills".to_owned(),
                skills: 2
            }]
        );
        let mismatch: Vec<_> = report
            .findings
            .iter()
            .filter(|f| f.kind == "name-mismatch")
            .map(|f| f.skill.as_deref())
            .collect();
        assert_eq!(mismatch, [Some("taste-skill")]);
        assert!(report
            .findings
            .iter()
            .all(|f| !f.fix.is_empty() && !f.description.is_empty()));
    }
}
