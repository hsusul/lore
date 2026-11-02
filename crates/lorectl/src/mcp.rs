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
//! them, so `recall_memory` passes every title and excerpt through
//! [`inbox::redact`] on the way out.
//!
//! ## Failure stays inside the protocol
//!
//! Stdout belongs to the protocol, so nothing else is ever printed to it. A
//! tool that fails (no archive yet, an unknown session) answers with an
//! `isError` result the agent can read, rather than ending the server: an
//! archive that appears later is picked up on the next call, because each call
//! opens the database afresh.

use std::io::{BufRead, Write};
use std::path::Path;

use lore_core::paths::ARCHIVE_DB_FILENAME;
use lore_core::search::{self, SortOrder, HIGHLIGHT_END, HIGHLIGHT_START};
use lore_core::{query, storage};
use lore_memory::vault::{self, Vault, VaultError};
use lore_memory::{inbox, recall};
use rusqlite::Connection;
use serde_json::{json, Value};

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

const VAULT_CONFIG_FIX: &str = "Set LORE_VAULT_DIR to an absolute path in this MCP server's env.";
const VAULT_BUSY_FIX: &str =
    "Another Lore process holds the vault lock. Retry once in a few seconds.";
const INTERNAL_FIX: &str = "Nothing was written. Tell the user. `lorectl mem reindex` may fix it.";

const INSTRUCTIONS: &str = "Lore archives past Claude Code and Codex sessions on this machine. \
Use search_sessions to find earlier work, decisions, or errors, then read_session to read one. \
Use recall_memory with your working directory to get accepted notes about the repo you are in. \
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
    call: fn(&Path, &Value) -> Result<Value, Value>,
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
            "query": { "type": "string", "description": "Words the notes should match." },
            "limit": {
                "type": "integer",
                "minimum": 1,
                "maximum": RECALL_MAX_LIMIT,
                "default": RECALL_DEFAULT_LIMIT
            }
        },
        "required": ["cwd", "query"]
    })
}

/// Serve MCP on stdin/stdout until stdin closes.
pub fn run(invocation: &Invocation) -> Result<u8, CliError> {
    let archive_dir = invocation.archive_dir()?;
    let stdin = std::io::stdin().lock();
    let mut stdout = std::io::stdout().lock();
    for line in stdin.lines() {
        let Ok(line) = line else { break };
        if line.trim().is_empty() {
            continue;
        }
        if let Some(response) = handle(&archive_dir, &line) {
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
fn handle(archive_dir: &Path, line: &str) -> Option<Value> {
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
        "initialize" => Ok(initialize(&params)),
        "ping" => Ok(json!({})),
        "tools/list" => Ok(json!({ "tools": TOOLS.iter().map(describe).collect::<Vec<_>>() })),
        "tools/call" => call_tool(archive_dir, &params),
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

fn call_tool(archive_dir: &Path, params: &Value) -> Result<Value, (i64, String)> {
    let name = params
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let tool = TOOLS
        .iter()
        .find(|t| t.name == name)
        .ok_or_else(|| (INVALID_PARAMS, format!("unknown tool `{name}`")))?;
    let arguments = params.get("arguments").cloned().unwrap_or(json!({}));
    Ok(match (tool.call)(archive_dir, &arguments) {
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

fn search_sessions(archive_dir: &Path, arguments: &Value) -> Result<Value, Value> {
    let query_text = string_arg(arguments, "query").ok_or("`query` is required")?;
    let sort = SortOrder::parse(string_arg(arguments, "sort"));
    let conn = open(archive_dir)?;
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

fn read_session(archive_dir: &Path, arguments: &Value) -> Result<Value, Value> {
    let session_id = string_arg(arguments, "session_id").ok_or("`session_id` is required")?;
    let after_id = match string_arg(arguments, "cursor") {
        Some(cursor) => Some(cursor.parse::<i64>().map_err(|_| "`cursor` is not valid")?),
        None => None,
    };
    let conn = open(archive_dir)?;
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
fn recall_memory(_archive_dir: &Path, arguments: &Value) -> Result<Value, Value> {
    let cwd = string_arg(arguments, "cwd")
        .map(Path::new)
        .filter(|cwd| cwd.is_absolute())
        .ok_or_else(|| invalid_argument("cwd", "must be an absolute path"))?;
    let query_text = string_arg(arguments, "query")
        .filter(|q| q.chars().any(char::is_alphanumeric))
        .ok_or_else(|| invalid_argument("query", "must contain at least one word"))?;
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
    let redact =
        |text: &str| inbox::redact(text).map_err(|e| tool_error("internal", e, INTERNAL_FIX));
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
