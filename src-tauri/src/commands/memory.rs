//! Lore's memory, read for the desktop app: lessons mined from the archive.

use lore_core::paths::ARCHIVE_DB_FILENAME;
use lore_ipc::{LessonDto, ReceiptDto};
use lore_memory::mine::{self, Candidate, Lesson, Limits};
use rusqlite::Connection;
use tauri::State;

use crate::state::AppState;

/// Lessons mined from the user's corrections, most conversations first, without
/// the ones scored as one-off orders (as `lorectl mem mine` shows them).
///
/// `async`, on its own read-only connection: mining reads every turn in the
/// archive, about a second on a large one, and must neither block the window
/// nor hold the UI's database lock while it does.
#[tauri::command]
pub async fn list_lessons(state: State<'_, AppState>) -> Result<Vec<LessonDto>, String> {
    let conn = lore_core::storage::open_read_only(&state.archive_dir.join(ARCHIVE_DB_FILENAME))
        .map_err(|e| e.to_string())?;
    lessons(&conn).map_err(|e| e.to_string())
}

fn lessons(conn: &Connection) -> rusqlite::Result<Vec<LessonDto>> {
    let limits = Limits::default();
    let turns = mine::read_turns(conn)?;
    let mut found = mine::lessons_within(mine::corrections_in(&turns, &limits), &limits);
    found.retain(|l| l.score >= 0);
    Ok(found.into_iter().map(lesson_dto).collect())
}

fn lesson_dto(lesson: Lesson) -> LessonDto {
    LessonDto {
        suggested_rule: mine::suggest_rule(&lesson.text),
        id: lesson.id,
        text: lesson.text,
        conversations: i64::try_from(lesson.conversations).unwrap_or(i64::MAX),
        latest_ms: lesson.latest_ms,
        receipts: lesson.receipts.into_iter().map(receipt_dto).collect(),
    }
}

fn receipt_dto(c: Candidate) -> ReceiptDto {
    ReceiptDto {
        session_id: c.session,
        agent_id: c.agent,
        repo: c.repo,
        at_ms: c.at_ms,
        text: c.text,
        preceding: c.preceding,
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;
    use lore_core::adapters::{AdapterRegistry, DiscoveryRoots};
    use lore_core::discovery::{discover, DiscoveryConfig};
    use lore_core::ingest::ingest_discovered;
    use lore_core::storage::blob::BlobStore;

    /// Codex rollouts of `(role, text)` turns, one per session, ingested into an
    /// in-memory archive. Session `i` starts on 2026-08-`i+1`.
    fn archive(dir: &std::path::Path, sessions: &[&[(&str, &str)]]) -> Connection {
        let root = dir.join("codex/sessions");
        for (i, turns) in sessions.iter().enumerate() {
            let day = root.join(format!("2026/08/{:02}", i + 1));
            std::fs::create_dir_all(&day).unwrap();
            let ts = |s: usize| format!("2026-08-{:02}T10:00:{:02}.000Z", i + 1, s);
            let mut lines = vec![
                serde_json::json!({"type": "session_meta", "timestamp": ts(0),
                "payload": {"id": format!("019e0000-0000-7000-8000-{:012x}", i + 1),
                    "cwd": "/proj", "cli_version": "0.133.0", "model_provider": "openai"}}),
            ];
            for (n, (role, text)) in turns.iter().enumerate() {
                let kind = if *role == "user" {
                    "input_text"
                } else {
                    "output_text"
                };
                lines.push(serde_json::json!({"type": "response_item", "timestamp": ts(n + 1),
                    "payload": {"type": "message", "role": role, "content": [{"type": kind, "text": text}]}}));
            }
            let body: String = lines.iter().map(|l| format!("{l}\n")).collect();
            std::fs::write(day.join(format!("rollout-{:012x}.jsonl", i + 1)), body).unwrap();
        }
        let mut config = DiscoveryConfig::new();
        config.set_roots("codex", DiscoveryRoots::new(vec![root]));
        config.set_roots("claude-code", DiscoveryRoots::new(vec![dir.join("none")]));
        let registry = AdapterRegistry::v0();
        let conn = lore_core::storage::open_in_memory().unwrap();
        let blobs = BlobStore::open(dir.join("blobs")).unwrap();
        let found = discover(&registry, &config);
        assert!(ingest_discovered(&conn, &registry, &found.sessions, &blobs)
            .failures
            .is_empty());
        conn
    }

    fn session_id(conn: &Connection, n: usize) -> String {
        conn.query_row(
            "SELECT id FROM agent_session WHERE native_session_id = ?1",
            [format!("019e0000-0000-7000-8000-{n:012x}")],
            |r| r.get(0),
        )
        .unwrap()
    }

    #[test]
    fn a_lesson_carries_every_session_it_was_said_in() {
        let dir = tempfile::tempdir().unwrap();
        let conn = archive(
            dir.path(),
            &[
                &[
                    ("user", "add axios"),
                    ("assistant", "Running npm install axios."),
                    ("user", "no, use pnpm not npm"),
                ],
                &[
                    ("user", "add lodash"),
                    ("assistant", "Running npm install lodash."),
                    ("user", "wrong tool, use pnpm instead of npm"),
                ],
                &[
                    ("user", "add dayjs"),
                    ("assistant", "npm install dayjs done."),
                    ("user", "I told you, pnpm not npm"),
                ],
            ],
        );
        let found = lessons(&conn).unwrap();
        assert_eq!(found.len(), 1, "{found:?}");
        let l = &found[0];
        assert_eq!(
            (
                l.text.as_str(),
                l.suggested_rule.as_str(),
                l.conversations,
                l.latest_ms
            ),
            (
                "I told you, pnpm not npm",
                "Pnpm not npm",
                3,
                Some(1_785_751_203_000)
            )
        );
        let receipts: Vec<(String, &str, Option<&str>, &str)> = l
            .receipts
            .iter()
            .map(|r| {
                (
                    r.session_id.clone(),
                    r.agent_id.as_str(),
                    r.preceding.as_deref(),
                    r.text.as_str(),
                )
            })
            .collect();
        assert_eq!(
            receipts,
            [
                (
                    session_id(&conn, 3),
                    "codex",
                    Some("npm install dayjs done."),
                    "I told you, pnpm not npm"
                ),
                (
                    session_id(&conn, 2),
                    "codex",
                    Some("Running npm install lodash."),
                    "wrong tool, use pnpm instead of npm"
                ),
                (
                    session_id(&conn, 1),
                    "codex",
                    Some("Running npm install axios."),
                    "no, use pnpm not npm"
                ),
            ]
        );
    }
}
