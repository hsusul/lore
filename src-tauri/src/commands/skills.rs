//! The skills doctor, read for the desktop app.

use std::collections::BTreeMap;
use std::path::Path;

use lore_core::paths::ARCHIVE_DB_FILENAME;
use lore_ipc::{SkillFindingDto, SkillFolderDto, SkillUsageDto, SkillsReportDto};
use lore_memory::mine::{self, Limits};
use lore_memory::skill_use;
use lore_skills::{report, Inventory, Roots};
use rusqlite::Connection;
use tauri::State;

use crate::commands::memory::receipt_dto;
use crate::state::AppState;

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

/// How each installed skill has been used, from the archive: uses, sessions,
/// the newest use, and the corrections right after it ran. A skill that never
/// ran is listed with none. Names that are not installed skills, such as
/// built-in commands, are left out. Most reprompted first, then most used.
#[tauri::command]
pub async fn skill_usage(state: State<'_, AppState>) -> Result<Vec<SkillUsageDto>, String> {
    let roots = Roots::from_env().ok_or("HOME must be set to an absolute path")?;
    let conn = lore_core::storage::open_read_only(&state.archive_dir.join(ARCHIVE_DB_FILENAME))
        .map_err(|e| e.to_string())?;
    usage_for(&conn, &lore_skills::inventory(&roots)).map_err(|e| e.to_string())
}

fn usage_for(conn: &Connection, inventory: &Inventory) -> rusqlite::Result<Vec<SkillUsageDto>> {
    let limits = Limits::default();
    let turns = mine::read_turns(conn)?;
    let mut by_skill: BTreeMap<&str, SkillUsageDto> = inventory
        .records
        .iter()
        .map(|r| {
            let dto = SkillUsageDto {
                skill: r.name.clone(),
                uses: 0,
                sessions: 0,
                last_ms: None,
                reprompts: Vec::new(),
            };
            (r.name.as_str(), dto)
        })
        .collect();
    for used in skill_use::skill_usage(conn, &turns, &limits)? {
        // A plugin's skill is invoked as `plugin:skill`.
        let name = used.skill.rsplit(':').next().unwrap_or(&used.skill);
        let key = if by_skill.contains_key(used.skill.as_str()) {
            used.skill.as_str()
        } else {
            name
        };
        let Some(dto) = by_skill.get_mut(key) else {
            continue;
        };
        dto.uses += i64::try_from(used.uses).unwrap_or(i64::MAX);
        dto.sessions += i64::try_from(used.sessions).unwrap_or(i64::MAX);
        dto.last_ms = dto.last_ms.max(used.last_ms);
        dto.reprompts
            .extend(used.reprompts.into_iter().map(receipt_dto));
    }
    let mut out: Vec<SkillUsageDto> = by_skill.into_values().collect();
    for dto in &mut out {
        dto.reprompts.sort_by_key(|r| std::cmp::Reverse(r.at_ms));
    }
    out.sort_by(|a, b| {
        b.reprompts
            .len()
            .cmp(&a.reprompts.len())
            .then(b.uses.cmp(&a.uses))
            .then(a.skill.cmp(&b.skill))
    });
    Ok(out)
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
    fn usage_lists_every_installed_skill_and_drops_names_that_are_not_skills() {
        let home = tempfile::tempdir().unwrap();
        let skills = home.path().join(".claude/skills");
        skill(&skills, "review", "review");
        skill(&skills, "idle", "idle");
        let conn = crate::commands::memory::tests::archive(
            home.path(),
            &[
                &[
                    ("user", "[$review](/s/review/SKILL.md) the diff"),
                    ("assistant", "Looks fine."),
                    ("user", "no, you missed the tests"),
                ],
                &[("user", "/model fast"), ("assistant", "Switched.")],
            ],
        );
        let used = usage_for(
            &conn,
            &lore_skills::inventory(&Roots::from_home(home.path())),
        )
        .unwrap();
        let rows: Vec<(&str, i64, Vec<&str>)> = used
            .iter()
            .map(|u| {
                (
                    u.skill.as_str(),
                    u.uses,
                    u.reprompts.iter().map(|r| r.text.as_str()).collect(),
                )
            })
            .collect();
        assert_eq!(
            rows,
            [
                ("review", 1, vec!["no, you missed the tests"]),
                ("idle", 0, vec![])
            ]
        );
        assert_eq!(
            used[1].last_ms, None,
            "a skill that never ran has no last use"
        );
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
