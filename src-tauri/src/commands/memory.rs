//! Lore's memory, read for the desktop app: lessons mined from the archive,
//! the notes in the vault, and what an agent is given at session start.

use std::path::Path;

use lore_core::paths::ARCHIVE_DB_FILENAME;
use lore_ipc::{AgentsReadDto, LessonDto, NoteDto, ReadNoteDto, ReceiptDto};
use lore_memory::mine::{self, Candidate, Lesson, Limits};
use lore_memory::session_start::{self, CHAR_CAP, NOTE_CAP};
use lore_memory::vault::{self, Vault, VaultError};
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

/// Every active note in the vault, by path. No vault yet is not an error: it
/// is how Lore starts, so the list is empty.
#[tauri::command]
pub async fn list_notes() -> Result<Vec<NoteDto>, String> {
    let Some(vault) = open_vault() else {
        return Ok(Vec::new());
    };
    notes(&vault).map_err(|e| e.to_string())
}

/// What an agent starting a session in `cwd` is given: the section
/// `lorectl hook session-start` prints, and each note in it.
#[tauri::command]
pub async fn agents_read(cwd: String) -> Result<AgentsReadDto, String> {
    let cwd = Path::new(&cwd);
    if !cwd.is_absolute() {
        return Err("cwd must be an absolute path".to_owned());
    }
    match open_vault() {
        Some(vault) => agents_read_in(&vault, cwd).map_err(|e| e.to_string()),
        None => Ok(nothing_read(None)),
    }
}

fn open_vault() -> Option<Vault> {
    Vault::open(&vault::resolve_dir(None).ok()?).ok()
}

fn notes(vault: &Vault) -> Result<Vec<NoteDto>, VaultError> {
    let hits = vault.index()?.list_every_active()?;
    Ok(hits
        .into_iter()
        .map(|h| NoteDto {
            id: h.id,
            path: h.path,
            title: h.title,
            scope: h.scope,
            note_type: h.note_type,
            excerpt: h.excerpt,
        })
        .collect())
}

fn agents_read_in(vault: &Vault, cwd: &Path) -> Result<AgentsReadDto, VaultError> {
    let Some(found) = session_start::session_start(vault, cwd)? else {
        return Ok(nothing_read(Some(cwd)));
    };
    let notes = found
        .notes
        .iter()
        .enumerate()
        .map(|(i, r)| ReadNoteDto {
            id: r.note.id.as_str().to_owned(),
            title: session_start::title(&r.note.title),
            path: r.path.clone(),
            freshness: r.freshness.label().to_owned(),
            shown: i < found.shown,
        })
        .collect();
    Ok(AgentsReadDto {
        repo_key: found.context.key().map(|k| k.as_str().to_owned()),
        repo_name: found.context.name().map(|n| n.as_str().to_owned()),
        notes,
        total: i64::try_from(found.total).unwrap_or(i64::MAX),
        note_cap: NOTE_CAP as i64,
        char_cap: CHAR_CAP as i64,
        text: found.text,
    })
}

/// No note applies in `cwd`, or there is no vault: an agent is given nothing.
fn nothing_read(cwd: Option<&Path>) -> AgentsReadDto {
    let context = cwd.map(lore_memory::repo::RepoContext::discover);
    AgentsReadDto {
        repo_key: context
            .as_ref()
            .and_then(|c| c.key())
            .map(|k| k.as_str().to_owned()),
        repo_name: context
            .as_ref()
            .and_then(|c| c.name())
            .map(|n| n.as_str().to_owned()),
        notes: Vec::new(),
        total: 0,
        note_cap: NOTE_CAP as i64,
        char_cap: CHAR_CAP as i64,
        text: String::new(),
    }
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

    /// A vault with a global note and a note for another repository.
    fn vault(dir: &Path) -> Vault {
        let (vault, _) = Vault::init(&dir.join("vault")).unwrap();
        vault
            .add("---\ntype: convention\ntitle: Never push to main\n---\nOpen a PR.\n")
            .unwrap();
        vault
            .add("---\ntype: fact\ntitle: Other repo\nscope: repo:aaaaaaaaaaaaaaaa\n---\nNot here.\n")
            .unwrap();
        vault
    }

    #[test]
    fn every_active_note_is_listed_in_any_scope() {
        let dir = tempfile::tempdir().unwrap();
        let listed: Vec<(String, String, String)> = notes(&vault(dir.path()))
            .unwrap()
            .into_iter()
            .map(|n| (n.path, n.title, n.scope))
            .collect();
        assert_eq!(
            listed,
            [
                (
                    "global/never-push-to-main.md".to_owned(),
                    "Never push to main".to_owned(),
                    "global".to_owned()
                ),
                (
                    "repos/aaaaaaaaaaaaaaaa/other-repo.md".to_owned(),
                    "Other repo".to_owned(),
                    "repo:aaaaaaaaaaaaaaaa".to_owned()
                ),
            ]
        );
    }

    #[test]
    fn agents_read_is_the_section_the_hook_prints() {
        let dir = tempfile::tempdir().unwrap();
        let vault = vault(dir.path());
        let plain = dir.path().join("plain");
        std::fs::create_dir_all(&plain).unwrap();
        let read = agents_read_in(&vault, &plain).unwrap();
        let id = &read.notes[0].id;
        assert_eq!(
            read.text,
            format!("Lore memory for this repo:\n- [unanchored] Never push to main ({id})\n")
        );
        assert_eq!(
            (
                read.notes.len(),
                read.notes[0].freshness.as_str(),
                read.notes[0].shown,
                read.total,
                read.repo_key
            ),
            (1, "unanchored", true, 1, None)
        );
        assert_eq!((read.note_cap, read.char_cap), (10, 2_000));
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
