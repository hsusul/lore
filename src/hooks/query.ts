/**
 * The search-query grammar as the UI sees it. Mirrors `parse_query` in
 * `crates/lore-core/src/search.rs`: whitespace-separated tokens, where a known
 * `key:value` prefix (or a `has:` flag) is a filter and anything else is a term.
 * A query of filters alone lists the matching sessions, newest first.
 */

export interface FilterSpec {
  key: string;
  example: string;
  help: string;
}

export const FILTERS: readonly FilterSpec[] = [
  { key: "agent:", example: "agent:codex", help: "Sessions from one agent" },
  { key: "path:", example: "path:src/auth/", help: "Touched a file under this path" },
  { key: "tool:", example: "tool:Bash", help: "Called this tool" },
  { key: "model:", example: "model:sonnet", help: "Ran on a matching model" },
  { key: "branch:", example: "branch:main", help: "Git branch seen in the session" },
  { key: "commit:", example: "commit:1a2b3c", help: "Commit sha prefix" },
  { key: "worktree:", example: "worktree:feat", help: "Git worktree id" },
  { key: "git-source:", example: "git-source:lore_captured", help: "Who saw the git evidence" },
  { key: "after:", example: "after:2026-07-01", help: "Started on or after a date" },
  { key: "before:", example: "before:2026-08-01", help: "Started before a date" },
  { key: "has:error", example: "has:error", help: "A tool call failed" },
  { key: "has:patch", example: "has:patch", help: "Recorded a patch" },
];

const PREFIXES = FILTERS.filter((f) => f.key.endsWith(":")).map((f) => f.key);
const FLAGS = new Set(FILTERS.filter((f) => !f.key.endsWith(":")).map((f) => f.key));
// `repo:` is real but takes an opaque repository id, so the UI scopes by
// repository through the scope picker instead of advertising it here.
const HIDDEN_PREFIXES = ["repo:"];

export type QueryToken =
  | { kind: "space"; text: string }
  | { kind: "term"; text: string }
  | { kind: "filter"; text: string };

function isFilter(token: string): boolean {
  if (FLAGS.has(token)) return true;
  return [...PREFIXES, ...HIDDEN_PREFIXES].some((p) => token.startsWith(p));
}

/** Split a raw query into tokens, keeping whitespace so it can be re-rendered verbatim. */
export function tokenize(raw: string): QueryToken[] {
  const tokens: QueryToken[] = [];
  for (const part of raw.split(/(\s+)/)) {
    if (part === "") continue;
    if (/^\s+$/.test(part)) tokens.push({ kind: "space", text: part });
    else tokens.push({ kind: isFilter(part) ? "filter" : "term", text: part });
  }
  return tokens;
}

/** The plain search terms in a query, without filters. */
export function queryTerms(raw: string): string[] {
  return tokenize(raw)
    .filter((t) => t.kind === "term")
    .map((t) => t.text);
}

/** The agent id when the whole query is one `agent:` filter, as the sidebar's agent rows write it. */
export function agentFilter(raw: string): string | null {
  const match = raw.trim().match(/^agent:(\S+)$/);
  return match ? match[1] : null;
}

export function agentQuery(agentId: string): string {
  return `agent:${agentId}`;
}
