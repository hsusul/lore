//! The doctor report a person reads: each finding described with paths under
//! `~`, paired with its fix. Read-only over the fixture home.
#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::path::Path;

use lore_skills::{report, Roots};

#[test]
fn each_finding_is_described_under_home_with_its_fix() {
    let home = Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/home");
    let inventory = lore_skills::inventory(&Roots::from_home(&home));
    let findings = lore_skills::doctor(&inventory);
    let entries = report::report(&inventory, &findings);

    assert_eq!(entries.len(), findings.len());
    let mismatch = entries
        .iter()
        .find(|e| e.finding.kind() == "name-mismatch")
        .unwrap();
    assert_eq!(
        mismatch.description,
        "~/.agents/skills/mislabeled declares `name: other-name`"
    );
    assert!(
        entries
            .iter()
            .all(|e| !e.description.contains(&*home.to_string_lossy())),
        "a path was shown outside ~"
    );
    assert!(
        report::summary(&findings, report::distinct_names(&inventory))
            .contains(&format!("; {} findings;", findings.len()))
    );
}
