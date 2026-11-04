//! The block an agent sees at session start, rendered from a real vault.
#![allow(clippy::expect_used, clippy::unwrap_used)]

use lore_memory::inbox::{self, Proposal};
use lore_memory::note::{AgentKind, NoteId, NoteType, Scope, SessionId};
use lore_memory::review;
use lore_memory::session_start;
use lore_memory::vault::Vault;
use time::OffsetDateTime;

#[test]
fn an_accepted_global_note_is_listed_with_its_freshness() {
    std::env::set_var("GIT_CONFIG_GLOBAL", "/dev/null");
    std::env::set_var("GIT_CONFIG_NOSYSTEM", "1");
    let dir = tempfile::tempdir().unwrap();
    let (vault, _) = Vault::init(dir.path()).unwrap();
    let elsewhere = tempfile::tempdir().unwrap();
    assert_eq!(session_start::notes(&vault, elsewhere.path()), None);

    inbox::propose(
        &vault,
        Proposal {
            id: NoteId::parse("01JC9V6Q4M8K2T7X3N5R0B1H6D").unwrap(),
            agent: AgentKind::ClaudeCode,
            session: SessionId::parse("s1").unwrap(),
            source_verified: true,
            note_type: NoteType::Convention,
            scope: Scope::Global,
            title: "Use pnpm, not npm".into(),
            body: "This machine has no npm cache.\n".into(),
            anchors: Vec::new(),
            relation: None,
            proposed_at: OffsetDateTime::from_unix_timestamp(1_793_534_400).unwrap(),
        },
    )
    .unwrap();
    let accepted = review::accept(&vault, "01JC9V6Q4M8K2T7X3N5R0B1H6D", None).unwrap();

    let block = session_start::notes(&vault, elsewhere.path()).unwrap();
    let lines: Vec<&str> = block.lines().collect();
    assert_eq!(lines[0], "Lore memory for this repo:");
    assert_eq!(lines.len(), 2, "{block}");
    assert!(
        lines[1].ends_with(&format!("] Use pnpm, not npm ({})", accepted.id.as_str())),
        "{block}"
    );
}
