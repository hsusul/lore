//! Network-boundary guard (call-site check).
//!
//! `lore-core` is an archive module and MUST have no network capability: only a
//! separate, default-off updater (outside this crate) may reach the network.
//! This test statically scans the crate's own `src/` for networking APIs and
//! fails the build if any appear, backstopping the dependency choice with a
//! call-site check. It complements the OS-level egress test (M7).
#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::fs;
use std::path::Path;

/// Networking symbols that must never appear in archive-module source.
const FORBIDDEN: &[&str] = &[
    "std::net",
    "tokio::net",
    "async_std::net",
    "TcpStream",
    "TcpListener",
    "UdpSocket",
    "reqwest",
    "hyper::",
    "ureq",
    "isahc",
    "curl",
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
fn archive_source_has_no_network_apis() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut violations = Vec::new();
    scan_dir(&src, &mut violations);
    assert!(
        violations.is_empty(),
        "lore-core (archive module) must not reference networking APIs:\n{}",
        violations.join("\n")
    );
}
