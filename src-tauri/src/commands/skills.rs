//! The skills doctor, read for the desktop app.

use std::collections::BTreeMap;
use std::path::Path;

use lore_core::paths::ARCHIVE_DB_FILENAME;
use lore_ipc::{
    ReceiptDto, SkillDraftDto, SkillFindingDto, SkillFolderDto, SkillUsageDto, SkillsReportDto,
};
use lore_memory::mine::{self, Limits};
use lore_memory::skill_use;
use lore_skills::{report, Inventory, Roots};
use rusqlite::Connection;
use tauri::State;

use crate::commands::lessons::{self, Agent};
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
                editable_path: editable_path(r).map(|p| p.display().to_string()),
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

/// At most this many corrections go into a draft's prompt, newest first.
const DRAFT_REPROMPTS: usize = 8;

/// What the agent is asked, for the skill at `path` holding `content`.
fn draft_prompt(name: &str, path: &Path, content: &str, reprompts: &[ReceiptDto]) -> String {
    let mut corrections = String::new();
    for r in reprompts.iter().take(DRAFT_REPROMPTS) {
        if let Some(before) = &r.preceding {
            corrections.push_str(&format!("- The agent had just said: \"{before}\"\n  "));
        } else {
            corrections.push_str("- ");
        }
        corrections.push_str(&format!("The person then said: \"{}\"\n", r.text));
    }
    format!(
        "You are improving an agent skill. Right after an agent ran this skill, the person \
had to correct it. Rewrite the skill so the agent would have done it right the first time.\n\n\
Rules:\n\
- Change only what the corrections show is wrong or missing. Keep everything else word for \
word, frontmatter included.\n\
- Add nothing the corrections do not support.\n\
- If the corrections are not about what the skill tells the agent to do, change nothing.\n\
- Do not use tools or edit files; reply with text only.\n\n\
Reply in exactly this form and nothing else:\n\
SUMMARY: <one sentence on what you changed and why, or No change needed.>\n\
<<<SKILL.md\n<the complete new SKILL.md>\nSKILL.md>>>\n\n\
The skill `{name}`, at {path}:\n<<<SKILL.md\n{content}\nSKILL.md>>>\n\n\
Corrections right after it ran, newest first:\n{corrections}",
        path = path.display()
    )
}

/// The summary and new SKILL.md from the agent's reply.
fn parse_draft(reply: &str) -> Result<(String, String), String> {
    let summary = reply
        .lines()
        .find_map(|l| l.trim().strip_prefix("SUMMARY:"))
        .map(|s| s.trim().to_owned())
        .ok_or("the agent's reply had no SUMMARY line")?;
    let start = reply
        .rfind("<<<SKILL.md\n")
        .ok_or("the agent's reply had no new SKILL.md")?
        + "<<<SKILL.md\n".len();
    let end = reply[start..]
        .find("\nSKILL.md>>>")
        .ok_or("the agent's reply had no new SKILL.md")?;
    Ok((
        summary,
        format!("{}\n", reply[start..start + end].trim_end()),
    ))
}

/// The installed skill `name` that Lore may edit, and its SKILL.md.
fn editable(name: &str) -> Result<std::path::PathBuf, String> {
    let roots = Roots::from_env().ok_or("HOME must be set to an absolute path")?;
    let inventory = lore_skills::inventory(&roots);
    let record = inventory
        .records
        .iter()
        .find(|r| r.name.as_str() == name)
        .ok_or_else(|| format!("no installed skill `{name}`"))?;
    editable_path(record).ok_or_else(|| {
        format!("`{name}` is managed by a plugin or a source Lore does not own, so an edit would be undone")
    })
}

/// Ask the person's `agent` to rewrite the skill `name` from the corrections
/// made right after it ran. Writes nothing; the person reviews the draft.
#[tauri::command]
pub async fn draft_skill_edit(
    state: State<'_, AppState>,
    agent: String,
    name: String,
) -> Result<SkillDraftDto, String> {
    let agent = Agent::parse(&agent).ok_or("unknown agent")?;
    let path = editable(&name)?;
    let before = std::fs::read_to_string(&path).map_err(|e| e.to_string())?;
    let conn = lore_core::storage::open_read_only(&state.archive_dir.join(ARCHIVE_DB_FILENAME))
        .map_err(|e| e.to_string())?;
    let roots = Roots::from_env().ok_or("HOME must be set to an absolute path")?;
    let reprompts = usage_for(&conn, &lore_skills::inventory(&roots))
        .map_err(|e| e.to_string())?
        .into_iter()
        .find(|u| u.skill == name)
        .map(|u| u.reprompts)
        .unwrap_or_default();
    if reprompts.is_empty() {
        return Err(format!("no corrections were made right after `{name}` ran"));
    }
    let prompt = draft_prompt(&name, &path, &before, &reprompts);
    let (binary, home, search) = lessons::locate(agent)?;
    let last = state.archive_dir.join("skill-draft.last.txt");
    let args = agent.text_only_arguments(&last);
    let reply = tauri::async_runtime::spawn_blocking(move || {
        lessons::run(agent, &binary, &args, &prompt, &home, &search, &last)
    })
    .await
    .map_err(|e| e.to_string())??;
    let (summary, after) = parse_draft(&reply)?;
    Ok(SkillDraftDto {
        path: path.display().to_string(),
        summary,
        before,
        after,
    })
}

/// Write a draft the person approved. Refused when the file changed since
/// the draft was made. The old SKILL.md is kept under the archive's
/// `skill-backups`. Returns where the backup is.
#[tauri::command]
pub async fn apply_skill_edit(
    state: State<'_, AppState>,
    name: String,
    before: String,
    after: String,
) -> Result<String, String> {
    let path = editable(&name)?;
    apply_in(
        &path,
        &state.archive_dir.join("skill-backups"),
        &name,
        &before,
        &after,
    )
}

fn apply_in(
    path: &Path,
    backups: &Path,
    name: &str,
    before: &str,
    after: &str,
) -> Result<String, String> {
    let now = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
    if now != before {
        return Err(format!(
            "{} changed since the draft was made; draft it again",
            path.display()
        ));
    }
    std::fs::create_dir_all(backups).map_err(|e| e.to_string())?;
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis());
    let backup = backups.join(format!("{name}-{stamp}.SKILL.md"));
    std::fs::write(&backup, &now).map_err(|e| e.to_string())?;
    std::fs::write(path, after).map_err(|e| e.to_string())?;
    Ok(backup.display().to_string())
}

/// The SKILL.md of a skill the person owns. A plugin's, a git source's or an
/// agent's built-in skill is not: its next update would undo an edit.
pub(crate) fn editable_path(
    record: &lore_skills::model::SkillRecord,
) -> Option<std::path::PathBuf> {
    use lore_skills::model::Source;
    if !matches!(record.source, Source::Local { .. } | Source::Unknown) {
        return None;
    }
    record
        .installs
        .first()
        .map(|install| install.real_path.join("SKILL.md"))
        .filter(|path| path.is_file())
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
    fn a_skill_the_person_owns_is_editable_and_its_path_is_listed() {
        let home = tempfile::tempdir().unwrap();
        let skills = home.path().join(".claude/skills");
        skill(&skills, "review", "review");
        let conn = crate::commands::memory::tests::archive(home.path(), &[&[("user", "hi")]]);
        let used = usage_for(
            &conn,
            &lore_skills::inventory(&Roots::from_home(home.path())),
        )
        .unwrap();
        let path = used[0].editable_path.clone().unwrap();
        assert!(path.ends_with(".claude/skills/review/SKILL.md"), "{path}");
    }

    #[test]
    fn a_draft_prompt_carries_the_skill_and_what_was_said_after_it() {
        let reprompt = ReceiptDto {
            session_id: "s1".to_owned(),
            agent_id: "claude-code".to_owned(),
            repo: None,
            at_ms: None,
            text: "no, you missed the tests".to_owned(),
            preceding: Some("Reviewed it, looks fine.".to_owned()),
        };
        let prompt = draft_prompt(
            "review",
            Path::new("/s/review/SKILL.md"),
            "Body.\n",
            &[reprompt],
        );
        assert!(
            prompt.contains(
                "The skill `review`, at /s/review/SKILL.md:\n<<<SKILL.md\nBody.\n\nSKILL.md>>>"
            ),
            "{prompt}"
        );
        assert!(prompt.ends_with(
            "- The agent had just said: \"Reviewed it, looks fine.\"\n  The person then said: \"no, you missed the tests\"\n"
        ));
    }

    #[test]
    fn a_draft_is_read_from_the_last_skill_block_of_the_reply() {
        let reply = "Here it is.\nSUMMARY: Check the tests before approving.\n<<<SKILL.md\n---\nname: review\n---\nRead the tests too.\nSKILL.md>>>\n";
        assert_eq!(
            parse_draft(reply),
            Ok((
                "Check the tests before approving.".to_owned(),
                "---\nname: review\n---\nRead the tests too.\n".to_owned()
            ))
        );
        assert_eq!(
            parse_draft("no format"),
            Err("the agent's reply had no SUMMARY line".to_owned())
        );
        assert_eq!(
            parse_draft("SUMMARY: x"),
            Err("the agent's reply had no new SKILL.md".to_owned())
        );
    }

    #[test]
    fn applying_keeps_a_backup_and_refuses_a_file_that_changed() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("SKILL.md");
        std::fs::write(&path, "old\n").unwrap();
        let backups = dir.path().join("backups");
        assert_eq!(
            apply_in(&path, &backups, "review", "older\n", "new\n"),
            Err(format!(
                "{} changed since the draft was made; draft it again",
                path.display()
            ))
        );
        let backup = apply_in(&path, &backups, "review", "old\n", "new\n").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "new\n");
        assert_eq!(std::fs::read_to_string(&backup).unwrap(), "old\n");
        assert!(backup.contains("/backups/review-"), "{backup}");
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
