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
fn index_holds_nothing_the_vault_lacks() {
    let (dir, vault) = fixture_vault();
    assert_eq!(vault.index().unwrap().len().unwrap(), 21);
    std::fs::remove_file(dir.path().join("global/remove-a-stale-worktree.md")).unwrap();
    std::fs::write(dir.path().join("README.md"), "# not a note\n").unwrap();
    let report = vault.reindex().unwrap();
    assert_eq!(report.indexed, 20);
    assert_eq!(report.skipped.len(), 1);
    assert_eq!(report.skipped[0].0, "README.md");
    let index = vault.index().unwrap();
    assert_eq!(index.len().unwrap(), 20);
    assert!(index.lookup("remove-a-stale-worktree").unwrap().is_empty());
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
fn index_find_resolves_ids_slugs_and_paths() {
    let (_dir, vault) = fixture_vault();
    let rel = "repos/3f9a1c0b5e2d7a46/reference-id-panics-on-symbolic-refs.md";
    assert_eq!(vault.find("01JC9V6Q4M8K2T7X3N5R0B1H17").unwrap(), rel);
    assert_eq!(
        vault.find("reference-id-panics-on-symbolic-refs").unwrap(),
        rel
    );
    assert_eq!(vault.find(rel).unwrap(), rel);
    assert!(vault.find("no-such-note").is_err());
    assert!(vault.find("../outside.md").is_err());
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
fn index_slug_is_the_file_name_so_a_renamed_file_is_found_by_it() {
    let (dir, vault) = fixture_vault();
    let old = dir
        .path()
        .join("repos/3f9a1c0b5e2d7a46/reference-id-panics-on-symbolic-refs.md");
    std::fs::rename(
        &old,
        dir.path()
            .join("repos/3f9a1c0b5e2d7a46/peel-symbolic-refs.md"),
    )
    .unwrap();
    vault.reindex().unwrap();
    assert_eq!(
        vault.find("peel-symbolic-refs").unwrap(),
        "repos/3f9a1c0b5e2d7a46/peel-symbolic-refs.md"
    );
    assert_eq!(top3(&vault, "Reference::id")[0], "peel-symbolic-refs");
}

#[test]
fn index_search_treats_punctuation_as_text() {
    let (_dir, vault) = fixture_vault();
    for query in ["\"", "NEAR(", "a OR", "*", "title:x", "(", ""] {
        assert!(vault.search(query, 5).is_ok(), "{query}");
    }
    let hits = vault.search("Reference::id", 5).unwrap();
    assert_eq!(hits[0].slug, "reference-id-panics-on-symbolic-refs");
    assert_eq!(hits[0].scope, "repo:3f9a1c0b5e2d7a46");
    assert!(!hits[0].excerpt.contains('\n'));
}

#[test]
fn index_search_active_keeps_global_and_one_repo_and_drops_retired() {
    let (_dir, vault) = fixture_vault();
    let index = vault.index().unwrap();
    let all = index.search("and", 100).unwrap();
    let repo = "repo:3f9a1c0b5e2d7a46";
    assert!(all.iter().any(|h| h.status == "retired" && h.scope == repo));
    assert!(all.iter().any(|h| h.scope == "repo:a71e9d24c3b85f10"));
    for scope in [Some(repo), None] {
        let expected: Vec<&str> = all
            .iter()
            .filter(|h| h.status == "active")
            .filter(|h| h.scope == "global" || Some(h.scope.as_str()) == scope)
            .map(|h| h.path.as_str())
            .collect();
        let active = index.search_active("and", scope, 100).unwrap();
        let paths: Vec<&str> = active.iter().map(|h| h.path.as_str()).collect();
        assert_eq!(paths, expected, "{scope:?}");
    }
}

#[test]
fn split_timeline_only_splits_on_exact_heading() {
    use lore_memory::index::split_timeline;

    let body = "## Timeline of events\nDetails here.\n";
    let (statement, timeline) = split_timeline(body);
    assert_eq!(statement, body);
    assert_eq!(timeline, "");

    let body2 = "Some text.\n## Timeline is important.\nMore text.\n";
    let (statement2, timeline2) = split_timeline(body2);
    assert_eq!(statement2, body2);
    assert_eq!(timeline2, "");

    let body3 = "Some text.\n\n## Timeline\n- 2026-11-01 noted\n";
    let (statement3, timeline3) = split_timeline(body3);
    assert_eq!(statement3, "Some text.\n\n");
    assert_eq!(timeline3, "## Timeline\n- 2026-11-01 noted\n");

    let body4 = "## Timeline\n- 2026-11-01 noted\n";
    let (statement4, timeline4) = split_timeline(body4);
    assert_eq!(statement4, "");
    assert_eq!(timeline4, "## Timeline\n- 2026-11-01 noted\n");
}
