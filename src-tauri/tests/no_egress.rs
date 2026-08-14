//! Network-boundary guard for the app shell (call-site check).
//!
//! `lore-app` is the Tauri shell over `lore-core`; in V0 it performs no network
//! I/O of its own. The only network-capable component is the updater, which
//! lives behind the off-by-default `updater` feature and is not wired in here
//! (see `lib.rs`). This test statically scans the crate's own `src/` for
//! networking APIs and fails the build if any appear, mirroring the archive
//! module's guard (`lore-core`'s `no_network_in_archive`) so an accidental
//! outbound call in a command cannot slip in unnoticed.
#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::fs;
use std::path::Path;

/// Networking symbols that must not appear in the shell's own source.
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
    for entry in fs::read_dir(dir).expect("read src dir") {
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
fn shell_source_has_no_network_apis() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut violations = Vec::new();
    scan_dir(&src, &mut violations);
    assert!(
        violations.is_empty(),
        "lore-app (Tauri shell) must not reference networking APIs; the updater is \
         the only network-capable component and lives behind its own feature:\n{}",
        violations.join("\n")
    );
}
