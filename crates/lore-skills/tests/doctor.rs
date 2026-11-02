//! The inventory of the fixture home, which holds at least one skill of every
//! kind. Read-only: the fixture is never written.
#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::path::{Path, PathBuf};

use lore_skills::Roots;

fn home() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/home")
}

#[test]
fn a_link_to_one_folder_is_one_copy_not_a_duplicate() {
    let inventory = lore_skills::inventory(&Roots::from_home(&home()));
    let linked: Vec<_> = inventory
        .records
        .iter()
        .filter(|r| r.name == "linked")
        .collect();
    assert_eq!(linked.len(), 1);
    assert!(matches!(
        linked[0].installs[0].mode,
        lore_skills::Mode::Symlink { .. }
    ));
}

#[test]
fn skills_no_agent_loads_are_known_but_not_reported() {
    let inventory = lore_skills::inventory(&Roots::from_home(&home()));
    assert!(inventory.elsewhere.contains_key("off-skill"));
    assert!(inventory.elsewhere.contains_key("linked"));
    assert!(inventory.records.iter().all(|r| r.name != "off-skill"));
}
