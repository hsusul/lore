//! The high-entropy rule flags random tokens, not paths, URL paths, or code
//! identifiers. Every known-format detector is unchanged.
#![allow(clippy::expect_used, clippy::unwrap_used)]

use lore_core::secrets::scan;

/// The rules that fire on `text`, in span order.
fn rules(text: &str) -> Vec<&'static str> {
    scan(text).unwrap().iter().map(|f| f.rule).collect()
}

/// Matches the high-entropy rule produced on a real history, none a secret.
const NOT_SECRETS: &[&str] = &[
    "/crates/lore-ipc/bindings/BackupScheduleDto",
    "Pro/OpenSource/aws-sdk-pandas-hunt-s3io/tests/unit/test_moto",
    "claude/skills/clean-git-commit/SKILL",
    "testForcedBurstWithinAMinuteAddsOneSnapshotAndKeepsEstimate",
    "worktrees/correctness-nov02/build/release/src/optimizer/",
    "/private/tmp/supabase-py-fresh2-nov02",
    "___CFXRegistrationPost_block_invoke",
    "https://github.com/hsusul/lore/pull/155/files",
    "/Users/me/Library/Application Support/Code/User/settings.json",
];

#[test]
fn paths_urls_and_identifiers_are_not_high_entropy_secrets() {
    for text in NOT_SECRETS {
        assert_eq!(rules(text), Vec::<&str>::new(), "{text}");
        let line = format!("error: cannot open {text} (os error 2)");
        assert_eq!(rules(&line), Vec::<&str>::new(), "{line}");
    }
}

#[test]
fn random_tokens_are_still_high_entropy_secrets() {
    for token in [
        "Zq7Lm2Xc9Vb4Nk8Rt1Yw6Pd3",
        "kQ9xTzP2mB7vR4sW8nL1cY6dH3fJ5gK0",
        "u8Jd_k2Lp-Qz9Xw4Nv7Bc1Mt6Rs3Yh5Ga0Fe",
        "aZ3kQ9xT/mB7vR4sW+nL1cY6dH3fJ5gK0pQ2",
        "x7k2m9q4w8r1t6y3u5i0o2p4a6s8d1f3g5h7",
    ] {
        assert_eq!(rules(token), ["high-entropy"], "{token}");
        let line = format!("export SESSION={token}");
        assert_eq!(scan(&line).unwrap().len(), 1, "{line}");
    }
}

#[test]
fn known_formats_are_flagged_inside_paths_and_identifiers() {
    let github = concat!("ghp_", "Zq7Lm2Xc9Vb4Nk8Rt1Yw6Pd3Fs5Gh0Jk2Lm4Q");
    let aws = concat!("AKIA", "Z7Q2LM4XC9VB4NK8");
    let jwt = concat!(
        "eyJhbGciOiJIUzI1NiJ9.",
        "eyJzdWIiOiIxMjM0NTY3ODkwIn0.",
        "dBjftJeZ4CVPmB92K27uhbUJU1p1r_wW1gFWFOEjXk"
    );
    assert_eq!(rules(&format!("/tmp/{github}/run")), ["github-token"]);
    assert_eq!(rules(&format!("creds/{aws}")), ["aws-access-key-id"]);
    assert_eq!(rules(&format!("Bearer {jwt}")), ["jwt"]);
    assert_eq!(
        rules("postgres://admin:Sup3rS3cret@db.internal:5432/app"),
        ["connection-string"]
    );
    assert_eq!(
        rules("password = 'CorrectHorseBatteryStaple'"),
        ["generic-assignment"]
    );
}

/// `n` tokens of 20 to 64 characters drawn from `alphabet`, deterministic.
fn random_tokens(alphabet: &[u8], n: usize) -> Vec<String> {
    let mut state: u64 = 0x9E37_79B9_7F4A_7C15;
    let mut next = move || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    };
    (0..n)
        .map(|_| {
            let len = 20 + (next() % 45) as usize;
            (0..len)
                .map(|_| alphabet[(next() % alphabet.len() as u64) as usize] as char)
                .collect()
        })
        .collect()
}

#[test]
fn almost_every_random_token_is_still_flagged() {
    let base64url = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let tokens = random_tokens(base64url, 10_000);
    let flagged = tokens
        .iter()
        .filter(|t| rules(t) == ["high-entropy"])
        .count();
    // Before the identifier check, 9,818 were flagged (the rest are under
    // the entropy floor). Six random tokens now read as identifiers.
    assert_eq!(flagged, 9_812);
}

/// Generated ids seen as high-entropy findings on a real history, none a
/// secret.
const STRUCTURED_IDS: &[&str] = &[
    "mcp__fc6983da-0816-4afe-a9db-48419f02fe10__list_teams",
    "toolu_016Txo4Jsas9kZh82FM24mWY",
    "call_x8ZQb7Yk2mN4pR9sT1vW3aBc",
    "local_fc6983da-0816-4afe-a9db-48419f02fe10",
    "scan_ingest-d6de84c9490de3fb",
    "target/debug/deps/scan_ingest-d6de84c9490de3fb",
    "_RNvNtCsjIpJoQiBNqZ_9lore_core6ingest11insert_rows",
    "_ZN9lore_core6ingest11insert_rows17h3f2a9c4d5e6b7a81E",
];

#[test]
fn tool_ids_mangled_symbols_and_build_hashes_are_not_secrets() {
    for text in STRUCTURED_IDS {
        assert_eq!(rules(text), Vec::<&str>::new(), "{text}");
        let line = format!("tool_use_id: {text} returned");
        assert_eq!(rules(&line), Vec::<&str>::new(), "{line}");
    }
}

#[test]
fn look_alikes_of_structured_ids_stay_flagged() {
    for text in [
        // A uuid inside a path, as most of the uuids left on a real history.
        "sessions/fc6983da-0816-4afe-a9db-48419f02fe10",
        // Not a uuid after local_, nor a name before the hash.
        "local_Zq7Lm2Xc9Vb4Nk8Rt1Yw6Pd3",
        "Zq7Lm2Xc9Vb4Nk8Rt1-d6de84c9490de3fb",
        // A random token that only starts like a tool id.
        "toolu_016Txo4Jsas9kZh82FM24mWY/Zq7Lm2Xc9Vb4Nk8",
    ] {
        assert_eq!(rules(text), ["high-entropy"], "{text}");
    }
}
