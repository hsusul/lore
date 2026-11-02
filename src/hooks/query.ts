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
  { key: "repo:", example: "repo:lore", help: "Sessions in a repository, by name" },
  { key: "worktree:", example: "worktree:feat", help: "Git worktree id" },
  { key: "path:", example: "path:src/auth/", help: "Touched a file under this path" },
  { key: "has:patch", example: "has:patch", help: "Recorded a patch" },
  { key: "has:error", example: "has:error", help: "A tool call failed" },
  { key: "tool:", example: "tool:Bash", help: "Called this tool" },
  { key: "model:", example: "model:sonnet", help: "Ran on a matching model" },
  { key: "branch:", example: "branch:main", help: "Git branch seen in the session" },
  { key: "commit:", example: "commit:1a2b3c", help: "Commit sha prefix" },
  { key: "git-source:", example: "git-source:lore_captured", help: "Who saw the git evidence" },
  { key: "after:", example: "after:2026-07-01", help: "Started on or after a date" },
  { key: "before:", example: "before:2026-08-01", help: "Started before a date" },
];

const PREFIXES = FILTERS.filter((f) => f.key.endsWith(":")).map((f) => f.key);
const FLAGS = new Set(FILTERS.filter((f) => !f.key.endsWith(":")).map((f) => f.key));
/** Every key a bare `key:` can stand for, including `has:`, whose values are the flags. */
const KEYS = [...PREFIXES, "has:"];

export type QueryToken =
  | { kind: "space"; text: string }
  | { kind: "term"; text: string }
  | { kind: "filter"; text: string };

function isFilter(token: string): boolean {
  if (FLAGS.has(token)) return true;
  return PREFIXES.some((p) => token.startsWith(p));
}

/** A filter with a value, which the backend applies; a bare `agent:` is ignored there. */
function isComplete(token: string): boolean {
  return FLAGS.has(token) || PREFIXES.some((p) => token.startsWith(p) && token.length > p.length);
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

/** The complete filters in a query, as the search field shows them. */
export function queryFilters(raw: string): string[] {
  return tokenize(raw)
    .filter((t) => t.kind === "filter" && isComplete(t.text))
    .map((t) => t.text);
}

/** Whether the backend would search anything: a term or a complete filter, not a bare `key:`. */
export function isRunnable(raw: string): boolean {
  return tokenize(raw).some((t) => (t.kind === "term" && !KEYS.includes(t.text)) || (t.kind === "filter" && isComplete(t.text)));
}

/** The query without its filters, for "Clear Filters". */
export function withoutFilters(raw: string): string {
  return queryTerms(raw).join(" ");
}

/**
 * The search field's view of a query: complete filters followed by a space become
 * tokens, and everything else stays editable text. Joining them back moves the tokens
 * to the front, which the backend does not mind: filters are order-free.
 */
export function splitQuery(raw: string): { tokens: string[]; text: string } {
  const parts = tokenize(raw);
  const tokens: string[] = [];
  let text = "";
  for (let i = 0; i < parts.length; i++) {
    const part = parts[i];
    if (part.kind === "filter" && isComplete(part.text) && parts[i + 1]?.kind === "space") {
      tokens.push(part.text);
      i++;
    } else {
      text += part.text;
    }
  }
  return { tokens, text: text.trimStart() };
}

export function joinQuery(tokens: string[], text: string): string {
  return tokens.map((t) => `${t} `).join("") + text;
}

/** The filter being typed at the end of the text (`agent:co`), which the field offers values for. */
export function pendingFilter(text: string): { key: string; partial: string } | null {
  const last = text.match(/(\S+)$/)?.[1];
  const key = last && KEYS.find((k) => last.startsWith(k));
  return last && key ? { key, partial: last.slice(key.length) } : null;
}

/** The agent id when the whole query is one `agent:` filter, as the sidebar's agent rows write it. */
export function agentFilter(raw: string): string | null {
  const match = raw.trim().match(/^agent:(\S+)$/);
  return match ? match[1] : null;
}

/** The trailing space commits the filter as a token in the search field. */
export function agentQuery(agentId: string): string {
  return `agent:${agentId} `;
}
