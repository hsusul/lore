//! Network-boundary guard (call-site check) for the skills doctor.
//!
//! `lore-skills` ships inside `lorectl`, so it inherits the same guarantee: no
//! network capability. Sibling of `lorectl/tests/no_network_in_lorectl.rs` and
//! `lore-core/tests/no_network_in_archive.rs`, duplicated for the reason given
//! there: a guard each crate owns cannot be weakened for it by a change made for
//! another. The dependency-graph and OS-level halves live in
//! `scripts/egress-check.sh`, which covers this package too.
#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::fs;
use std::path::Path;

/// Kept identical to the archive's list on purpose: one promise, one wording.
const FORBIDDEN: &[&str] = &[
    "std::net",
    "tokio::net",
    "async_std::net",
    "TcpStream",
    "TcpListener",
    "UdpSocket",
    "reqwest::",
    "reqwest ",
    "hyper::",
    "hyper ",
    "ureq::",
    "isahc::",
    "curl::",
    "tungstenite::",
    "attohttpc::",
    "surf::",
    "wreq::",
    "native_tls",
    "rustls",
];

fn scan_dir(dir: &Path, violations: &mut Vec<String>) {
    let entries = fs::read_dir(dir).expect("read src dir");
    for entry in entries {
        let path = entry.expect("dir entry").path();
        if path.is_dir() {
            scan_dir(&path, violations);
        } else if path.extension().is_some_and(|e| e == "rs") {
            let text = fs::read_to_string(&path).expect("read rs file");
            for (lineno, line) in text.lines().enumerate() {
                for needle in FORBIDDEN {
                    if line.contains(needle) {
                        violations.push(format!("{}:{} -> {needle}", path.display(), lineno + 1));
                    }
                }
            }
        }
    }
}

#[test]
fn skills_source_has_no_network_apis() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut violations = Vec::new();
    scan_dir(&src, &mut violations);
    assert!(
        violations.is_empty(),
        "lore-skills (shipped in lorectl) must not reference networking APIs:\n{}",
        violations.join("\n")
    );
}

#[test]
fn the_scanner_actually_looks_at_this_crate() {
    // A guard that silently scanned an empty directory would pass forever.
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let seen = fs::read_dir(&src)
        .expect("read src dir")
        .filter(|e| {
            e.as_ref()
                .is_ok_and(|e| e.path().extension().is_some_and(|x| x == "rs"))
        })
        .count();
    assert!(seen > 0, "no Rust source found under {}", src.display());
}
