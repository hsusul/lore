//! `lorectl mem propose|inbox|accept|reject|retire`: the agent write path.
//!
//! `propose` is how a test or an agent without MCP files a proposal; the body
//! is read from stdin. The rest is a person reviewing the inbox.

use std::io::{Read, Write};
use std::path::PathBuf;

use lexopt::prelude::*;
use lore_memory::inbox::{self, Proposal};
use lore_memory::note::{
    AgentKind, Anchor, LineRange, NoteId, NoteType, RelPath, Relation, Scope, SessionId,
};
use lore_memory::review::{self, ReviewError};
use lore_memory::vault::{self, Added, Vault};
use serde_json::Value;
use time::OffsetDateTime;

use crate::exit::{self, CliError};

/// The subcommands this module owns.
pub const SUBCOMMANDS: [&str; 5] = ["propose", "inbox", "accept", "reject", "retire"];

#[derive(Debug, Default, PartialEq, Eq)]
struct Args {
    help: bool,
    vault: Option<PathBuf>,
    json: bool,
    operand: Option<String>,
    agent: Option<String>,
    session: Option<String>,
    note_type: Option<String>,
    title: Option<String>,
    scope: Option<String>,
    anchors: Vec<String>,
    relation: Option<(String, String)>,
    as_slug: Option<String>,
    reason: Option<String>,
}

fn usage(message: impl Into<String>) -> CliError {
    CliError::Usage(message.into())
}

fn field(name: &str, problem: impl ToString) -> CliError {
    CliError::Review(ReviewError::Inbox(inbox::InboxError::Field {
        field: name.into(),
        problem: problem.to_string(),
    }))
}

fn parse(sub: &str, rest: &[String]) -> Result<Args, CliError> {
    let mut parser = lexopt::Parser::from_args(rest);
    let mut args = Args::default();
    let takes = |flags: &[&str], flag: &str| flags.contains(&flag);
    let allowed: &[&str] = match sub {
        "propose" => &[
            "agent", "session", "type", "title", "scope", "anchor", "updates", "extends",
            "derives", "json",
        ],
        "inbox" => &["json"],
        "accept" => &["as"],
        "reject" | "retire" => &["reason"],
        _ => &[],
    };
    while let Some(arg) = parser.next().map_err(|e| usage(e.to_string()))? {
        match arg {
            Short('h') | Long("help") => args.help = true,
            Long("vault") => args.vault = Some(PathBuf::from(value(&mut parser)?)),
            Long("json") if takes(allowed, "json") => args.json = true,
            Long(flag) if takes(allowed, flag) => {
                let flag = flag.to_string();
                let text = value(&mut parser)?;
                match flag.as_str() {
                    "agent" => args.agent = Some(text),
                    "session" => args.session = Some(text),
                    "type" => args.note_type = Some(text),
                    "title" => args.title = Some(text),
                    "scope" => args.scope = Some(text),
                    "anchor" => args.anchors.push(text),
                    "as" => args.as_slug = Some(text),
                    "reason" => args.reason = Some(text),
                    kind => {
                        if args.relation.is_some() {
                            return Err(usage(
                                "give at most one of --updates, --extends, --derives",
                            ));
                        }
                        args.relation = Some((kind.to_string(), text));
                    }
                }
            }
            Value(text)
                if args.operand.is_none() && matches!(sub, "accept" | "reject" | "retire") =>
            {
                args.operand = Some(text.to_string_lossy().into_owned());
            }
            other => return Err(usage(other.unexpected().to_string())),
        }
    }
    if args.help {
        return Ok(args);
    }
    let what = match sub {
        "accept" | "reject" => Some("PROPOSAL"),
        "retire" => Some("NOTE"),
        _ => None,
    };
    if let (Some(what), None) = (what, &args.operand) {
        return Err(usage(format!("`lorectl mem {sub}` needs a {what}")));
    }
    if matches!(sub, "reject" | "retire") && args.reason.is_none() {
        return Err(usage(format!("`lorectl mem {sub}` needs --reason")));
    }
    if sub == "propose" {
        for (flag, given) in [
            ("agent", &args.agent),
            ("session", &args.session),
            ("type", &args.note_type),
            ("title", &args.title),
        ] {
            if given.is_none() {
                return Err(usage(format!("`lorectl mem propose` needs --{flag}")));
            }
        }
    }
    Ok(args)
}

fn value(parser: &mut lexopt::Parser) -> Result<String, CliError> {
    Ok(parser
        .value()
        .map_err(|e| usage(e.to_string()))?
        .to_string_lossy()
        .into_owned())
}

/// `path` or `path:lines`.
fn parse_anchor(text: &str) -> Result<Anchor, CliError> {
    let (path, lines) = match text.rsplit_once(':') {
        Some((path, lines)) if LineRange::parse(lines).is_ok() => {
            (path, LineRange::parse(lines).ok())
        }
        _ => (text, None),
    };
    Ok(Anchor {
        path: RelPath::parse(path).map_err(|e| field("anchor", e))?,
        lines,
        hash: None,
        commit: None,
    })
}

fn build(args: Args, body: String) -> Result<Proposal, CliError> {
    let text = |v: Option<String>| v.unwrap_or_default();
    let relation = args
        .relation
        .map(|(kind, id)| Relation::parse(&kind, &id).map_err(|e| field("relation", e)))
        .transpose()?;
    Ok(Proposal {
        id: NoteId::generate(),
        agent: AgentKind::parse(&text(args.agent)).map_err(|e| field("agent", e))?,
        session: SessionId::parse(&text(args.session)).map_err(|e| field("session", e))?,
        note_type: NoteType::parse(&text(args.note_type)).map_err(|e| field("type", e))?,
        scope: match args.scope {
            Some(scope) => Scope::parse(&scope).map_err(|e| field("scope", e))?,
            None => Scope::Global,
        },
        title: text(args.title),
        body,
        anchors: args
            .anchors
            .iter()
            .map(|a| parse_anchor(a))
            .collect::<Result<_, _>>()?,
        relation,
        proposed_at: OffsetDateTime::now_utc(),
    })
}

/// Run one of [`SUBCOMMANDS`].
pub fn run(sub: &str, rest: &[String], json: bool, usage_text: &str) -> Result<u8, CliError> {
    let args = parse(sub, rest)?;
    if args.help {
        print!("{usage_text}");
        return Ok(exit::OK);
    }
    let json = json || args.json;
    let vault = Vault::open(&vault::resolve_dir(args.vault.as_deref())?)?;
    let mut out = std::io::stdout().lock();
    match sub {
        "propose" => {
            let mut body = String::new();
            std::io::stdin()
                .read_to_string(&mut body)
                .map_err(|e| usage(format!("cannot read the body from stdin: {e}")))?;
            let proposal = build(args, body)?;
            let written = inbox::propose(&vault, proposal).map_err(ReviewError::from)?;
            if json {
                let _ = writeln!(out, "{}", written.to_line());
            } else {
                let _ = writeln!(
                    out,
                    "proposed {} {}/{}",
                    written.id.as_str(),
                    inbox::INBOX_DIR,
                    written.file_name()
                );
            }
        }
        "inbox" => {
            let pending = review::inbox(&vault)?;
            let printable = |s: &str| s.chars().filter(|c| !c.is_control()).collect::<String>();
            for skipped in &pending.skipped {
                eprintln!(
                    "lorectl: skipped {}: {}",
                    printable(&skipped.at),
                    printable(&skipped.reason)
                );
            }
            if json {
                let items: Vec<Value> = pending
                    .proposals
                    .iter()
                    .filter_map(|p| serde_json::from_str(&p.to_line()).ok())
                    .collect();
                let _ = writeln!(out, "{}", Value::Array(items));
            } else {
                for p in &pending.proposals {
                    // An update keeps its target's title, so name the target.
                    let what = match &p.relation {
                        Some(Relation::Updates(target)) => match vault.find(target.as_str()) {
                            Ok(rel) => format!("updates -> {rel}"),
                            Err(_) => format!("updates -> {} (not found)", target.as_str()),
                        },
                        _ => p.title.clone(),
                    };
                    let _ = writeln!(
                        out,
                        "{}  {}  {}  {}  {what}",
                        p.id.as_str(),
                        p.source(),
                        p.note_type.as_str(),
                        p.scope,
                    );
                }
            }
        }
        "accept" => {
            let id = args.operand.unwrap_or_default();
            let done = review::accept(&vault, &id, args.as_slug.as_deref())?;
            let verb = match (done.outcome, done.updated) {
                (Added::Unchanged, _) => "unchanged",
                (Added::Committed, true) => "updated",
                (Added::Committed, false) => "accepted",
            };
            let _ = writeln!(out, "{verb} {} {}", done.rel, done.id.as_str());
        }
        "reject" => {
            let id = args.operand.unwrap_or_default();
            let fresh = review::reject(&vault, &id, &args.reason.unwrap_or_default())?;
            let verb = if fresh { "rejected" } else { "unchanged" };
            let _ = writeln!(out, "{verb} {id}");
        }
        _ => {
            let key = args.operand.unwrap_or_default();
            let (rel, outcome) = review::retire(&vault, &key, &args.reason.unwrap_or_default())?;
            let verb = match outcome {
                Added::Committed => "retired",
                Added::Unchanged => "unchanged",
            };
            let _ = writeln!(out, "{verb} {rel}");
        }
    }
    Ok(exit::OK)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(sub: &str, rest: &[&str]) -> Result<Args, CliError> {
        let rest: Vec<String> = rest.iter().map(|s| s.to_string()).collect();
        parse(sub, &rest)
    }

    #[test]
    fn review_subcommands_parse() {
        let accept = args("accept", &["01JC9V6Q4M8K2T7X3N5R0B1H6D", "--as", "x"]).unwrap();
        assert_eq!(
            accept.operand.as_deref(),
            Some("01JC9V6Q4M8K2T7X3N5R0B1H6D")
        );
        assert_eq!(accept.as_slug.as_deref(), Some("x"));
        let retire = args("retire", &["slug", "--reason", "gone"]).unwrap();
        assert_eq!(retire.reason.as_deref(), Some("gone"));
        assert!(args("inbox", &["--json"]).unwrap().json);
        let propose = args(
            "propose",
            &[
                "--agent",
                "codex",
                "--session",
                "s",
                "--type",
                "fact",
                "--title",
                "T",
                "--anchor",
                "a.rs:3-4",
                "--updates",
                "01JC9V6Q4M8K2T7X3N5R0B1H6D",
            ],
        )
        .unwrap();
        assert_eq!(propose.anchors, ["a.rs:3-4"]);
        assert_eq!(propose.relation.unwrap().0, "updates");
    }

    #[test]
    fn bad_review_arguments_are_usage_errors() {
        for (sub, rest) in [
            ("accept", &[][..]),
            ("reject", &["id"][..]),
            ("retire", &["slug"][..]),
            ("inbox", &["extra"][..]),
            ("inbox", &["--reason", "x"][..]),
            ("propose", &["--agent", "codex"][..]),
            ("accept", &["id", "--json"][..]),
            ("propose", &["--updates", "a", "--extends", "b"][..]),
        ] {
            let error = args(sub, rest).unwrap_err();
            assert_eq!(exit::exit_code(&error), exit::USAGE, "{sub} {rest:?}");
        }
    }

    #[test]
    fn an_anchor_may_carry_lines() {
        let anchor = parse_anchor("src/a.rs:12-20").unwrap();
        assert_eq!(anchor.path.as_str(), "src/a.rs");
        assert_eq!(anchor.lines.unwrap().to_string(), "12-20");
        assert_eq!(parse_anchor("src/a.rs").unwrap().lines, None);
    }
}
