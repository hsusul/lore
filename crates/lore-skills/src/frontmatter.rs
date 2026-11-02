//! SKILL.md frontmatter.
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
        return (Err("frontmatter has no closing `---`".into()), "");
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
    fn the_body_starts_after_the_closing_fence() {
        let (_, body) = parse("---\nname: x\ndescription: d\n---\nhello\n");
        assert_eq!(body, "hello\n");
    }
}
