use lore_ipc::{LessonDto, LessonReceiptDto};
use lore_memory::mine::{self, Lesson, Signal};
use tauri::State;

use crate::state::AppState;

/// Lessons mined from the archive's corrections, best first, without the ones
/// that scored as one-off task orders (as `lorectl mem mine` lists them).
///
/// `async` so mining a large archive runs off the main thread, and on its own
/// read-only connection so it never holds the UI's database lock.
#[tauri::command]
pub async fn list_lessons(
    state: State<'_, AppState>,
    limit: Option<u32>,
) -> Result<Vec<LessonDto>, String> {
    let db = state
        .archive_dir
        .join(lore_core::paths::ARCHIVE_DB_FILENAME);
    let conn = lore_core::storage::open_read_only(&db).map_err(|e| e.to_string())?;
    let limits = mine::Limits::default();
    let candidates = mine::corrections_within(&conn, &limits).map_err(|e| e.to_string())?;
    let mut lessons = mine::lessons_within(candidates, &limits);
    lessons.retain(|l| l.score >= 0);
    lessons.truncate(limit.map_or(usize::MAX, |n| n as usize));
    Ok(lessons.iter().map(lesson_dto).collect())
}

fn lesson_dto(lesson: &Lesson) -> LessonDto {
    LessonDto {
        id: lesson.id.clone(),
        text: lesson.text.clone(),
        suggested_rule: mine::suggest_rule(&lesson.text),
        conversations: i64::try_from(lesson.conversations).unwrap_or(i64::MAX),
        latest_ms: lesson.latest_ms,
        receipts: lesson
            .receipts
            .iter()
            .map(|r| LessonReceiptDto {
                session_id: r.session.clone(),
                agent_id: r.agent.clone(),
                repo: r.repo.clone(),
                at_ms: r.at_ms,
                text: r.text.clone(),
                preceding: r.preceding.clone(),
                signal: match r.signal {
                    Signal::Correction => "correction",
                    Signal::Repeat => "repeat",
                    Signal::Rule => "rule",
                }
                .to_owned(),
            })
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lore_memory::mine::Candidate;

    #[test]
    fn a_lesson_maps_to_its_dto_with_a_suggested_rule() {
        let lesson = Lesson {
            id: "0123456789".into(),
            text: "no, use pnpm not npm".into(),
            conversations: 1,
            score: 3,
            latest_ms: Some(1_793_000_000_000),
            receipts: vec![Candidate {
                session: "s1".into(),
                agent: "codex".into(),
                repo: Some("lore".into()),
                repo_path: None,
                at_ms: Some(1_793_000_000_000),
                text: "no, use pnpm not npm".into(),
                preceding: Some("Ran npm install.".into()),
                signal: Signal::Correction,
            }],
            conflicts: Vec::new(),
        };
        let dto = lesson_dto(&lesson);
        assert_eq!(
            dto.suggested_rule,
            mine::suggest_rule("no, use pnpm not npm")
        );
        assert_eq!(dto.conversations, 1);
        assert_eq!(
            dto.receipts,
            [LessonReceiptDto {
                session_id: "s1".into(),
                agent_id: "codex".into(),
                repo: Some("lore".into()),
                at_ms: Some(1_793_000_000_000),
                text: "no, use pnpm not npm".into(),
                preceding: Some("Ran npm install.".into()),
                signal: "correction".into(),
            }]
        );
    }
}
