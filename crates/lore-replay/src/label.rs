//! `label`: a model judge reads each case and decides whether the corrected
//! turn made a mistake, names it, writes the lesson, and picks a grader.
//! **This talks to a model provider** (see [`crate::claude`]).
//!
//! The prompt and grader vocabulary are the prototype's
//! (`eval/proto/label_cases.py`), so labels from either tool mean the same.

use std::collections::HashSet;
use std::io::Write;
use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use crate::claude::{self, head};
use crate::error::{Error, Result};
use crate::types::{Label, LabeledCase};

pub const GRADERS: &str = r#"Grader kinds (pick the most deterministic one that fits):
- "asks_permission": the mistake is asking the user to confirm or choose instead of acting. Repeat = the final assistant text asks the user whether/which to proceed and the agent did not act.
- "tool_pattern": the mistake is a specific command or tool action. Give "pattern": a Python regex matched against each Bash command or tool input JSON. Repeat = any tool call matches.
- "file_written": the mistake is creating or tracking a kind of file. Give "pattern": a regex on written/edited file paths. Repeat = any Write/Edit path matches.
- "text_pattern": the mistake is visible in the final text by a regex. Give "pattern". Repeat = final text matches.
- "judge": none of the above fits. Give "rubric": one yes/no question about the new assistant turn whose YES means the mistake repeated."#;

const PROMPT: &str = r#"You label correction points mined from a person's coding-agent sessions. For each case you see the human prompt, the assistant's actions in reply, and the human's next message (the candidate correction).

Decide:
- "mistake": true only if the assistant's reply did something the next message rejects or corrects (a wrong action, a wrong assumption, an unwanted behavior). False when the next message is a new request, a clarification of the person's own words, a question, approval, or a preference about something the assistant could not have known from the conversation so far.
- "what": one sentence naming the mistake (empty if none).
- "lesson": one imperative sentence the agent should have followed, general enough to apply in a later session (empty if none).
- "grader": an object per the grader kinds below (null if no mistake).
- "visible": true if the mistake is visible in the assistant actions shown, false if you are inferring it from the correction alone.

{graders}

Return only a JSON array, one object per case in order, each {"id", "mistake", "what", "lesson", "grader", "visible"}.

Cases:
{cases}
"#;

#[derive(Debug, Clone)]
pub struct LabelOptions {
    pub cases: PathBuf,
    pub out: PathBuf,
    pub model: String,
    pub batch: usize,
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct LabelSummary {
    pub cases: usize,
    pub already: usize,
    pub labeled: usize,
    pub mistakes: usize,
    /// Cases the judge was asked about and gave no usable label for; a later
    /// run asks again.
    pub missing: usize,
}

impl LabelSummary {
    #[must_use]
    pub fn summary(&self) -> String {
        format!(
            "{} cases: {} already labeled, {} labeled now ({} mistakes), {} without a usable label",
            self.cases, self.already, self.labeled, self.mistakes, self.missing
        )
    }
}

/// What the judge sees of one case, truncated as the prototype truncates.
#[must_use]
pub fn brief(case: &Value) -> Value {
    let str_of = |v: &Value, k: &str| v.get(k).and_then(Value::as_str).unwrap_or("").to_owned();
    let actions: Vec<Value> = case
        .get("actions")
        .and_then(Value::as_array)
        .map(|acts| {
            let skip = acts.len().saturating_sub(12);
            acts.iter()
                .skip(skip)
                .map(|a| {
                    if str_of(a, "kind") == "text" {
                        Value::from(format!("TEXT: {}", head(&str_of(a, "text"), 900)))
                    } else {
                        let input = crate::pyjson::dumps(a.get("input").unwrap_or(&Value::Null));
                        Value::from(format!("TOOL {}: {}", str_of(a, "name"), head(&input, 400)))
                    }
                })
                .collect()
        })
        .unwrap_or_default();
    json!({
        "id": str_of(case, "id"),
        "prompt": head(&str_of(case, "prompt"), 1200),
        "assistant": actions,
        "next_human_message": head(&str_of(case, "correction"), 800),
    })
}

/// The labeling prompt for a batch of cases.
#[must_use]
pub fn prompt(batch: &[Value]) -> String {
    let briefs: Vec<Value> = batch.iter().map(brief).collect();
    let cases = serde_json::to_string_pretty(&briefs).unwrap_or_default();
    PROMPT
        .replace("{graders}", GRADERS)
        .replace("{cases}", &cases)
}

/// The labels in a judge reply: the first JSON array in the text holding
/// labels. Only entries with a `mistake` key count as labels, so an array of
/// echoed case briefs is skipped. Entries that are not a label, or carry no
/// id, are dropped.
#[must_use]
pub fn parse_reply(reply: &str) -> Vec<(String, Label)> {
    for (start, _) in reply.match_indices('[') {
        let mut de = serde_json::Deserializer::from_str(&reply[start..]).into_iter::<Vec<Value>>();
        let Ok(Some(items)) = de.next().transpose() else {
            continue;
        };
        let labels: Vec<(String, Label)> = items
            .into_iter()
            .filter_map(|item| {
                item.get("mistake")?;
                let label: Label = serde_json::from_value(item).ok()?;
                Some((label.id.clone()?, label))
            })
            .collect();
        if !labels.is_empty() {
            return labels;
        }
    }
    Vec::new()
}

/// Label every case in `opts.cases` not already labeled in `opts.out`,
/// appending one line per newly labeled case.
pub fn label(opts: &LabelOptions) -> Result<LabelSummary> {
    let cases = read_values(&opts.cases)?;
    let done = labeled_ids(&opts.out)?;
    let mut summary = LabelSummary {
        cases: cases.len(),
        ..LabelSummary::default()
    };
    let todo: Vec<Value> = cases
        .into_iter()
        .filter(|c| {
            let id = c.get("id").and_then(Value::as_str).unwrap_or("");
            let seen = done.contains(id);
            summary.already += usize::from(seen);
            !seen
        })
        .collect();
    let mut out = append(&opts.out)?;
    let by = claude::judge_name(&opts.model);
    for batch in todo.chunks(opts.batch.max(1)) {
        let reply = claude::ask(&opts.model, &prompt(batch))?;
        let labels: std::collections::HashMap<String, Label> =
            parse_reply(&reply).into_iter().collect();
        for case in batch {
            let id = case.get("id").and_then(Value::as_str).unwrap_or("");
            let Some(label) = labels.get(id) else {
                summary.missing += 1;
                continue;
            };
            let mut line = case.clone();
            if let Value::Object(map) = &mut line {
                map.insert("label".into(), json!(label));
                map.insert("labeled_by".into(), Value::from(by.clone()));
            }
            writeln!(out, "{line}").map_err(|e| Error::io("write", &opts.out, e))?;
            summary.labeled += 1;
            summary.mistakes += usize::from(label.mistake);
        }
        out.flush().map_err(|e| Error::io("write", &opts.out, e))?;
    }
    Ok(summary)
}

/// Ids in a labels file whose line carries a label.
fn labeled_ids(path: &Path) -> Result<HashSet<String>> {
    Ok(read_lines::<LabeledCase>(path)?
        .into_iter()
        .filter(|c| c.label.is_some())
        .map(|c| c.id)
        .collect())
}

/// The labels file: one [`LabeledCase`] per line.
pub fn read_labels(path: &Path) -> Result<Vec<LabeledCase>> {
    read_lines(path)
}

/// Every non-blank line of a JSON-lines file, parsed. A missing file is
/// empty, so a first run and a resumed run take the same path.
pub fn read_lines<T: serde::de::DeserializeOwned>(path: &Path) -> Result<Vec<T>> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(Error::io("read", path, e)),
    };
    text.lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| {
            serde_json::from_str(l).map_err(|source| Error::Json {
                path: path.to_owned(),
                source,
            })
        })
        .collect()
}

fn read_values(path: &Path) -> Result<Vec<Value>> {
    if !path.exists() {
        return Err(Error::io(
            "read",
            path,
            std::io::Error::from(std::io::ErrorKind::NotFound),
        ));
    }
    read_lines(path)
}

/// `path` opened for appending, its folder created.
pub fn append(path: &Path) -> Result<std::fs::File> {
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent).map_err(|e| Error::io("create", parent, e))?;
    }
    std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(|e| Error::io("open", path, e))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::Grader;

    #[test]
    fn a_reply_with_prose_around_the_array_parses() {
        let reply = r#"Here you go:
[{"id":"a:1","mistake":true,"what":"w","lesson":"l","grader":{"kind":"tool_pattern","pattern":"(bad"},"visible":true},
 {"id":"a:2","mistake":false,"what":"","lesson":"","grader":null,"visible":false},
 {"mistake":true}, "junk"]
done"#;
        let labels = parse_reply(reply);
        assert_eq!(labels.len(), 2);
        assert_eq!(labels[0].0, "a:1");
        assert!(matches!(labels[0].1.grader, Some(Grader::Judge { .. })));
        assert!(!labels[1].1.mistake);
        assert!(parse_reply("no array").is_empty());
    }

    #[test]
    fn a_reply_that_echoes_the_briefs_first_yields_the_real_labels() {
        let reply = r#"Cases: [{"id":"a:1","prompt":"p"},{"id":"a:2","prompt":"q"}]
Labels:
[{"id":"a:1","mistake":true,"what":"w","lesson":"l","grader":null,"visible":true},
{"id":"a:2","mistake":false,"what":"","lesson":"","grader":null,"visible":false}]"#;
        let labels = parse_reply(reply);
        assert_eq!(labels.len(), 2);
        assert_eq!(labels[0].0, "a:1");
        assert!(labels[0].1.mistake);
        assert_eq!(labels[1].0, "a:2");
    }

    #[test]
    fn a_reply_with_brackets_in_introductory_prose_parses() {
        let reply = r#"Evaluating cases [1-2]:
[{"id":"a:1","mistake":false,"what":"","lesson":"","grader":null,"visible":false}]"#;
        let labels = parse_reply(reply);
        assert_eq!(labels.len(), 1);
        assert_eq!(labels[0].0, "a:1");

        let with_trailing = r#"Evaluating cases [1-2]:
[{"id":"a:1","mistake":false,"what":"","lesson":"","grader":null,"visible":false}]
[Note: done]"#;
        let labels = parse_reply(with_trailing);
        assert_eq!(labels.len(), 1);
        assert_eq!(labels[0].0, "a:1");
    }

    #[test]
    fn the_brief_truncates_like_the_prototype() {
        let case = json!({"id": "a:1", "prompt": "p".repeat(2000), "correction": "c",
            "actions": (0..20).map(|i| json!({"kind": "tool", "name": "Bash", "input": {"command": i}}))
                .collect::<Vec<_>>()});
        let b = brief(&case);
        assert_eq!(b["prompt"].as_str().unwrap().len(), 1200);
        assert_eq!(b["assistant"].as_array().unwrap().len(), 12);
        assert_eq!(b["assistant"][0], "TOOL Bash: {\"command\": 8}");
        assert!(prompt(&[case]).contains("\"next_human_message\": \"c\""));
    }
}
