//! Lore's memory, read for the desktop app: lessons mined from the archive,
//! the notes in the vault, and what an agent is given at session start.

use std::collections::HashMap;
use std::path::Path;

use lore_core::paths::ARCHIVE_DB_FILENAME;
use lore_ipc::{AgentsReadDto, LessonDto, NoteDto, ProposalDto, ReadNoteDto, ReceiptDto};
use lore_memory::mine::{self, Candidate, Lesson, Limits};
use lore_memory::note::Note;
use lore_memory::session_start::{self, CHAR_CAP, NOTE_CAP};
use lore_memory::vault::{self, Vault, VaultError};
use lore_memory::{filing, inbox, index, review};
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
pub async fn list_notes(state: State<'_, AppState>) -> Result<Vec<NoteDto>, String> {
    let Some(vault) = open_vault() else {
        return Ok(Vec::new());
    };
    let conn = lore_core::storage::open_read_only(&state.archive_dir.join(ARCHIVE_DB_FILENAME))
        .map_err(|e| e.to_string())?;
    notes(&vault, &conn).map_err(|e| e.to_string())
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

/// Keep the mined lesson `id` as a note: filed as a proposal and accepted, one
/// commit in the vault, as `lorectl init` keeps one. `title` is the rule as
/// the person edited it; without one, the suggested rule. Returns the note's
/// path in the vault.
#[tauri::command]
pub async fn accept_lesson(
    state: State<'_, AppState>,
    id: String,
    title: Option<String>,
) -> Result<String, String> {
    let vault = open_vault().ok_or("no vault yet; `lorectl mem init` creates one")?;
    let conn = lore_core::storage::open_read_only(&state.archive_dir.join(ARCHIVE_DB_FILENAME))
        .map_err(|e| e.to_string())?;
    accept_in(&conn, &vault, &id, title.as_deref())
}

/// Retire the note named by `key` (its id, slug, or path) with `reason`: one
/// commit, and agents stop being given it. Returns the note's path.
#[tauri::command]
pub async fn retire_note(key: String, reason: String) -> Result<String, String> {
    let vault = open_vault().ok_or("no vault yet; `lorectl mem init` creates one")?;
    review::retire(&vault, &key, &reason)
        .map(|(rel, _)| rel)
        .map_err(|e| e.to_string())
}

/// Rewrite the note named by `key` with `title` and `body`, as the person
/// edited them: one commit in the vault. The note's `## Timeline` stays as it
/// was, since it is Lore's record of where the note came from. Secrets are
/// masked, as in a proposal. Returns the note's path.
#[tauri::command]
pub async fn edit_note(key: String, title: String, body: String) -> Result<String, String> {
    let vault = open_vault().ok_or("no vault yet; `lorectl mem init` creates one")?;
    edit_in(&vault, &key, &title, &body)
}

fn edit_in(vault: &Vault, key: &str, title: &str, body: &str) -> Result<String, String> {
    let title = title.trim();
    inbox::check_line("title", title).map_err(|e| e.to_string())?;
    if body
        .lines()
        .any(|l| l.trim_end() == inbox::TIMELINE_HEADING)
    {
        return Err("the `## Timeline` section is Lore's to write".to_owned());
    }
    let rel = vault.find(key).map_err(|e| e.to_string())?;
    let mut note = Note::parse(&vault.read(&rel).map_err(|e| e.to_string())?)
        .map_err(|e| format!("{rel}: {e}"))?;
    let (_, timeline) = index::split_timeline(&note.body);
    let statement = inbox::redact(body.trim()).map_err(|e| e.to_string())?;
    note.body = match (statement.is_empty(), timeline.is_empty()) {
        (true, _) => timeline.to_owned(),
        (false, true) => format!("{statement}\n"),
        (false, false) => format!("{statement}\n\n{timeline}"),
    };
    note.title = inbox::redact(title).map_err(|e| e.to_string())?;
    vault.write(&rel, &note).map_err(|e| e.to_string())?;
    Ok(rel)
}

fn accept_in(
    conn: &Connection,
    vault: &Vault,
    id: &str,
    title: Option<&str>,
) -> Result<String, String> {
    let limits = Limits::default();
    let turns = mine::read_turns(conn).map_err(|e| e.to_string())?;
    let found = mine::lessons_within(mine::corrections_in(&turns, &limits), &limits);
    let lesson = found
        .iter()
        .find(|l| l.id == id)
        .ok_or_else(|| format!("no lesson `{id}` in the archive"))?;
    let (filed, _) = inbox::proposals(vault).map_err(|e| e.to_string())?;
    if filing::is_filed(&filed, lesson) {
        return Err(format!("lesson `{id}` was already filed"));
    }
    let title = title
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .map_or_else(|| mine::suggest_rule(&lesson.text), str::to_owned);
    filing::keep(vault, lesson, &title).map_err(|e| e.to_string())
}

/// The lessons agents wrote and filed for review, oldest first. No vault yet
/// is an empty list.
#[tauri::command]
pub async fn list_proposals(state: State<'_, AppState>) -> Result<Vec<ProposalDto>, String> {
    let Some(vault) = open_vault() else {
        return Ok(Vec::new());
    };
    let conn = lore_core::storage::open_read_only(&state.archive_dir.join(ARCHIVE_DB_FILENAME))
        .map_err(|e| e.to_string())?;
    proposals(&vault, &conn).map_err(|e| e.to_string())
}

/// Accept proposal `id`, with its title replaced by `title` when the person
/// edited it: one note file and one commit. Returns the note's path.
#[tauri::command]
pub async fn accept_proposal(id: String, title: Option<String>) -> Result<String, String> {
    let vault = open_vault().ok_or("no vault yet; `lorectl mem init` creates one")?;
    let edits = review::Edits {
        title: title.map(|t| t.trim().to_owned()).filter(|t| !t.is_empty()),
        body: None,
    };
    review::accept_edited(&vault, &id, None, &edits)
        .map(|accepted| accepted.rel)
        .map_err(|e| e.to_string())
}

/// Reject proposal `id` with `reason`. Only the decision log changes.
#[tauri::command]
pub async fn reject_proposal(id: String, reason: String) -> Result<(), String> {
    let vault = open_vault().ok_or("no vault yet; `lorectl mem init` creates one")?;
    review::reject(&vault, &id, &reason)
        .map(|_| ())
        .map_err(|e| e.to_string())
}

fn proposals(vault: &Vault, conn: &Connection) -> Result<Vec<ProposalDto>, review::ReviewError> {
    Ok(review::inbox(vault)?
        .proposals
        .into_iter()
        .map(|p| ProposalDto {
            repo_name: session_repo(conn, p.session.as_str()),
            id: p.id.as_str().to_owned(),
            title: p.title,
            body: p.body,
            scope: p.scope.to_string(),
            note_type: p.note_type.as_str().to_owned(),
            agent_id: p.agent.as_str().to_owned(),
            session_id: p.session.as_str().to_owned(),
            proposed_ms: i64::try_from(p.proposed_at.unix_timestamp_nanos() / 1_000_000)
                .unwrap_or(i64::MAX),
        })
        .collect())
}

fn open_vault() -> Option<Vault> {
    Vault::open(&vault::resolve_dir(None).ok()?).ok()
}

fn notes(vault: &Vault, conn: &Connection) -> Result<Vec<NoteDto>, VaultError> {
    let hits = vault.index()?.list_every_active()?;
    // A scope is one repository, so the first note that names it names all.
    let mut names: HashMap<String, String> = HashMap::new();
    Ok(hits
        .into_iter()
        .map(|h| {
            let repo_name = if h.scope.starts_with("repo:") {
                if !names.contains_key(&h.scope) {
                    if let Some(name) = learned_in_repo(vault, conn, &h.path) {
                        names.insert(h.scope.clone(), name);
                    }
                }
                names.get(&h.scope).cloned()
            } else {
                None
            };
            NoteDto {
                id: h.id,
                path: h.path,
                title: h.title,
                scope: h.scope,
                note_type: h.note_type,
                excerpt: h.excerpt,
                repo_name,
            }
        })
        .collect())
}

/// The archive's name for the repository a note's source sessions ran in.
fn learned_in_repo(vault: &Vault, conn: &Connection, rel: &str) -> Option<String> {
    let note = Note::parse(&vault.read(rel).ok()?).ok()?;
    note.sources
        .iter()
        .find_map(|source| session_repo(conn, source.session.as_str()))
}

/// The archive's name for the repository an archived session ran in.
fn session_repo(conn: &Connection, session: &str) -> Option<String> {
    conn.query_row(
        "SELECT r.display_name FROM session_segment seg
           JOIN repository r ON r.id = seg.repository_id
          WHERE seg.session_id = ?1 ORDER BY seg.seq_start LIMIT 1",
        [session],
        |r| r.get(0),
    )
    .ok()
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

pub(crate) fn receipt_dto(c: Candidate) -> ReceiptDto {
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
pub(crate) mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;
    use lore_core::adapters::{AdapterRegistry, DiscoveryRoots};
    use lore_core::discovery::{discover, DiscoveryConfig};
    use lore_core::ingest::ingest_discovered;
    use lore_core::storage::blob::BlobStore;

    /// Codex rollouts of `(role, text)` turns, one per session, ingested into an
    /// in-memory archive. Session `i` starts on 2026-08-`i+1`.
    pub(crate) fn archive(dir: &std::path::Path, sessions: &[&[(&str, &str)]]) -> Connection {
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
        let conn = lore_core::storage::open_in_memory().unwrap();
        let listed: Vec<(String, String, String)> = notes(&vault(dir.path()), &conn)
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
    fn a_repo_note_is_named_by_the_repository_its_session_ran_in() {
        let dir = tempfile::tempdir().unwrap();
        let conn = archive(dir.path(), &[&[("user", "hello")]]);
        let session = session_id(&conn, 1);
        conn.execute_batch(
            "INSERT INTO repository (id, identity_key, display_name, created_at, updated_at)
               VALUES ('r_1', 'k_1', 'beeclean', 0, 0);
             UPDATE session_segment SET repository_id = 'r_1';",
        )
        .unwrap();
        let (vault, _) = Vault::init(&dir.path().join("vault")).unwrap();
        let note = format!(
            "---\ntype: fact\ntitle: Ask first\nscope: repo:bbbbbbbbbbbbbbbb\nsources:\n  - codex:{session}\n---\nAsk.\n"
        );
        vault.add(&note).unwrap();
        vault
            .add("---\ntype: fact\ntitle: Same repo, no source\nscope: repo:bbbbbbbbbbbbbbbb\n---\nStill beeclean.\n")
            .unwrap();
        vault
            .add("---\ntype: fact\ntitle: Unlinked\nscope: repo:cccccccccccccccc\n---\nNo session.\n")
            .unwrap();
        vault
            .add("---\ntype: convention\ntitle: Everywhere\n---\nGlobal.\n")
            .unwrap();

        let named: Vec<(String, Option<String>)> = notes(&vault, &conn)
            .unwrap()
            .into_iter()
            .map(|n| (n.title, n.repo_name))
            .collect();
        assert_eq!(
            named,
            [
                ("Everywhere".to_owned(), None),
                ("Ask first".to_owned(), Some("beeclean".to_owned())),
                (
                    "Same repo, no source".to_owned(),
                    Some("beeclean".to_owned())
                ),
                ("Unlinked".to_owned(), None),
            ]
        );
    }

    fn agent_proposal(id: &str, session: &str, title: &str) -> lore_memory::inbox::Proposal {
        use lore_memory::note::{AgentKind, NoteId, NoteType, Scope, SessionId};
        lore_memory::inbox::Proposal {
            id: NoteId::parse(id).unwrap(),
            agent: AgentKind::Codex,
            session: SessionId::parse(session).unwrap(),
            source_verified: true,
            note_type: NoteType::Convention,
            scope: Scope::Global,
            title: title.to_owned(),
            body: "The agent pushed unformatted code.\n".to_owned(),
            anchors: Vec::new(),
            relation: None,
            proposed_at: time::OffsetDateTime::from_unix_timestamp(1_785_000_000).unwrap(),
        }
    }

    #[test]
    fn agent_proposals_are_listed_then_accepted_edited_or_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let conn = archive(dir.path(), &[&[("user", "hello")]]);
        let session = session_id(&conn, 1);
        conn.execute_batch(
            "INSERT INTO repository (id, identity_key, display_name, created_at, updated_at)
               VALUES ('r_1', 'k_1', 'lore', 0, 0);
             UPDATE session_segment SET repository_id = 'r_1';",
        )
        .unwrap();
        let (vault, _) = Vault::init(&dir.path().join("vault")).unwrap();
        let fmt = "01JD0000000000000000000011";
        let vague = "01JD0000000000000000000012";
        lore_memory::inbox::propose(
            &vault,
            agent_proposal(fmt, &session, "Run cargo fmt before you push"),
        )
        .unwrap();
        lore_memory::inbox::propose(
            &vault,
            agent_proposal(vague, &session, "Keep the log short"),
        )
        .unwrap();

        let listed: Vec<(String, String, Option<String>, i64)> = proposals(&vault, &conn)
            .unwrap()
            .into_iter()
            .map(|p| (p.id, p.title, p.repo_name, p.proposed_ms))
            .collect();
        assert_eq!(
            listed,
            [
                (
                    fmt.to_owned(),
                    "Run cargo fmt before you push".to_owned(),
                    Some("lore".to_owned()),
                    1_785_000_000_000
                ),
                (
                    vague.to_owned(),
                    "Keep the log short".to_owned(),
                    Some("lore".to_owned()),
                    1_785_000_000_000
                ),
            ]
        );

        let edits = review::Edits {
            title: Some("Run cargo fmt before every push".to_owned()),
            body: None,
        };
        let rel = review::accept_edited(&vault, fmt, None, &edits)
            .unwrap()
            .rel;
        assert_eq!(rel, "global/run-cargo-fmt-before-every-push.md");
        assert!(review::reject(&vault, vague, "not a rule").unwrap());
        assert!(proposals(&vault, &conn).unwrap().is_empty());
        let kept: Vec<String> = notes(&vault, &conn)
            .unwrap()
            .into_iter()
            .map(|n| n.title)
            .collect();
        assert_eq!(kept, ["Run cargo fmt before every push"]);
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

    fn id_of(vault: &Vault, rel: &str) -> String {
        Note::parse(&vault.read(rel).unwrap())
            .unwrap()
            .id
            .as_str()
            .to_owned()
    }

    #[test]
    fn accepting_a_lesson_keeps_one_note_and_only_once() {
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
                    ("user", "add dayjs"),
                    ("assistant", "npm install dayjs done."),
                    ("user", "I told you, pnpm not npm"),
                ],
            ],
        );
        let (vault, _) = Vault::init(&dir.path().join("vault")).unwrap();
        let id = lessons(&conn).unwrap()[0].id.clone();

        let rel = accept_in(&conn, &vault, &id, Some("  Use pnpm, not npm  ")).unwrap();
        assert_eq!(rel, "global/use-pnpm-not-npm.md");
        let kept: Vec<(String, String)> = notes(&vault, &conn)
            .unwrap()
            .into_iter()
            .map(|n| (n.path, n.title))
            .collect();
        assert_eq!(kept, [(rel.clone(), "Use pnpm, not npm".to_owned())]);
        assert_eq!(
            accept_in(&conn, &vault, &id, None),
            Err(format!("lesson `{id}` was already filed"))
        );
        assert_eq!(
            accept_in(&conn, &vault, "nope", None),
            Err("no lesson `nope` in the archive".to_owned())
        );

        let edited = edit_in(
            &vault,
            &id_of(&vault, &rel),
            " Use pnpm everywhere ",
            "The lockfile is pnpm-lock.yaml.\n",
        )
        .unwrap();
        assert_eq!(edited, rel, "an edit keeps the file where it is");
        let text = vault.read(&rel).unwrap();
        let note = Note::parse(&text).unwrap();
        assert_eq!(note.title, "Use pnpm everywhere");
        assert!(
            note.body
                .starts_with("The lockfile is pnpm-lock.yaml.\n\n## Timeline\n"),
            "{}",
            note.body
        );
        assert_eq!(
            note.body.matches("## Timeline").count(),
            1,
            "the timeline is kept, once"
        );
        assert_eq!(
            edit_in(&vault, &rel, "Use pnpm", "## Timeline\n- forged\n"),
            Err("the `## Timeline` section is Lore's to write".to_owned())
        );
        assert!(
            edit_in(&vault, &rel, "  ", "text").is_err(),
            "a note needs a title"
        );

        let (retired, _) = review::retire(&vault, &rel, "pnpm is gone now").unwrap();
        assert_eq!(retired, rel);
        assert!(
            notes(&vault, &conn).unwrap().is_empty(),
            "a retired note is no longer active"
        );
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
