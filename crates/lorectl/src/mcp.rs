//! `lorectl mcp` — the archive as a Model Context Protocol server.
//!
//! An agent launches this as a child process and speaks JSON-RPC 2.0 to it,
//! one message per line on stdin and stdout (the MCP stdio transport). Pipes
//! only: the no-network guarantee covers this command like every other.
//!
//! ## Only redacted text leaves
//!
//! Whatever a tool returns is sent on to the agent's model provider, so every
//! tool reads from the redacted search projection and nothing else. That is
//! why `read_session` is built on [`search::session_documents_page`] rather
//! than `query::get_session`, which returns cleartext for the local UI, and why
//! no tool returns a session title: `agent_session.title` is cleartext too (a
//! fallback title echoes the first user message), while the redacted copy of a
//! native title is already the first entry `read_session` returns.
//!
//! Memory notes are different: a person writes them by hand and nothing scans
//! them, so `recall_memory` and `read_note` pass every title, excerpt, and body
//! through [`inbox::redact`] on the way out.
//!
//! `propose_memory` is the one tool that writes, and only through
//! [`inbox::propose`]: one redacted line in `<vault>/_inbox/<agent>-<session>.jsonl`,
//! never a note. A person accepts it into memory.
//!
//! ## Failure stays inside the protocol
//!
//! Stdout belongs to the protocol, so nothing else is ever printed to it. A
//! tool that fails (no archive yet, an unknown session) answers with an
//! `isError` result the agent can read, rather than ending the server: an
//! archive that appears later is picked up on the next call, because each call
//! opens the database afresh.

use std::fs;
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};

use lore_core::paths::ARCHIVE_DB_FILENAME;
use lore_core::search::{self, SortOrder, HIGHLIGHT_END, HIGHLIGHT_START};
use lore_core::{query, storage};
use lore_memory::anchor::WorkTree;
use lore_memory::inbox::{InboxError, Proposal};
use lore_memory::note::{
    AgentKind, Anchor, InvalidValue, LineRange, Note, NoteId, NoteType, RelPath, Relation, Scope,
    SessionId,
};
use lore_memory::repo::RepoContext;
use lore_memory::vault::{self, Vault, VaultError};
use lore_memory::{inbox, index, mine, recall};
use rusqlite::Connection;
use serde_json::{json, Value};
use time::OffsetDateTime;

use crate::cli::Invocation;
use crate::exit::{self, CliError};

/// The protocol revisions this server can speak. The tools surface it uses is
/// the same in all of them, so a client's requested revision is echoed back
/// when it is one of these, and the newest is offered otherwise.
const PROTOCOL_VERSIONS: &[&str] = &["2025-06-18", "2025-03-26", "2024-11-05"];

const SEARCH_LIMIT: i64 = 20;
const READ_PAGE_DOCUMENTS: i64 = 50;
/// Characters of session text per `read_session` page, so one call cannot
/// flood the agent's context window.
const READ_PAGE_CHARS: usize = 24_000;

const RECALL_DEFAULT_LIMIT: u64 = 5;
const RECALL_MAX_LIMIT: u64 = 10;
const RECALL_EXCERPT_CHARS: usize = 300;
/// Characters of serialized `recall_memory` reply, so recall stays a glance.
const RECALL_REPLY_CHARS: usize = 6_000;
/// Characters of note body `read_note` returns, Timeline excluded.
const READ_NOTE_BODY_CHARS: usize = 8_000;

const MINE_DEFAULT_LIMIT: u64 = 20;
const MINE_MAX_LIMIT: u64 = 50;
/// Receipts returned per candidate.
const MINE_RECEIPTS: usize = 5;
/// Characters of serialized `mine_candidates` reply.
const MINE_REPLY_CHARS: usize = 16_000;

const PROPOSE_TITLE_CHARS: usize = 120;
const PROPOSE_BODY_CHARS: usize = 4_000;
const PROPOSE_MAX_ANCHORS: usize = 8;

const NOTE_ARG_PROBLEM: &str = "must be a note id, file name, or path inside the vault";
const NOTE_NOT_FOUND_FIX: &str = "Call recall_memory to get a current note id.";
const NOTE_AMBIGUOUS_FIX: &str = "Pass the note id instead of the slug.";
const VAULT_CONFIG_FIX: &str = "Set LORE_VAULT_DIR to an absolute path in this MCP server's env.";
const VAULT_BUSY_FIX: &str =
    "Another Lore process holds the vault lock. Retry once in a few seconds.";
const INTERNAL_FIX: &str = "Nothing was written. Tell the user. `lorectl mem reindex` may fix it.";
const AGENT_UNKNOWN_FIX: &str =
    "Ask the user to set LORE_AGENT=claude-code or codex in the MCP server config.";
const ANCHOR_NOT_FOUND_FIX: &str = "Anchor paths are relative to the repo root and must exist. \
Check path and lines, or omit lines.";
const SCOPE_MISMATCH_FIX: &str = "Anchors need scope repo and a cwd inside that repo.";
const SESSION_MISMATCH_FIX: &str = "That session belongs to another agent. Omit session_id.";

const INSTRUCTIONS: &str = "Lore archives past Claude Code and Codex sessions on this machine. \
Use search_sessions to find earlier work, decisions, or errors, then read_session to read one. \
Use recall_memory with your working directory to get accepted notes about the repo you are in, \
then read_note to read one in full. \
Use mine_candidates to find corrections and rules the person gave agents, then file lessons. \
Use propose_memory to file a note for a person to review. \
Text that matched a secret detector is masked.";

/// JSON-RPC error codes this server answers with.
const PARSE_ERROR: i64 = -32700;
const INVALID_REQUEST: i64 = -32600;
const METHOD_NOT_FOUND: i64 = -32601;
const INVALID_PARAMS: i64 = -32602;

struct Tool {
    name: &'static str,
    description: &'static str,
    input_schema: fn() -> Value,
    /// A string error is plain text, as the first two tools have always
    /// answered. Any other error is `{code, message, fix, field?}`.
    call: fn(&Ctx, &Value) -> Result<Value, Value>,
}

/// What one connection knows beyond each request.
struct Ctx {
    archive_dir: PathBuf,
    /// `LORE_AGENT` from the server's env. It wins over `clientInfo`.
    env_agent: Option<String>,
    /// `clientInfo.name` from `initialize`.
    client: Option<String>,
    /// `mcp-<ULID>` for proposals that name no session, so parallel servers
    /// never share an inbox file. A `.` splits the ULID, which whole is one
    /// high-entropy token the inbox would refuse as a secret-looking session.
    session: String,
}

impl Ctx {
    fn agent(&self) -> Option<AgentKind> {
        match self.env_agent.as_deref().or(self.client.as_deref())? {
            "claude-code" => Some(AgentKind::ClaudeCode),
            name => name.starts_with("codex").then_some(AgentKind::Codex),
        }
    }
}

const TOOLS: &[Tool] = &[
    Tool {
        name: "search_sessions",
        description: "Full-text search across archived coding-agent sessions. Plain words are \
ANDed. Filters: agent:claude-code|codex, path:<file>, tool:<name>, model:<id>, repo:<id>, \
branch:<name>, commit:<sha>, before:<date>, after:<date>, has:error, has:patch. Returns \
session ids with redacted snippets.",
        input_schema: search_schema,
        call: search_sessions,
    },
    Tool {
        name: "read_session",
        description: "Read one archived session's text in order: its title, each message \
labelled by role, then any recorded patches. Paged; pass next_cursor back to continue. \
Thinking and raw tool payloads are not included.",
        input_schema: read_schema,
        call: read_session,
    },
    Tool {
        name: "recall_memory",
        description: "Call before you start or change direction in a repo, to get accepted \
notes (conventions, pitfalls, decisions) for it, each labelled with whether its code anchors \
still match.",
        input_schema: recall_schema,
        call: recall_memory,
    },
    Tool {
        name: "read_note",
        description: "Call when a recalled note's excerpt is not enough, or before proposing \
an update to it.",
        input_schema: read_note_schema,
        call: read_note,
    },
    Tool {
        name: "mine_candidates",
        description: "Call when asked to find lessons in past sessions. Returns candidate \
corrections and standing rules the person gave agents, newest first, each with receipts: the \
session, agent, date, what the person said, and what the agent had just done. Several \
candidates often say one thing in different words. Group them by meaning, write each group as \
one short rule in plain words, and file each rule once with propose_memory, citing its \
sessions in the body and passing one of them as session_id. Leave out anything that only \
redirected the task at hand.",
        input_schema: mine_schema,
        call: mine_candidates,
    },
    Tool {
        name: "propose_memory",
        description: "Call once you have verified something a future agent here should know. \
A person reviews it before it becomes memory. Pass `updates` with a note id to supersede that \
note. Retiring a note needs a person.",
        input_schema: propose_schema,
        call: propose_memory,
    },
];

fn search_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "query": { "type": "string", "description": "Words and filters to search for." },
            "sort": { "type": "string", "enum": ["relevance", "newest", "oldest"] },
            "cursor": { "type": "string", "description": "next_cursor from a previous call." }
        },
        "required": ["query"]
    })
}

fn read_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "session_id": { "type": "string" },
            "cursor": { "type": "string", "description": "next_cursor from a previous call." }
        },
        "required": ["session_id"]
    })
}

fn recall_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "cwd": { "type": "string", "description": "Absolute path you are working in." },
            "query": { "type": "string",
                "description": "Words the notes should match. Omit to get every note for the repo." },
            "limit": {
                "type": "integer",
                "minimum": 1,
                "maximum": RECALL_MAX_LIMIT,
                "default": RECALL_DEFAULT_LIMIT
            }
        },
        "required": ["cwd"]
    })
}

fn mine_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "limit": {
                "type": "integer",
                "minimum": 1,
                "maximum": MINE_MAX_LIMIT,
                "default": MINE_DEFAULT_LIMIT
            }
        }
    })
}

fn read_note_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "note": {
                "type": "string",
                "description": "A note id from recall_memory, the note's file name, or its path in the vault."
            },
            "cwd": {
                "type": "string",
                "description": "Absolute path you are working in, to check the note's code anchors."
            }
        },
        "required": ["note"]
    })
}

fn propose_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "title": { "type": "string", "maxLength": PROPOSE_TITLE_CHARS },
            "body": { "type": "string", "maxLength": PROPOSE_BODY_CHARS,
                "description": "Markdown. Lore writes the `## Timeline` section." },
            "type": { "type": "string", "enum": NoteType::ALL.map(NoteType::as_str) },
            "scope": { "type": "string", "enum": ["global", "repo"],
                "description": "repo means the repository at cwd." },
            "cwd": { "type": "string",
                "description": "Absolute path you are working in. Required for scope repo." },
            "anchors": { "type": "array", "maxItems": PROPOSE_MAX_ANCHORS, "items": {
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "Relative to the repo root." },
                    "lines": { "type": "string", "description": "Like `12-20`." }
                },
                "required": ["path"]
            } },
            "updates": { "type": "string", "description": "Id of the note this supersedes." },
            "session_id": { "type": "string", "description": "Your own session id, if known." }
        },
        "required": ["title", "body", "type", "scope"]
    })
}

/// Serve MCP on stdin/stdout until stdin closes.
pub fn run(invocation: &Invocation) -> Result<u8, CliError> {
    let mut ctx = Ctx {
        archive_dir: invocation.archive_dir()?,
        env_agent: std::env::var("LORE_AGENT").ok().filter(|a| !a.is_empty()),
        client: None,
        session: {
            let ulid = NoteId::generate();
            let (time, random) = ulid.as_str().split_at(10);
            format!("mcp-{time}.{random}")
        },
    };
    let stdin = std::io::stdin().lock();
    let mut stdout = std::io::stdout().lock();
    for line in stdin.lines() {
        let Ok(line) = line else { break };
        if line.trim().is_empty() {
            continue;
        }
        if let Some(response) = handle(&mut ctx, &line) {
            if writeln!(stdout, "{response}")
                .and_then(|()| stdout.flush())
                .is_err()
            {
                break;
            }
        }
    }
    Ok(exit::OK)
}

/// Answer one line of input. `None` for a notification, which gets no reply.
fn handle(ctx: &mut Ctx, line: &str) -> Option<Value> {
    let Ok(message) = serde_json::from_str::<Value>(line) else {
        return Some(error(&Value::Null, PARSE_ERROR, "not valid JSON"));
    };
    let id = message.get("id").cloned();
    let Some(method) = message.get("method").and_then(Value::as_str) else {
        return Some(error(
            &id.unwrap_or(Value::Null),
            INVALID_REQUEST,
            "not a JSON-RPC request",
        ));
    };
    let id = id?;
    let params = message.get("params").cloned().unwrap_or(Value::Null);

    let result = match method {
        "initialize" => {
            let client = params.pointer("/clientInfo/name").and_then(Value::as_str);
            ctx.client = client.map(str::to_owned);
            Ok(initialize(&params))
        }
        "ping" => Ok(json!({})),
        "tools/list" => Ok(json!({ "tools": TOOLS.iter().map(describe).collect::<Vec<_>>() })),
        "tools/call" => call_tool(ctx, &params),
        _ => Err((METHOD_NOT_FOUND, format!("unknown method `{method}`"))),
    };
    Some(match result {
        Ok(result) => json!({ "jsonrpc": "2.0", "id": id, "result": result }),
        Err((code, message)) => error(&id, code, &message),
    })
}

fn initialize(params: &Value) -> Value {
    let requested = params.get("protocolVersion").and_then(Value::as_str);
    let version = requested
        .filter(|v| PROTOCOL_VERSIONS.contains(v))
        .unwrap_or(PROTOCOL_VERSIONS[0]);
    json!({
        "protocolVersion": version,
        "capabilities": { "tools": {} },
        "serverInfo": { "name": "lorectl", "version": lore_core::version() },
        "instructions": INSTRUCTIONS,
    })
}

fn describe(tool: &Tool) -> Value {
    json!({
        "name": tool.name,
        "description": tool.description,
        "inputSchema": (tool.input_schema)(),
    })
}

fn call_tool(ctx: &Ctx, params: &Value) -> Result<Value, (i64, String)> {
    let name = params
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let tool = TOOLS
        .iter()
        .find(|t| t.name == name)
        .ok_or_else(|| (INVALID_PARAMS, format!("unknown tool `{name}`")))?;
    let arguments = params.get("arguments").cloned().unwrap_or(json!({}));
    Ok(match (tool.call)(ctx, &arguments) {
        Ok(value) => json!({
            "content": [{ "type": "text", "text": value.to_string() }],
            "structuredContent": value,
        }),
        Err(Value::String(message)) => json!({
            "content": [{ "type": "text", "text": message }],
            "isError": true,
        }),
        Err(error) => json!({
            "content": [{ "type": "text", "text": error.to_string() }],
            "structuredContent": error,
            "isError": true,
        }),
    })
}

fn tool_error(code: &str, message: impl ToString, fix: &str) -> Value {
    json!({ "code": code, "message": message.to_string(), "fix": fix })
}

fn invalid_argument(field: &str, problem: &str) -> Value {
    let mut error = tool_error(
        "invalid_argument",
        format!("`{field}` {problem}"),
        &format!("Fix `{field}`: {problem}, then call again."),
    );
    error["field"] = json!(field);
    error
}

fn vault_error(error: &VaultError) -> Value {
    let (code, fix) = match error {
        VaultError::RelativeDir(_) | VaultError::NoHome => ("vault_config", VAULT_CONFIG_FIX),
        VaultError::Busy(_) => ("vault_busy", VAULT_BUSY_FIX),
        _ => ("internal", INTERNAL_FIX),
    };
    tool_error(code, error, fix)
}

fn error(id: &Value, code: i64, message: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}

/// Open the archive read-only, turning failure into text an agent can act on.
fn open(archive_dir: &Path) -> Result<Connection, String> {
    storage::open_read_only(&archive_dir.join(ARCHIVE_DB_FILENAME)).map_err(|e| {
        let error = CliError::from(e);
        match error.advice() {
            Some(advice) => format!("{error}; {advice}"),
            None => error.to_string(),
        }
    })
}

fn string_arg<'a>(arguments: &'a Value, name: &str) -> Option<&'a str> {
    arguments.get(name).and_then(Value::as_str)
}

fn search_sessions(ctx: &Ctx, arguments: &Value) -> Result<Value, Value> {
    let query_text = string_arg(arguments, "query").ok_or("`query` is required")?;
    let sort = SortOrder::parse(string_arg(arguments, "sort"));
    let conn = open(&ctx.archive_dir)?;
    let page = search::search_page(
        &conn,
        query_text,
        SEARCH_LIMIT,
        string_arg(arguments, "cursor"),
        sort,
    )
    .map_err(|e| e.to_string())?;
    let hits: Vec<Value> = page
        .hits
        .iter()
        .map(|hit| {
            json!({
                "session_id": hit.session_id,
                "agent_id": hit.agent_id,
                "started_at_ms": hit.started_at,
                "source_kind": hit.source_kind,
                "snippet": hit.snippet.replace(HIGHLIGHT_START, "").replace(HIGHLIGHT_END, ""),
            })
        })
        .collect();
    Ok(json!({ "hits": hits, "next_cursor": page.next_cursor }))
}

fn read_session(ctx: &Ctx, arguments: &Value) -> Result<Value, Value> {
    let session_id = string_arg(arguments, "session_id").ok_or("`session_id` is required")?;
    let after_id = match string_arg(arguments, "cursor") {
        Some(cursor) => Some(cursor.parse::<i64>().map_err(|_| "`cursor` is not valid")?),
        None => None,
    };
    let conn = open(&ctx.archive_dir)?;
    let summary = query::get_session(&conn, session_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("no session `{session_id}` in the archive"))?
        .summary;
    let documents =
        search::session_documents_page(&conn, session_id, after_id, READ_PAGE_DOCUMENTS)
            .map_err(|e| e.to_string())?;

    let full_page = documents.len() == usize::try_from(READ_PAGE_DOCUMENTS).unwrap_or(usize::MAX);
    let mut budget = READ_PAGE_CHARS;
    let mut entries = Vec::new();
    let mut last_id = None;
    for document in &documents {
        if budget == 0 {
            break;
        }
        let length = document.text.chars().count();
        let (text, truncated) = if length > budget {
            (document.text.chars().take(budget).collect(), true)
        } else {
            (document.text.clone(), false)
        };
        budget = budget.saturating_sub(length);
        last_id = Some(document.id);
        let mut entry = json!({ "label": document.label, "text": text });
        if truncated {
            entry["truncated"] = json!(true);
        }
        entries.push(entry);
    }
    let more = entries.len() < documents.len() || full_page;
    let next_cursor = last_id.filter(|_| more).map(|id| id.to_string());

    Ok(json!({
        "session_id": summary.id,
        "agent_id": summary.agent_id,
        "started_at_ms": summary.started_at,
        "entries": entries,
        "next_cursor": next_cursor,
    }))
}

/// Recall never creates a vault: none yet is a normal state, not an error.
/// Without words to match, it returns every note that applies at `cwd`.
fn recall_memory(_ctx: &Ctx, arguments: &Value) -> Result<Value, Value> {
    let cwd = string_arg(arguments, "cwd")
        .map(Path::new)
        .filter(|cwd| cwd.is_absolute())
        .ok_or_else(|| invalid_argument("cwd", "must be an absolute path"))?;
    // No query, or one with no words, recalls every note in scope.
    let query_text = match arguments.get("query") {
        None => "",
        Some(query) => query
            .as_str()
            .ok_or_else(|| invalid_argument("query", "must be a string"))?,
    };
    let limit = match arguments.get("limit") {
        None => RECALL_DEFAULT_LIMIT,
        Some(limit) => limit
            .as_u64()
            .filter(|n| (1..=RECALL_MAX_LIMIT).contains(n))
            .ok_or_else(|| invalid_argument("limit", "must be an integer from 1 to 10"))?,
    };
    let dir = vault::resolve_dir(None).map_err(|e| vault_error(&e))?;
    let vault = match Vault::open(&dir) {
        Ok(vault) => vault,
        Err(VaultError::Missing(_)) => return Ok(json!({ "vault": "missing", "notes": [] })),
        Err(e) => return Err(vault_error(&e)),
    };
    let limit = usize::try_from(limit).unwrap_or(usize::MAX);
    let found = recall::recall(&vault, query_text, cwd, limit).map_err(|e| vault_error(&e))?;
    let mut notes = Vec::new();
    for recalled in &found.notes {
        let excerpt: String = redact(&recalled.excerpt)?
            .chars()
            .take(RECALL_EXCERPT_CHARS)
            .collect();
        notes.push(json!({
            "id": recalled.note.id.as_str(),
            "title": redact(&recalled.note.title)?,
            "scope": recalled.note.scope.to_string(),
            "freshness": recalled.freshness.label(),
            "excerpt": excerpt,
        }));
    }
    let mut truncated = false;
    loop {
        let reply = json!({ "vault": "ok", "notes": notes, "truncated": truncated });
        if notes.is_empty() || reply.to_string().chars().count() <= RECALL_REPLY_CHARS {
            return Ok(reply);
        }
        notes.pop();
        truncated = true;
    }
}

/// One note by id, file name, or vault path. Read never creates a vault and
/// never returns a file from outside it.
fn read_note(_ctx: &Ctx, arguments: &Value) -> Result<Value, Value> {
    let bad_note = || invalid_argument("note", NOTE_ARG_PROBLEM);
    let key = string_arg(arguments, "note")
        .filter(|key| !key.is_empty() && !Path::new(key).is_absolute())
        .filter(|key| !key.split(['/', '\\']).any(|part| part.starts_with('.')))
        .ok_or_else(bad_note)?;
    // A bare file name is the slug, wherever the note's folder is.
    let key = match key.strip_suffix(".md") {
        Some(slug) if !key.contains('/') => slug,
        _ => key,
    };
    let cwd = match arguments.get("cwd") {
        None => None,
        Some(cwd) => Some(
            cwd.as_str()
                .map(Path::new)
                .filter(|cwd| cwd.is_absolute())
                .ok_or_else(|| invalid_argument("cwd", "must be an absolute path"))?,
        ),
    };
    let vault = open_vault()?;
    let rel = vault.find(key).map_err(|e| match e {
        VaultError::BadPath(..) => bad_note(),
        VaultError::NotFound(_) => tool_error("note_not_found", e, NOTE_NOT_FOUND_FIX),
        VaultError::Ambiguous(..) => {
            let mut error = tool_error("note_ambiguous", e, NOTE_AMBIGUOUS_FIX);
            let candidates = vault.index().ok().and_then(|index| index.lookup(key).ok());
            error["candidates"] = json!(candidates.unwrap_or_default());
            error
        }
        e => vault_error(&e),
    })?;
    // A symlink in the vault must not lead out of it.
    let inside = fs::canonicalize(vault.root().join(&rel))
        .ok()
        .zip(fs::canonicalize(vault.root()).ok())
        .is_some_and(|(file, root)| file.starts_with(root));
    if !inside {
        return Err(bad_note());
    }
    let text = vault.read(&rel).map_err(|e| vault_error(&e))?;
    let note = Note::parse(&text).map_err(|e| vault_error(&e.into()))?;

    let (body, timeline) = index::split_timeline(&note.body);
    let timeline_entries = timeline.lines().filter(|l| l.starts_with("- ")).count();
    let context = cwd.map_or(RepoContext::NoRepo, RepoContext::discover);
    let tree = context.root().map(WorkTree::new);
    let (_, freshness) = recall::check_note(&note, &context, tree.as_ref());
    let body = redact(body.trim_end())?;
    let truncated = body.chars().count() > READ_NOTE_BODY_CHARS;
    Ok(json!({
        "id": note.id.as_str(),
        "title": redact(&note.title)?,
        "scope": note.scope.to_string(),
        "status": note.status.as_str(),
        "freshness": freshness.label(),
        "body": body.chars().take(READ_NOTE_BODY_CHARS).collect::<String>(),
        "timeline_entries": timeline_entries,
        "truncated": truncated,
    }))
}

/// The configured vault, or `vault_missing`. Never creates one.
fn open_vault() -> Result<Vault, Value> {
    let dir = vault::resolve_dir(None).map_err(|e| vault_error(&e))?;
    Vault::open(&dir).map_err(|e| match e {
        VaultError::Missing(_) => {
            let fix = format!(
                "No memory vault at {}. Continue without memory. \
                 Tell the user `lorectl mem init` creates one.",
                dir.display()
            );
            tool_error("vault_missing", e, &fix)
        }
        e => vault_error(&e),
    })
}

/// File a note in the vault inbox for a person to review. A `session_id` the
/// archive holds for this agent is stored as its archive id and marked
/// verified; one it holds for another agent is refused; any other is stored
/// as given. Without `session_id`, the session is the connection's `mcp-<ULID>`.
fn propose_memory(ctx: &Ctx, arguments: &Value) -> Result<Value, Value> {
    let unknown = "cannot tell which agent is calling";
    let agent = ctx
        .agent()
        .ok_or_else(|| tool_error("agent_unknown", unknown, AGENT_UNKNOWN_FIX))?;
    for (field, max) in [("title", PROPOSE_TITLE_CHARS), ("body", PROPOSE_BODY_CHARS)] {
        if string_arg(arguments, field).map_or(0, |s| s.chars().count()) > max {
            let problem = format!("must be at most {max} characters");
            return Err(invalid_argument(field, &problem));
        }
    }
    let note_type = string_arg(arguments, "type")
        .and_then(|t| NoteType::parse(t).ok())
        .ok_or_else(|| {
            let problem = "must be one of convention, decision, fact, howto, pitfall";
            invalid_argument("type", problem)
        })?;
    let repo_scope = match string_arg(arguments, "scope") {
        Some("global") => false,
        Some("repo") => true,
        _ => return Err(invalid_argument("scope", "must be global or repo")),
    };
    let mut anchors = proposed_anchors(arguments.get("anchors"))?;
    let session = match arguments.get("session_id") {
        None => Some(ctx.session.as_str()),
        Some(id) => id.as_str(),
    }
    .and_then(|id| SessionId::parse(id).ok())
    .ok_or_else(|| invalid_argument("session_id", "must be letters, digits, `.`, `_`, `-`"))?;
    let owner = (arguments.get("session_id"))
        .and_then(|_| archived_session(&ctx.archive_dir, session.as_str()));
    let source_verified = owner.is_some();
    let session = match owner {
        None => session,
        Some(owner) if owner.agent_id == agent.as_str() => SessionId::parse(&owner.archive_id)
            .map_err(|e| tool_error("internal", e, INTERNAL_FIX))?,
        Some(owner) => {
            let given = session.as_str();
            let mismatch = format!("session `{given}` was recorded by {}", owner.agent_id);
            return Err(tool_error(
                "session_mismatch",
                mismatch,
                SESSION_MISMATCH_FIX,
            ));
        }
    };
    let relation = (arguments.get("updates"))
        .map(|id| id.as_str().and_then(|id| NoteId::parse(id).ok()))
        .map(|id| id.map(Relation::Updates).ok_or("must be a note id"))
        .transpose()
        .map_err(|problem| invalid_argument("updates", problem))?;
    let mut scope = Scope::Global;
    if !repo_scope && !anchors.is_empty() {
        let mismatch = "anchors need scope repo";
        return Err(tool_error("scope_mismatch", mismatch, SCOPE_MISMATCH_FIX));
    }
    if repo_scope {
        let cwd = string_arg(arguments, "cwd")
            .map(Path::new)
            .filter(|cwd| cwd.is_absolute())
            .ok_or_else(|| invalid_argument("cwd", "must be an absolute path"))?;
        let context = RepoContext::discover(cwd);
        let (Some(key), Some(root)) = (context.key(), context.root()) else {
            let outside = format!(
                "{} is not inside a git repository with an origin remote",
                cwd.display()
            );
            return Err(tool_error("scope_mismatch", outside, SCOPE_MISMATCH_FIX));
        };
        scope = Scope::Repo(key.clone());
        let tree = WorkTree::new(root);
        for anchor in &mut anchors {
            *anchor = tree
                .pin(anchor)
                .map_err(|e| tool_error("anchor_not_found", e, ANCHOR_NOT_FOUND_FIX))?;
        }
    }
    let vault = open_vault()?;
    if let Some(Relation::Updates(target)) = &relation {
        vault.find(target.as_str()).map_err(|e| match e {
            VaultError::NotFound(_) => tool_error("note_not_found", e, NOTE_NOT_FOUND_FIX),
            e => vault_error(&e),
        })?;
    }
    let proposal = Proposal {
        id: NoteId::generate(),
        agent,
        session,
        source_verified,
        note_type,
        scope,
        title: string_arg(arguments, "title")
            .unwrap_or_default()
            .to_owned(),
        body: string_arg(arguments, "body").unwrap_or_default().to_owned(),
        anchors,
        relation,
        proposed_at: OffsetDateTime::now_utc(),
    };
    let written = inbox::propose(&vault, proposal).map_err(|e| match e {
        InboxError::Field { field, problem } => invalid_argument(
            if field == "session" {
                "session_id"
            } else {
                &field
            },
            &problem,
        ),
        InboxError::Vault(e) => vault_error(&e),
        e => tool_error("internal", e, INTERNAL_FIX),
    })?;
    Ok(json!({
        "proposal_id": written.id.as_str(),
        "inbox": format!("{}/{}", inbox::INBOX_DIR, written.file_name()),
        "session": written.session.as_str(),
        "source_verified": source_verified,
    }))
}

/// The miner's candidate lessons, newest first, each with up to
/// [`MINE_RECEIPTS`] receipts. Read only; every text is redacted again on
/// the way out, and the reply drops candidates from the tail to fit.
fn mine_candidates(ctx: &Ctx, arguments: &Value) -> Result<Value, Value> {
    let limit = match arguments.get("limit") {
        None => MINE_DEFAULT_LIMIT,
        Some(limit) => limit
            .as_u64()
            .filter(|n| (1..=MINE_MAX_LIMIT).contains(n))
            .ok_or_else(|| invalid_argument("limit", "must be an integer from 1 to 50"))?,
    };
    let conn = open(&ctx.archive_dir)?;
    let found = mine::corrections(&conn).map_err(|e| e.to_string())?;
    let mut lessons = mine::lessons(found);
    lessons.sort_by_key(|l| std::cmp::Reverse(l.latest_ms));
    lessons.truncate(usize::try_from(limit).unwrap_or(usize::MAX));
    let mut candidates = Vec::new();
    for lesson in &lessons {
        let mut receipts = Vec::new();
        for r in lesson.receipts.iter().take(MINE_RECEIPTS) {
            receipts.push(json!({
                "session": r.session, "agent": r.agent,
                "date": r.at_ms.and_then(crate::receipt::date),
                "correction": redact(&r.text)?,
                "before": r.preceding.as_deref().map(redact).transpose()?,
            }));
        }
        candidates.push(json!({
            "text": redact(&lesson.text)?,
            "conversations": lesson.conversations,
            "receipts": receipts,
        }));
    }
    let mut truncated = false;
    loop {
        let reply = json!({ "candidates": candidates, "truncated": truncated });
        if candidates.is_empty() || reply.to_string().chars().count() <= MINE_REPLY_CHARS {
            return Ok(reply);
        }
        candidates.pop();
        truncated = true;
    }
}

/// The archived session `id` names, as an archive or native session id. Read
/// only. A missing or unreadable archive counts as not found, so proposing
/// never depends on the archive.
fn archived_session(archive_dir: &Path, id: &str) -> Option<query::SessionOwner> {
    let conn = open(archive_dir).ok()?;
    query::find_session(&conn, id).ok().flatten()
}

/// `anchors` as `[{path, lines?}]`, unpinned. The agent never sends a hash.
fn proposed_anchors(anchors: Option<&Value>) -> Result<Vec<Anchor>, Value> {
    let items = match anchors {
        None | Some(Value::Null) => return Ok(Vec::new()),
        Some(Value::Array(items)) => items,
        Some(_) => return Err(invalid_argument("anchors", "must be a list")),
    };
    if items.len() > PROPOSE_MAX_ANCHORS {
        return Err(invalid_argument("anchors", "must be at most 8 anchors"));
    }
    let text = |item: &Value, key: &str| item.get(key).and_then(Value::as_str).map(str::to_owned);
    let mut parsed = Vec::new();
    for (i, item) in items.iter().enumerate() {
        let bad = |key: &str, e: InvalidValue| {
            invalid_argument(&format!("anchors[{i}].{key}"), &e.to_string())
        };
        parsed.push(Anchor {
            path: RelPath::parse(&text(item, "path").unwrap_or_default())
                .map_err(|e| bad("path", e))?,
            lines: (text(item, "lines")
                .as_deref()
                .map(LineRange::parse)
                .transpose())
            .map_err(|e| bad("lines", e))?,
            hash: None,
            commit: None,
        });
    }
    Ok(parsed)
}

fn redact(text: &str) -> Result<String, Value> {
    inbox::redact(text).map_err(|e| tool_error("internal", e, INTERNAL_FIX))
}
