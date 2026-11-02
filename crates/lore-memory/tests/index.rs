//! The index against the fixture vault: exact rankings, rebuilt from the vault
//! alone, and never holding anything the vault lacks.
#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::path::Path;

use lore_memory::index::{Index, INDEX_FILE};
use lore_memory::vault::Vault;

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let path = entry.unwrap().path();
        let target = to.join(path.file_name().unwrap());
        if path.is_dir() {
            copy_dir(&path, &target);
        } else {
            std::fs::copy(&path, &target).unwrap();
        }
    }
}

fn fixture_vault() -> (tempfile::TempDir, Vault) {
    std::env::set_var("GIT_CONFIG_GLOBAL", "/dev/null");
    std::env::set_var("GIT_CONFIG_NOSYSTEM", "1");
    let dir = tempfile::tempdir().unwrap();
    let (vault, _) = Vault::init(dir.path()).unwrap();
    copy_dir(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/vault"),
        dir.path(),
    );
    (dir, vault)
}

fn top3(vault: &Vault, query: &str) -> Vec<String> {
    vault
        .search(query, 3)
        .unwrap()
        .into_iter()
        .map(|hit| hit.slug)
        .collect()
}

const EXPECTED: [(&str, [&str; 3]); 5] = [
    (
        "worktree",
        [
            "remove-a-stale-worktree",
            "git-worktrees-share-one-object-store",
            "git-worktree-add-a-branch-can-be-checked-out-once",
        ],
    ),
    (
        "sqlite",
        [
            "sqlite-wal-files-outlive-the-connection",
            "rebuild-the-memory-index",
            "fts5-bm25-scores-lower-is-better",
        ],
    ),
    (
        "lock",
        [
            "recover-from-a-held-git-index-lock",
            "sqlite-wal-files-outlive-the-connection",
            "the-index-lives-outside-lore-db",
        ],
    ),
    (
        "commit",
        [
            "git-commit-without-an-identity-fails-in-ci",
            "conventional-commits-with-a-scope",
            "agents-propose-people-accept",
        ],
    ),
    (
        "index",
        [
            "rebuild-the-memory-index",
            "recover-from-a-held-git-index-lock",
            "the-index-lives-outside-lore-db",
        ],
    ),
];

fn rankings(vault: &Vault) -> Vec<(String, Vec<String>)> {
    EXPECTED
        .iter()
        .map(|(query, _)| (query.to_string(), top3(vault, query)))
        .collect()
}

fn expected() -> Vec<(String, Vec<String>)> {
    EXPECTED
        .iter()
        .map(|(q, slugs)| (q.to_string(), slugs.iter().map(|s| s.to_string()).collect()))
        .collect()
}

#[test]
fn index_ranks_the_fixture_vault_exactly() {
    let (_dir, vault) = fixture_vault();
    assert_eq!(rankings(&vault), expected());
}

#[test]
fn index_rebuilt_after_deletion_ranks_the_same() {
    let (dir, vault) = fixture_vault();
    let report = vault.reindex().unwrap();
    assert_eq!((report.indexed, report.skipped.len()), (21, 0));
    std::fs::remove_file(dir.path().join(".lore").join(INDEX_FILE)).unwrap();
    assert!(Index::open(&vault.lore_dir()).unwrap().is_none());
    let report = vault.reindex().unwrap();
    assert_eq!(report.indexed, 21);
    assert_eq!(rankings(&vault), expected());
}

#[test]
fn index_finds_a_hand_edit_after_reindex() {
    let (dir, vault) = fixture_vault();
    let path = dir.path().join("global/agents-propose-people-accept.md");
    assert!(top3(&vault, "kingfisher").is_empty());
    let text = std::fs::read_to_string(&path).unwrap();
    std::fs::write(
        &path,
        text.replace("A person accepts", "A kingfisher accepts"),
    )
    .unwrap();
    vault.reindex().unwrap();
    assert_eq!(top3(&vault, "kingfisher"), ["agents-propose-people-accept"]);
}

#[test]
fn index_sees_an_added_note_without_a_reindex() {
    let (_dir, vault) = fixture_vault();
    vault.reindex().unwrap();
    vault
        .add("---\ntype: fact\ntitle: Kingfishers dive\n---\nThey dive for fish.\n")
        .unwrap();
    assert_eq!(top3(&vault, "kingfishers"), ["kingfishers-dive"]);
}

#[test]
fn index_excerpts_leave_out_the_timeline_but_it_stays_searchable() {
    let (_dir, vault) = fixture_vault();
    vault
        .add("---\ntype: fact\ntitle: Herons wait\n---\nHerons wait for fish.\n\n## Timeline\n- 2026-11-01 heron noted by codex\n")
        .unwrap();
    for query in ["herons", "heron"] {
        let hits = vault.search(query, 3).unwrap();
        assert_eq!(hits[0].slug, "herons-wait");
        assert!(!hits[0].excerpt.contains("Timeline"), "{}", hits[0].excerpt);
        assert!(!hits[0].excerpt.contains("2026"), "{}", hits[0].excerpt);
    }
    assert_eq!(top3(&vault, "codex noted"), ["herons-wait"]);
}

#[test]
fn index_search_treats_punctuation_as_text() {
    let (_dir, vault) = fixture_vault();
    for query in ["\"", "NEAR(", "a OR", "*", "title:x", "(", ""] {
        assert!(vault.search(query, 5).is_ok(), "{query}");
    }
    let hits = vault.search("Reference::id", 5).unwrap();
    assert_eq!(hits[0].slug, "reference-id-panics-on-symbolic-refs");
    assert_eq!(hits[0].scope, "repo:3f9a1c0b");
    assert!(!hits[0].excerpt.contains('\n'));
}
