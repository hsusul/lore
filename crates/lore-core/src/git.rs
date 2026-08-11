//! Git evidence: provenance-separated observations and repository identity.
//!
//! Scaffold for M4 (`docs/architecture/GIT_INTEGRATION.md`, ADR-0006). Live
//! repository capture (gix primary, hardened system-`git` fallback) is added in
//! later increments. This module currently owns the dependency-free pieces that
//! are needed regardless of the capture backend — notably remote-URL
//! normalization, which strips credentials so a recorded remote can be stored in
//! `git_observation.remote_url_norm` and used as identity evidence without ever
//! persisting a secret.

/// Normalize a git remote URL to a canonical, credential-free `host[:port]/path`
/// form suitable for identity matching and safe storage.
///
/// - Credentials (`user[:secret]@`) are removed — they are never stored.
/// - `scheme://`, scp-like (`git@host:path`), and bare `host/path` forms all
///   normalize to the same shape.
/// - The host is lowercased; a default git port (22/80/443/9418) is dropped; a
///   trailing `.git` and surrounding slashes are removed. The path is preserved
///   as-is (it may be case-sensitive on some forges).
///
/// Returns `None` for empty/whitespace input. Credentials never appear in the
/// output for any input.
#[must_use]
pub fn normalize_remote_url(raw: &str) -> Option<String> {
    let raw = raw.trim();
    if raw.is_empty() {
        return None;
    }

    let (authority, path) = if let Some(scheme_end) = raw.find("://") {
        // scheme://[userinfo@]host[:port]/path
        split_authority_path(&raw[scheme_end + 3..])
    } else if let Some(colon) = raw.find(':') {
        // scp-like: [userinfo@]host:path (no scheme, path after the first colon)
        (&raw[..colon], trim_slashes(&raw[colon + 1..]))
    } else {
        // bare host/path
        split_authority_path(raw)
    };

    let host = canonical_host(strip_userinfo(authority));
    if host.is_empty() {
        return None;
    }
    let path = clean_path(path);
    if path.is_empty() {
        Some(host)
    } else {
        Some(format!("{host}/{path}"))
    }
}

/// Split `host[/path]` at the first `/`.
fn split_authority_path(s: &str) -> (&str, &str) {
    match s.find('/') {
        Some(slash) => (&s[..slash], &s[slash + 1..]),
        None => (s, ""),
    }
}

/// Drop `userinfo@` (using the last `@` so a secret containing `@` cannot leak).
fn strip_userinfo(authority: &str) -> &str {
    match authority.rfind('@') {
        Some(at) => &authority[at + 1..],
        None => authority,
    }
}

/// Lowercase the host and drop a well-known default git port.
fn canonical_host(host_port: &str) -> String {
    let lowered = host_port.to_ascii_lowercase();
    if let Some((host, port)) = lowered.rsplit_once(':') {
        if matches!(port, "22" | "80" | "443" | "9418") {
            return host.to_string();
        }
    }
    lowered
}

fn trim_slashes(path: &str) -> &str {
    path.trim_matches('/')
}

/// Trim surrounding slashes and a trailing `.git`.
fn clean_path(path: &str) -> String {
    let path = trim_slashes(path);
    path.strip_suffix(".git").unwrap_or(path).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_credentials_and_canonicalizes() {
        // Credential-bearing HTTPS URL: secret removed, .git dropped.
        let out =
            normalize_remote_url("https://user:ghp_secrettoken@github.com/org/repo.git").unwrap();
        assert_eq!(out, "github.com/org/repo");
        assert!(!out.contains("ghp_secrettoken"));
        assert!(!out.contains('@'));
    }

    #[test]
    fn normalizes_scp_scheme_and_bare_forms_identically() {
        let canonical = "github.com/org/repo";
        assert_eq!(
            normalize_remote_url("git@github.com:org/repo.git").as_deref(),
            Some(canonical)
        );
        assert_eq!(
            normalize_remote_url("ssh://git@github.com:22/org/repo.git").as_deref(),
            Some(canonical)
        );
        assert_eq!(
            normalize_remote_url("https://github.com/org/repo").as_deref(),
            Some(canonical)
        );
        // Bare form (as recorded in the Codex fixture).
        assert_eq!(
            normalize_remote_url("github.com/org/repo").as_deref(),
            Some(canonical)
        );
    }

    #[test]
    fn lowercases_host_but_preserves_path_case() {
        assert_eq!(
            normalize_remote_url("HTTPS://GitHub.com/Org/Repo.git").as_deref(),
            Some("github.com/Org/Repo")
        );
    }

    #[test]
    fn keeps_non_default_port() {
        assert_eq!(
            normalize_remote_url("ssh://git@host.example.com:2222/team/repo.git").as_deref(),
            Some("host.example.com:2222/team/repo")
        );
    }

    #[test]
    fn empty_input_is_none() {
        assert!(normalize_remote_url("").is_none());
        assert!(normalize_remote_url("   ").is_none());
    }

    #[test]
    fn multiple_at_signs_do_not_leak_userinfo() {
        // An `@` inside the userinfo must not be mistaken for the host boundary.
        let out = normalize_remote_url("https://user:p@ss@gitlab.example.com/g/p.git").unwrap();
        assert_eq!(out, "gitlab.example.com/g/p");
    }
}
