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

const INSTRUCTIONS: &str = "Lore archives past Claude Code and Codex sessions on this machine. \
Use search_sessions to find earlier work, decisions, or errors, then read_session to read one. \
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
    call: fn(&Path, &Value) -> Result<Value, String>,
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
        Err(message) => json!({
            "content": [{ "type": "text", "text": message }],
            "isError": true,
        }),
    })
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

fn search_sessions(archive_dir: &Path, arguments: &Value) -> Result<Value, String> {
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

fn read_session(archive_dir: &Path, arguments: &Value) -> Result<Value, String> {
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
