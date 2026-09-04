//! Network-boundary guard (call-site check) for the CLI surface.
//!
//! `lorectl` is a second surface over the archive and inherits the archive's
//! guarantee: it MUST have no network capability. This is the sibling of
//! `lore-core/tests/no_network_in_archive.rs`, which scans only its own crate's
//! `src/`. The scanner is duplicated rather than shared because a workspace
//! crate existing only to hold forty lines of `read_dir` would cost more to
//! carry than the duplication does — and because a guard that each crate owns
//! outright cannot be weakened for one crate by a change made for another.
//!
//! The dependency-graph and OS-level halves of the same promise live in
//! `scripts/egress-check.sh`, which covers this package too.
#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::fs;
use std::path::Path;

/// Networking symbols that must never appear in this crate's source.
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
fn cli_source_has_no_network_apis() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut violations = Vec::new();
    scan_dir(&src, &mut violations);
    assert!(
        violations.is_empty(),
        "lorectl (a surface over the archive) must not reference networking APIs:\n{}",
        violations.join("\n")
    );
}

#[test]
fn the_scanner_actually_looks_at_this_crate() {
    // A guard that silently scanned an empty directory would pass forever.
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut seen = 0;
    for entry in fs::read_dir(&src).expect("read src dir") {
        if entry
            .expect("dir entry")
            .path()
            .extension()
            .is_some_and(|e| e == "rs")
        {
            seen += 1;
        }
    }
    assert!(seen > 0, "no Rust source found under {}", src.display());
}
