//! Enforcement: a rule on a note that a hook uses to deny a tool call.
//!
//! Some lessons are mechanical: never add AI attribution to a commit, never
//! commit a planning doc. An agent can read such a lesson and still repeat the
//! mistake, so a note may carry `enforce` rules, and `lorectl hook pre-tool`
//! denies a tool call that one of them matches, naming the note.
//!
//! A rule is parsed once, at the note boundary, into [`Enforcement`]: its tool
//! is one of a closed set and its patterns are compiled regexes, so whatever
//! holds an `Enforcement` can match with it and never has to handle a bad one.
//! A rule that does not parse (a pattern that does not compile, matches the
//! empty string, or is too long, an unknown tool, a rule past the limit)
//! becomes a [`RuleError`] instead: that rule blocks nothing, and the note
//! stays valid, so a typo in a rule never hides the lesson from recall.

use std::fmt;
use std::path::Path;

use regex::{Regex, RegexBuilder};
use serde_json::Value;

use crate::note::{InvalidValue, Note, NoteError, NoteId, Scope, Status};
use crate::repo::RepoContext;
use crate::vault::{Vault, VaultError};

/// The longest pattern a rule may have, in bytes.
pub const PATTERN_MAX_LEN: usize = 500;
/// The compiled size a pattern may reach. Far above any narrow rule, and far
/// below what would make a hook slow.
const COMPILED_SIZE_LIMIT: usize = 1 << 20;
/// The most rules one note may carry.
pub const MAX_RULES: usize = 8;

/// What part of a tool call a rule looks at.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EnforceTool {
    /// A shell command: Claude Code's `Bash`, Codex's `Bash` and `exec`.
    Bash,
    /// The text a tool writes: `Write` content, `Edit` and `MultiEdit` new
    /// strings, `NotebookEdit` source, and the lines an `apply_patch` adds.
    Write,
    /// The path of a file a tool writes, as the agent wrote it.
    Path,
}

impl EnforceTool {
    pub const ALL: [EnforceTool; 3] = [EnforceTool::Bash, EnforceTool::Write, EnforceTool::Path];

    pub fn parse(s: &str) -> Result<Self, InvalidValue> {
        Self::ALL
            .into_iter()
            .find(|t| t.as_str() == s)
            .ok_or_else(|| InvalidValue::new(format!("expected bash, write, or path, got `{s}`")))
    }

    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            EnforceTool::Bash => "bash",
            EnforceTool::Write => "write",
            EnforceTool::Path => "path",
        }
    }

    /// How a deny reason names what matched.
    fn noun(self) -> &'static str {
        match self {
            EnforceTool::Bash => "the command",
            EnforceTool::Write => "the text written",
            EnforceTool::Path => "the path written",
        }
    }
}

/// A compiled regex and the text it was compiled from. Two patterns are equal
/// when their text is.
#[derive(Clone)]
pub struct Pattern {
    source: String,
    regex: Regex,
}

impl Pattern {
    /// Compile `source`. Refused when it is empty, too long, spans lines, does
    /// not compile within the size limit, or matches the empty string (which
    /// would deny every call).
    pub fn parse(source: &str) -> Result<Self, InvalidValue> {
        if source.is_empty() {
            return Err(InvalidValue::new("the pattern is empty"));
        }
        if source.len() > PATTERN_MAX_LEN {
            return Err(InvalidValue::new(format!(
                "the pattern is longer than {PATTERN_MAX_LEN} bytes"
            )));
        }
        if source.chars().any(char::is_control) {
            return Err(InvalidValue::new(
                "the pattern must be one line without control characters",
            ));
        }
        let regex = RegexBuilder::new(source)
            .size_limit(COMPILED_SIZE_LIMIT)
            .build()
            .map_err(|e| {
                let first = e.to_string();
                let line = first.lines().last().unwrap_or_default().trim().to_string();
                InvalidValue::new(format!("`{source}` is not a valid regex: {line}"))
            })?;
        if regex.is_match("") {
            return Err(InvalidValue::new(format!(
                "`{source}` matches empty text, so it would deny every call"
            )));
        }
        Ok(Self {
            source: source.to_string(),
            regex,
        })
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.source
    }

    /// The first span of `text` the pattern matches.
    #[must_use]
    pub fn find<'t>(&self, text: &'t str) -> Option<&'t str> {
        self.regex.find(text).map(|m| m.as_str())
    }

    #[must_use]
    pub fn is_match(&self, text: &str) -> bool {
        self.regex.is_match(text)
    }
}

impl PartialEq for Pattern {
    fn eq(&self, other: &Self) -> bool {
        self.source == other.source
    }
}

impl Eq for Pattern {}

impl fmt::Debug for Pattern {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("Pattern").field(&self.source).finish()
    }
}

/// An `enforce` rule that did not parse. Only this rule is dropped: the note
/// it is on stays valid and is recalled as usual. The rule is kept as written,
/// so rewriting the note keeps it for a person to fix.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuleError {
    /// The rule's place in the note's `enforce` list.
    pub index: usize,
    /// What is wrong, naming the field, as in `enforce[0].pattern`.
    pub error: NoteError,
    /// The list item as written, without its `- `, continuation lines
    /// indented four spaces.
    pub(crate) written: String,
}

impl fmt::Display for RuleError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.error)
    }
}

/// One rule: deny a call when `pattern` matches the part `tool` names, unless
/// `unless` matches that same text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Enforcement {
    pub tool: EnforceTool,
    pub pattern: Pattern,
    pub unless: Option<Pattern>,
}

impl Enforcement {
    /// Parse a rule from its three texts.
    pub fn parse(tool: &str, pattern: &str, unless: Option<&str>) -> Result<Self, InvalidValue> {
        Ok(Self {
            tool: EnforceTool::parse(tool)?,
            pattern: Pattern::parse(pattern)?,
            unless: unless.map(Pattern::parse).transpose()?,
        })
    }

    /// What in `call` this rule matches: the first text of its kind the
    /// pattern matches and `unless` does not, and the span it matched.
    #[must_use]
    pub fn check<'c>(&self, call: &'c ToolCall) -> Option<&'c str> {
        let texts = match self.tool {
            EnforceTool::Bash => &call.commands,
            EnforceTool::Write => &call.writes,
            EnforceTool::Path => &call.paths,
        };
        texts.iter().find_map(|text| {
            let found = self.pattern.find(text)?;
            let exempt = self.unless.as_ref().is_some_and(|u| u.is_match(text));
            (!exempt).then_some(found)
        })
    }

    /// The rule as JSON: `{tool, pattern, unless?}`.
    #[must_use]
    pub fn to_json(&self) -> Value {
        let mut map = serde_json::Map::new();
        map.insert("tool".into(), self.tool.as_str().into());
        map.insert("pattern".into(), self.pattern.as_str().into());
        if let Some(unless) = &self.unless {
            map.insert("unless".into(), unless.as_str().into());
        }
        Value::Object(map)
    }

    /// Parse the JSON [`Enforcement::to_json`] writes. The error names the
    /// key that is wrong.
    pub fn from_json(value: &Value) -> Result<Self, (&'static str, String)> {
        let map = value
            .as_object()
            .ok_or(("", "expected an object with tool and pattern".to_string()))?;
        if let Some(key) = map
            .keys()
            .find(|k| !["tool", "pattern", "unless"].contains(&k.as_str()))
        {
            return Err(("", format!("unexpected key `{key}`")));
        }
        let text = |key: &'static str| match map.get(key) {
            None | Some(Value::Null) => Ok(None),
            Some(Value::String(s)) => Ok(Some(s.as_str())),
            Some(_) => Err((key, "expected a string".to_string())),
        };
        let tool = text("tool")?.ok_or(("tool", "is required".to_string()))?;
        let pattern = text("pattern")?.ok_or(("pattern", "is required".to_string()))?;
        let unless = text("unless")?;
        Ok(Self {
            tool: EnforceTool::parse(tool).map_err(|e| ("tool", e.to_string()))?,
            pattern: Pattern::parse(pattern).map_err(|e| ("pattern", e.to_string()))?,
            unless: unless
                .map(Pattern::parse)
                .transpose()
                .map_err(|e| ("unless", e.to_string()))?,
        })
    }

    /// A one-line description: `bash '<pattern>' unless '<unless>'`.
    #[must_use]
    pub fn describe(&self) -> String {
        let mut text = format!("{} '{}'", self.tool.as_str(), self.pattern.as_str());
        if let Some(unless) = &self.unless {
            text.push_str(&format!(" unless '{}'", unless.as_str()));
        }
        text
    }
}

/// The parts of one tool call rules look at. A tool Lore does not know reads
/// as empty, so no rule matches it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ToolCall {
    /// Shell commands.
    pub commands: Vec<String>,
    /// Text written to files.
    pub writes: Vec<String>,
    /// Paths of files written, as the agent wrote them.
    pub paths: Vec<String>,
}

impl ToolCall {
    /// Read a call from an agent's tool name and input, the shape both Claude
    /// Code's and Codex's hook JSON and the archive's `tool_call` rows use.
    #[must_use]
    pub fn from_tool(name: &str, input: &Value) -> Self {
        let text = |key: &str| input.get(key).and_then(Value::as_str);
        let owned = |s: Option<&str>| s.filter(|s| !s.is_empty()).map(str::to_owned);
        let mut call = Self::default();
        match name {
            "Bash" | "bash" | "shell" | "local_shell" | "exec" | "exec_command"
            | "unified_exec" => {
                let command = match input.get("command") {
                    Some(Value::Array(argv)) => {
                        let words: Vec<&str> = argv.iter().filter_map(Value::as_str).collect();
                        Some(words.join(" "))
                    }
                    Some(Value::String(s)) => Some(s.clone()),
                    _ => owned(text("cmd")),
                };
                // The archive keeps Codex `exec` input as the code it ran.
                let command = command.or_else(|| owned(input.as_str()));
                call.commands.extend(command.filter(|c| !c.is_empty()));
            }
            "Write" => {
                call.paths.extend(owned(text("file_path")));
                call.writes.extend(owned(text("content")));
            }
            "Edit" => {
                call.paths.extend(owned(text("file_path")));
                call.writes.extend(owned(text("new_string")));
            }
            "MultiEdit" => {
                call.paths.extend(owned(text("file_path")));
                let edits = input.get("edits").and_then(Value::as_array);
                call.writes.extend(
                    edits
                        .into_iter()
                        .flatten()
                        .filter_map(|e| owned(e.get("new_string").and_then(Value::as_str))),
                );
            }
            "NotebookEdit" => {
                call.paths.extend(owned(text("notebook_path")));
                call.writes.extend(owned(text("new_source")));
            }
            "apply_patch" => {
                let patch = ["command", "input", "patch"]
                    .iter()
                    .find_map(|key| text(key))
                    .or_else(|| input.as_str())
                    .unwrap_or_default();
                call.paths = patch_paths(patch);
                let added: Vec<&str> = patch.lines().filter_map(|l| l.strip_prefix('+')).collect();
                if !added.is_empty() {
                    call.writes.push(added.join("\n"));
                }
            }
            _ => {}
        }
        call
    }

    /// Whether the call has nothing a rule could match.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.commands.is_empty() && self.writes.is_empty() && self.paths.is_empty()
    }
}

/// Every file a Codex `apply_patch` patch adds, updates, deletes, or moves to,
/// once each, in order. A patch with no file header names none.
#[must_use]
pub fn patch_paths(patch: &str) -> Vec<String> {
    const HEADERS: [&str; 4] = [
        "*** Add File:",
        "*** Update File:",
        "*** Delete File:",
        "*** Move to:",
    ];
    let mut paths: Vec<String> = Vec::new();
    for line in patch.lines() {
        let Some(path) = HEADERS
            .iter()
            .find_map(|header| line.strip_prefix(header))
            .map(str::trim)
            .filter(|path| !path.is_empty())
        else {
            continue;
        };
        if !paths.iter().any(|p| p == path) {
            paths.push(path.to_string());
        }
    }
    paths
}

/// An active note's rule, with what a deny reason names.
#[derive(Debug, Clone)]
pub struct Rule {
    pub id: NoteId,
    pub title: String,
    pub scope: Scope,
    /// The note's path in the vault.
    pub rel: String,
    pub enforcement: Enforcement,
}

/// An active note's rule that did not parse, so it blocks nothing.
#[derive(Debug, Clone)]
pub struct DroppedRule {
    pub id: NoteId,
    pub title: String,
    /// The note's path in the vault.
    pub rel: String,
    pub error: RuleError,
}

/// Every active note's rules, the rules that did not parse, and the files
/// that mention `enforce:` but are not valid notes (so their rules block
/// nothing).
#[derive(Debug, Default)]
pub struct Rules {
    pub rules: Vec<Rule>,
    pub dropped: Vec<DroppedRule>,
    pub broken: Vec<(String, NoteError)>,
}

impl Rules {
    /// Read every rule in the vault. Only files that have an `enforce:` line
    /// are parsed, so the common case is one read per file and no parse.
    /// Reads only: the index is not touched.
    pub fn load(vault: &Vault) -> Result<Self, VaultError> {
        let mut out = Self::default();
        for stamp in vault.stamps()? {
            let Ok(text) = std::fs::read_to_string(vault.root().join(&stamp.path)) else {
                continue;
            };
            if !has_enforce_key(&text) {
                continue;
            }
            let note = match Note::parse(&text) {
                Ok(note) => note,
                Err(error) => {
                    out.broken.push((stamp.path, error));
                    continue;
                }
            };
            if note.status != Status::Active {
                continue;
            }
            for error in note.enforce_errors {
                out.dropped.push(DroppedRule {
                    id: note.id.clone(),
                    title: note.title.clone(),
                    rel: stamp.path.clone(),
                    error,
                });
            }
            for enforcement in note.enforce {
                out.rules.push(Rule {
                    id: note.id.clone(),
                    title: note.title.clone(),
                    scope: note.scope.clone(),
                    rel: stamp.path.clone(),
                    enforcement,
                });
            }
        }
        Ok(out)
    }

    /// The rules that apply in `cwd`: global ones, and those of the
    /// repository at `cwd`. The repository is only looked up when a rule is
    /// scoped to one.
    #[must_use]
    pub fn in_scope(&self, cwd: &Path) -> Vec<&Rule> {
        let mut context: Option<RepoContext> = None;
        self.rules
            .iter()
            .filter(|rule| match &rule.scope {
                Scope::Global => true,
                Scope::Repo(key) => context
                    .get_or_insert_with(|| RepoContext::discover(cwd))
                    .key()
                    .is_some_and(|here| here == key),
            })
            .collect()
    }
}

/// The first rule in `rules` that denies `call`, and the span it matched.
#[must_use]
pub fn first_denial<'r, 'c>(rules: &[&'r Rule], call: &'c ToolCall) -> Option<(&'r Rule, &'c str)> {
    rules
        .iter()
        .find_map(|rule| rule.enforcement.check(call).map(|found| (*rule, found)))
}

/// The reason a deny gives: `Lore note <id>: <title>. <what matched>`, on one
/// line, with the matched text cut to 80 characters.
#[must_use]
pub fn deny_reason(rule: &Rule, found: &str) -> String {
    let mut shown: String = found
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .take(80)
        .collect();
    if found.chars().count() > 80 {
        shown.push_str("...");
    }
    format!(
        "Lore note {}: {}. {} matches `{}`: \"{}\". Follow the note instead.",
        rule.id.as_str(),
        rule.title,
        capitalize(rule.enforcement.tool.noun()),
        rule.enforcement.pattern.as_str(),
        shown
    )
}

fn capitalize(text: &str) -> String {
    let mut chars = text.chars();
    chars
        .next()
        .map(|first| first.to_uppercase().chain(chars).collect())
        .unwrap_or_default()
}

/// Whether the frontmatter of `text` has a top-level `enforce:` key.
fn has_enforce_key(text: &str) -> bool {
    let Some(rest) = text.strip_prefix("---\n") else {
        return false;
    };
    rest.lines().take_while(|line| *line != "---").any(|line| {
        line.strip_prefix("enforce")
            .is_some_and(|r| r.trim_start().starts_with(':'))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn rule(tool: &str, pattern: &str, unless: Option<&str>) -> Enforcement {
        Enforcement::parse(tool, pattern, unless).unwrap()
    }

    #[test]
    fn a_bad_pattern_is_refused_not_compiled() {
        for bad in ["", "(unclosed", ".*", "a?", "x\ny"] {
            assert!(Pattern::parse(bad).is_err(), "{bad:?}");
        }
        assert!(Pattern::parse(&"a".repeat(PATTERN_MAX_LEN + 1)).is_err());
        assert!(EnforceTool::parse("Bash").is_err());
    }

    #[test]
    fn unless_exempts_the_text_it_matches() {
        let r = rule("bash", r"git push --force", Some("--force-with-lease"));
        let call = |c: &str| ToolCall::from_tool("Bash", &json!({ "command": c }));
        assert_eq!(r.check(&call("git push --force")), Some("git push --force"));
        assert_eq!(r.check(&call("git push --force-with-lease")), None);
        assert_eq!(r.check(&call("git status")), None);
    }

    #[test]
    fn tool_inputs_read_as_their_parts() {
        let codex = ToolCall::from_tool("Bash", &json!({ "command": ["bash", "-lc", "ls"] }));
        assert_eq!(codex.commands, ["bash -lc ls"]);
        let patch = "*** Begin Patch\n*** Add File: PLAN.md\n+step one\n+step two\n*** End Patch";
        let call = ToolCall::from_tool("apply_patch", &json!({ "command": patch }));
        assert_eq!(call.paths, ["PLAN.md"]);
        assert_eq!(call.writes, ["step one\nstep two"]);
        let multi = ToolCall::from_tool(
            "MultiEdit",
            &json!({ "file_path": "/r/a.rs", "edits": [{ "new_string": "x" }, { "new_string": "y" }] }),
        );
        assert_eq!((multi.paths.len(), multi.writes.len()), (1, 2));
        assert!(ToolCall::from_tool("Read", &json!({ "file_path": "/r/a" })).is_empty());
    }

    #[test]
    fn json_round_trips_and_names_the_bad_key() {
        let r = rule("path", r"(^|/)PLAN\.md$", Some("docs/"));
        assert_eq!(Enforcement::from_json(&r.to_json()).unwrap(), r);
        let bad = json!({ "tool": "bash", "pattern": "(" });
        assert_eq!(Enforcement::from_json(&bad).unwrap_err().0, "pattern");
        let extra = json!({ "tool": "bash", "pattern": "x", "when": "y" });
        assert!(Enforcement::from_json(&extra).is_err());
    }

    #[test]
    fn only_a_top_level_enforce_key_counts() {
        assert!(has_enforce_key(
            "---\nid: x\nenforce:\n  - tool: bash\n---\n"
        ));
        assert!(!has_enforce_key("---\nid: x\n---\nenforce: in the body\n"));
        assert!(!has_enforce_key("---\nenforcement: x\n---\n"));
    }
}
