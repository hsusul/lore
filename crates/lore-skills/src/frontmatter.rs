//! SKILL.md frontmatter and the skill names its body mentions.
//!
//! Hand-rolled rather than a YAML crate: it only needs `name` and
//! `description`, plus enough structure to say when an agent's YAML reader
//! would reject the block, and it keeps the audited dependency tree unchanged.

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frontmatter {
    pub name: String,
    pub description: String,
}

/// Split SKILL.md into its parsed frontmatter and its body.
pub fn parse(text: &str) -> (Result<Frontmatter, String>, &str) {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let mut lines = text.split_inclusive('\n');
    let first = lines.next().unwrap_or_default();
    if first.trim_end() != "---" {
        return (
            Err("does not start with a `---` frontmatter fence".into()),
            text,
        );
    }
    let mut offset = first.len();
    let mut header = Vec::new();
    let mut closed = false;
    for line in lines {
        offset += line.len();
        let line = line.trim_end_matches(['\n', '\r']);
        if line.trim_end() == "---" {
            closed = true;
            break;
        }
        header.push(line);
    }
    if !closed {
        return (Err("has no closing `---`".into()), "");
    }
    (fields(&header), text.get(offset..).unwrap_or_default())
}

fn fields(header: &[&str]) -> Result<Frontmatter, String> {
    let mut name = None;
    let mut description = None;
    let mut seen: Vec<&str> = Vec::new();
    let mut i = 0;
    while let Some(line) = header.get(i) {
        // Line numbers count the opening fence as line 1.
        let lineno = i + 2;
        i += 1;
        if line.trim().is_empty() || line.trim_start().starts_with('#') {
            continue;
        }
        if line.starts_with([' ', '\t']) {
            return Err(format!("line {lineno} is indented with no key above it"));
        }
        let Some((key, rest)) = line.split_once(':') else {
            return Err(format!("line {lineno} is not `key: value`"));
        };
        if key.is_empty() || key.contains(char::is_whitespace) {
            return Err(format!("line {lineno} has no valid key"));
        }
        if !rest.is_empty() && !rest.starts_with([' ', '\t']) {
            return Err(format!("line {lineno} needs a space after `{key}:`"));
        }
        if seen.contains(&key) {
            return Err(format!("`{key}` appears twice"));
        }
        seen.push(key);
        let start = i;
        while header
            .get(i)
            .is_some_and(|l| l.starts_with([' ', '\t']) || l.trim().is_empty())
        {
            i += 1;
        }
        let continuation = header.get(start..i).unwrap_or_default();
        match key {
            "name" | "description" => {
                let value =
                    scalar(rest.trim(), continuation).map_err(|e| format!("`{key}` {e}"))?;
                if key == "name" {
                    name = Some(value);
                } else {
                    description = Some(value);
                }
            }
            _ => {}
        }
    }
    match (name, description) {
        (Some(name), Some(description)) => Ok(Frontmatter { name, description }),
        (None, _) => Err("has no `name`".into()),
        (Some(_), None) => Err("has no `description`".into()),
    }
}

/// A string scalar: plain, single- or double-quoted, or a `|` / `>` block.
fn scalar(inline: &str, continuation: &[&str]) -> Result<String, String> {
    let more = continuation
        .iter()
        .map(|l| l.trim())
        .filter(|l| !l.is_empty());
    if inline.starts_with(['|', '>']) {
        let sep = if inline.starts_with('|') { "\n" } else { " " };
        return Ok(more.collect::<Vec<_>>().join(sep));
    }
    let joined = std::iter::once(inline)
        .chain(more)
        .filter(|l| !l.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    if let Some(body) = joined.strip_prefix('"') {
        return quoted(body, '"');
    }
    if let Some(body) = joined.strip_prefix('\'') {
        return quoted(body, '\'');
    }
    if let Some(c) = joined.chars().next() {
        if "[{*&!%@`".contains(c) || joined.starts_with("- ") {
            return Err(format!(
                "starts with `{c}`, which YAML does not read as plain text"
            ));
        }
    }
    let value = joined.split(" #").next().unwrap_or_default().trim();
    if value.contains(": ") {
        return Err("contains `: ` outside quotes".into());
    }
    Ok(value.to_string())
}

fn quoted(body: &str, quote: char) -> Result<String, String> {
    let mut out = String::new();
    let mut chars = body.char_indices().peekable();
    while let Some((at, c)) = chars.next() {
        if quote == '"' && c == '\\' {
            if let Some((_, next)) = chars.next() {
                out.push(next);
            }
            continue;
        }
        if c == quote {
            if quote == '\'' && chars.peek().is_some_and(|(_, n)| *n == '\'') {
                chars.next();
                out.push('\'');
                continue;
            }
            let rest = body.get(at + 1..).unwrap_or_default().trim();
            if !rest.is_empty() && !rest.starts_with('#') {
                return Err("has text after its closing quote".into());
            }
            return Ok(out);
        }
        out.push(c);
    }
    Err(format!("has an unclosed `{quote}`"))
}

/// Candidate skill names a body mentions as `**name**`, `` `name` skill``, or
/// `/name`. The caller keeps only names that exist on the machine.
///
/// Bold is also how prose emphasises a word, so `**name**` counts only when
/// the name is hyphenated or its line says "skill", and never as a `**label**:`.
#[must_use]
pub fn mentions(body: &str) -> Vec<String> {
    let mut out = Vec::new();
    let bytes = body.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let rest = body.get(i..).unwrap_or_default();
        if let Some(after) = rest.strip_prefix("**") {
            let name = name_prefix(after);
            let tail = after.get(name.len()..).and_then(|t| t.strip_prefix("**"));
            if let Some(tail) = tail.filter(|_| !name.is_empty()) {
                let line = line_around(body, i);
                if !tail.starts_with(':') && (name.contains('-') || line.contains("skill")) {
                    out.push(name.to_string());
                }
                i += 4 + name.len();
                continue;
            }
        } else if let Some(after) = rest.strip_prefix('`') {
            let name = name_prefix(after);
            if let Some(tail) = after.get(name.len()..).and_then(|t| t.strip_prefix('`')) {
                if !name.is_empty() && tail.trim_start_matches(' ').starts_with("skill") {
                    out.push(name.to_string());
                }
            }
        } else if let Some(after) = rest.strip_prefix('/') {
            let prev = body.get(..i).and_then(|b| b.chars().next_back());
            let opens = prev.is_none_or(opens_mention);
            let name = name_prefix(after);
            let tail = after.get(name.len()..).unwrap_or_default();
            if opens && !name.is_empty() && ends_mention(tail) {
                out.push(name.to_string());
            }
        }
        i += rest.chars().next().map_or(1, char::len_utf8);
    }
    out.sort();
    out.dedup();
    out
}

fn opens_mention(c: char) -> bool {
    c.is_whitespace() || "([{\"'\u{feff}«‹“‘（【「『《〈〔〖".contains(c)
}

fn line_around(body: &str, at: usize) -> &str {
    let start = body
        .get(..at)
        .and_then(|b| b.rfind('\n'))
        .map_or(0, |n| n + 1);
    let end = body
        .get(at..)
        .and_then(|b| b.find('\n'))
        .map_or(body.len(), |n| at + n);
    body.get(start..end).unwrap_or_default()
}

/// The longest prefix shaped like a skill name: `[a-z0-9][a-z0-9-]*`, at
/// least two characters, not ending in `-`.
fn name_prefix(s: &str) -> &str {
    let len = s
        .bytes()
        .take_while(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || *b == b'-')
        .count();
    let name = s.get(..len).unwrap_or_default();
    let name = name.trim_end_matches('-');
    if name.len() < 2 || name.starts_with('-') {
        ""
    } else {
        name
    }
}

/// A `/name` mention ends at whitespace, closing punctuation, or a sentence
/// period. Anything else (`/name/`, `/name.md`, `/name:x`) is a path or a
/// plugin command, not a skill.
fn ends_mention(tail: &str) -> bool {
    let mut chars = tail.chars();
    match chars.next() {
        None => true,
        Some(c) if c.is_whitespace() || "),;!?`'\"[]{}»›”’）】」』》〉〕〗".contains(c) => {
            true
        }
        Some('.') => chars.next().is_none_or(char::is_whitespace),
        Some(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn front(text: &str) -> Result<Frontmatter, String> {
        parse(text).0
    }

    #[test]
    fn reads_plain_quoted_and_block_values() {
        let fm = front("---\nname: a-b\ndescription: \"Says: hi\"\n---\nbody").unwrap();
        assert_eq!(fm.name, "a-b");
        assert_eq!(fm.description, "Says: hi");
        let fm = front("---\nname: x\ndescription: >\n  one\n  two\n---\n").unwrap();
        assert_eq!(fm.description, "one two");
        let fm = front("---\nname: 'it''s'\ndescription: d # note\n---\n").unwrap();
        assert_eq!(fm.name, "it's");
        assert_eq!(fm.description, "d");
    }

    #[test]
    fn other_keys_and_nested_maps_are_tolerated() {
        let fm = front(
            "---\nname: x\nmetadata:\n  author: me\nallowed-tools: [Read]\ndescription: d\n---\n",
        );
        assert!(fm.is_ok(), "{fm:?}");
    }

    #[test]
    fn rejects_what_a_yaml_reader_would_reject() {
        for (text, needle) in [
            ("no fence\n", "does not start"),
            ("---\nname: x\n", "no closing"),
            ("---\nname: x\ndescription: \"open\n---\n", "unclosed"),
            ("---\nname: x\ndescription: Use when: always\n---\n", "`: `"),
            ("---\nname: x\nname: y\ndescription: d\n---\n", "twice"),
            ("---\njust words\n---\n", "not `key: value`"),
            ("---\ndescription: d\n---\n", "no `name`"),
        ] {
            let error = front(text).unwrap_err();
            assert!(error.contains(needle), "{text:?} gave {error:?}");
        }
    }

    #[test]
    fn unclosed_fence_error_has_no_redundant_frontmatter_prefix() {
        assert_eq!(front("---\nname: x\n").unwrap_err(), "has no closing `---`");
    }

    #[test]
    fn the_body_starts_after_the_closing_fence() {
        let (_, body) = parse("---\nname: x\ndescription: d\n---\nhello\n");
        assert_eq!(body, "hello\n");
    }

    #[test]
    fn finds_the_three_mention_shapes_and_nothing_else() {
        let body = "Use the **how** skill first. Then the `swarm` skill, or run /poteto-mode.\n\
                    Per **principle-x**. Not **Bold Words**, not `code`, not ~/x/tdd, not /tmp/file.\n\
                    Not /a, not https://e.com/why, not **domains**: a label, not to **triage** them.";
        assert_eq!(
            mentions(body),
            ["how", "poteto-mode", "principle-x", "swarm"]
        );
    }

    #[test]
    fn mentions_recognizes_brackets_and_cjk_punctuation() {
        let body = "See [/bracket-skill] and {/brace-skill}.\n\
                    日本語の「/cjk-quote-skill」や（/cjk-paren-skill）を参照。";
        assert_eq!(
            mentions(body),
            [
                "brace-skill",
                "bracket-skill",
                "cjk-paren-skill",
                "cjk-quote-skill"
            ]
        );
    }
}
