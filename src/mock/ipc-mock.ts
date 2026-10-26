// Browser-only stand-in for the Tauri shell. `npm run dev` in a plain browser
// has no Rust backend, so every `invoke` and `listen` call would fail. This
// module answers them from the in-memory archive in `sample-data.ts`.
//
// It is reachable only through the dynamic import in `main.tsx`, which is
// guarded by `import.meta.env.DEV` and a runtime "not inside Tauri" check. A
// production build drops that branch, so this code never ships in the app.
//
// `createMockHandlers` is the handler table keyed by command name. A test in
// `ipc.test.ts` derives the command list from `ipc.ts` and fails if one has no
// handler here.

import { mockIPC } from "@tauri-apps/api/mocks";

import {
  HIGHLIGHT_END,
  HIGHLIGHT_START,
  type DetectedAgent,
  type FolderSummary,
  type MessagePage,
  type RepositorySummary,
  type ScanProgress,
  type SearchHit,
  type SearchPage,
  type SessionDetail,
  type SessionPage,
  type SessionSummary,
} from "../ipc";
import {
  SAMPLE_AGENTS,
  SAMPLE_BACKUP_SCHEDULE,
  SAMPLE_FOLDERS,
  SAMPLE_FOLDER_ASSIGNMENTS,
  SAMPLE_REPOSITORIES,
  SAMPLE_SETTINGS,
  buildSampleSessions,
  type SampleSession,
} from "./sample-data";

type Args = Record<string, unknown>;
export type MockHandler = (args: Args) => unknown;
export type MockEmit = (event: string, payload: unknown) => void;

export interface MockOptions {
  /** Clock anchor for the sample timestamps. Defaults to load time. */
  now?: number;
  /** Pause between `scan_progress` events during `rescan`. */
  scanStepMs?: number;
}

const DETAIL_MESSAGES = 200;
const MAX_LIMIT = 200;
const SAMPLE_ROOT_PICK = "/Users/you/sample-sessions";
const BACKUP_INTERVALS = ["off", "daily", "weekly"];

const asString = (value: unknown): string => (typeof value === "string" ? value : String(value ?? ""));
const asLimit = (value: unknown, fallback: number): number => {
  const n = typeof value === "number" && Number.isFinite(value) ? Math.floor(value) : fallback;
  return Math.min(Math.max(n, 1), MAX_LIMIT);
};
const asCursor = (value: unknown): string | null => (typeof value === "string" ? value : null);
const sleep = (ms: number) => new Promise<void>((resolve) => setTimeout(resolve, ms));

// ── Search ───────────────────────────────────────────────────────────────

const WORD = "[\\p{L}\\p{N}_]";

function queryTokens(query: string): string[] {
  return [...new Set(query.toLowerCase().match(/[\p{L}\p{N}_]+/gu) ?? [])];
}

function escapeRegex(text: string): string {
  return text.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
}

/** Matches whole words that start with any query token, as FTS5 prefix terms do. */
function tokenPattern(tokens: string[]): RegExp {
  const alternatives = [...tokens].sort((a, b) => b.length - a.length).map(escapeRegex);
  return new RegExp(`(?<!${WORD})(?:${alternatives.join("|")})${WORD}*`, "giu");
}

function countMatches(text: string, token: string): number {
  return text.match(tokenPattern([token]))?.length ?? 0;
}

/** A short window around the first match, with every match wrapped in the highlight markers. */
function makeSnippet(text: string, tokens: string[]): string {
  const flat = text.replace(/\s+/g, " ").trim();
  const first = flat.search(tokenPattern(tokens));
  const words = [...flat.matchAll(/\S+/g)];
  const at = words.findIndex((w) => (w.index ?? 0) + w[0].length > Math.max(first, 0));
  const lo = Math.max(0, at - 6);
  const hi = Math.min(words.length, at + 7);
  const slice = words.length
    ? flat.slice(words[lo].index ?? 0, (words[hi - 1].index ?? 0) + words[hi - 1][0].length)
    : flat;
  const marked = slice.replace(tokenPattern(tokens), (m) => `${HIGHLIGHT_START}${m}${HIGHLIGHT_END}`);
  return `${lo > 0 ? "…" : ""}${marked}${hi < words.length ? "…" : ""}`;
}

interface Doc {
  session: SampleSession;
  source_kind: string;
  source_id: string;
  field: string;
  text: string;
}

function* documents(sessions: SampleSession[]): Generator<Doc> {
  for (const session of sessions) {
    const title = session.summary.title;
    if (title) {
      yield { session, source_kind: "session", source_id: session.summary.id, field: "title", text: title };
    }
    for (const message of session.messages) {
      for (const part of message.parts) {
        if (!part.searchable) continue;
        if (part.text != null) {
          yield { session, source_kind: "message_part", source_id: `${message.id}:${part.ordinal}`, field: "text", text: part.text };
        } else if (part.content_json != null) {
          yield { session, source_kind: "message_part", source_id: `${message.id}:${part.ordinal}`, field: "content_json", text: part.content_json };
        }
      }
    }
    for (const [id, patch] of Object.entries(session.patches)) {
      yield { session, source_kind: "file_event", source_id: id, field: "patch", text: patch };
    }
  }
}

type SortOrder = "relevance" | "newest" | "oldest";

function runSearch(sessions: SampleSession[], query: string, sort: SortOrder): SearchHit[] {
  const tokens = queryTokens(query);
  if (tokens.length === 0) return [];
  const hits: SearchHit[] = [];
  for (const doc of documents(sessions)) {
    const lowered = doc.text.toLowerCase();
    const counts = tokens.map((token) => countMatches(lowered, token));
    if (counts.some((n) => n === 0)) continue; // every term must match
    const bonus = doc.field === "title" ? 2 : 0;
    hits.push({
      session_id: doc.session.summary.id,
      source_kind: doc.source_kind,
      source_id: doc.source_id,
      field: doc.field,
      snippet: makeSnippet(doc.text, tokens),
      rank: -(counts.reduce((a, b) => a + b, 0) + bonus),
      title: doc.session.summary.title,
      agent_id: doc.session.summary.agent_id,
      started_at: doc.session.summary.started_at,
    });
  }
  const byRecency = (a: SearchHit, b: SearchHit) => (b.started_at ?? 0) - (a.started_at ?? 0);
  return hits.sort((a, b) => {
    const primary =
      sort === "newest" ? byRecency(a, b) : sort === "oldest" ? -byRecency(a, b) : a.rank - b.rank || byRecency(a, b);
    return primary || a.source_id.localeCompare(b.source_id);
  });
}

// ── Paging ───────────────────────────────────────────────────────────────

/** Search cursors are bound to the query and sort that made them, like the real ones. */
function searchOffset(cursor: string | null, query: string, sort: SortOrder): number {
  if (cursor === null) return 0;
  const [cursorSort, cursorQuery, offset] = [
    cursor.slice(0, cursor.indexOf("|")),
    cursor.slice(cursor.indexOf("|") + 1, cursor.lastIndexOf("|")),
    Number(cursor.slice(cursor.lastIndexOf("|") + 1)),
  ];
  if (cursorSort !== sort || cursorQuery !== query || !Number.isInteger(offset) || offset < 0) {
    throw new Error("search cursor does not match this query and sort");
  }
  return offset;
}

/** Session cursors are keyset positions `(started_at, id)` in newest-first order. */
function sessionPage(sessions: SessionSummary[], limit: number, cursor: string | null): SessionPage {
  let rest = sessions;
  if (cursor !== null) {
    const split = cursor.indexOf("|");
    const startedAt = Number(cursor.slice(0, split));
    const id = cursor.slice(split + 1);
    if (split < 1 || !Number.isFinite(startedAt)) throw new Error("invalid session cursor");
    rest = sessions.filter((s) => {
      const t = s.started_at ?? 0;
      return t < startedAt || (t === startedAt && s.id > id);
    });
  }
  const page = rest.slice(0, limit);
  const last = page[page.length - 1];
  return {
    sessions: page,
    next_cursor: rest.length > limit && last ? `${last.started_at ?? 0}|${last.id}` : null,
  };
}

function messageOffset(cursor: string | null): number {
  if (cursor === null) return 0;
  const offset = Number(cursor.replace(/^m:/, ""));
  if (!Number.isInteger(offset) || offset < 0) throw new Error("invalid message cursor");
  return offset;
}

// ── Markdown export ──────────────────────────────────────────────────────

function toMarkdown(session: SampleSession): string {
  const { summary } = session;
  const lines = [`# ${summary.title ?? "(untitled session)"}`, "", `- Agent: ${summary.agent_id}`];
  if (summary.primary_model) lines.push(`- Model: ${summary.primary_model}`);
  lines.push(`- Messages: ${summary.message_count}`, "");
  for (const message of session.messages) {
    lines.push(`## ${message.role}`, "");
    for (const part of message.parts) {
      if (part.text != null) lines.push(part.text, "");
      else if (part.content_json != null) lines.push("```json", part.content_json, "```", "");
    }
  }
  return lines.join("\n");
}

function downloadText(filename: string, text: string): boolean {
  if (typeof document === "undefined") return false;
  const url = URL.createObjectURL(new Blob([text], { type: "text/markdown" }));
  const link = document.createElement("a");
  link.href = url;
  link.download = filename;
  link.click();
  URL.revokeObjectURL(url);
  return true;
}

// ── The handler table ────────────────────────────────────────────────────

/** Build a fresh in-memory archive and the handler for every IPC command. */
export function createMockHandlers(
  emit: MockEmit,
  { now = Date.now(), scanStepMs = 220 }: MockOptions = {},
): Record<string, MockHandler> {
  let sessions = buildSampleSessions(now);
  let folders = SAMPLE_FOLDERS.map((f) => ({ ...f }));
  let assignments = new Map(Object.entries(SAMPLE_FOLDER_ASSIGNMENTS));
  let settings = new Map(Object.entries(SAMPLE_SETTINGS));
  let backup = { ...SAMPLE_BACKUP_SCHEDULE };
  let nextFolder = 1;
  const customRoots = new Map<string, string[]>();

  const find = (id: unknown) => sessions.find((s) => s.summary.id === asString(id));
  const summaries = () => sessions.map((s) => s.summary);
  const inRepository = (repoId: string) =>
    sessions.filter((s) => s.segments.some((seg) => seg.repository_id === repoId)).map((s) => s.summary);
  const inFolder = (folderId: string) =>
    sessions.filter((s) => assignments.get(s.summary.id) === folderId).map((s) => s.summary);

  const folderSummary = (f: (typeof folders)[number]): FolderSummary => ({
    ...f,
    session_count: inFolder(f.id).length,
  });
  const repositorySummary = (r: (typeof SAMPLE_REPOSITORIES)[number]): RepositorySummary => ({
    ...r,
    session_count: inRepository(r.id).length,
  });
  const detectedAgent = (a: (typeof SAMPLE_AGENTS)[number]): DetectedAgent => {
    const custom = customRoots.get(a.id) ?? [];
    return {
      ...a,
      roots: [...a.roots, ...custom],
      custom_roots: custom,
      session_count: sessions.filter((s) => s.summary.agent_id === a.id).length,
    };
  };

  const detail = (s: SampleSession): SessionDetail => ({
    summary: s.summary,
    parse_note: s.parse_note,
    segments: s.segments,
    messages: s.messages.slice(0, DETAIL_MESSAGES),
    file_events: s.file_events,
    next_message_cursor: s.messages.length > DETAIL_MESSAGES ? `m:${DETAIL_MESSAGES}` : null,
  });

  const searchPageFor = (args: Args): SearchPage => {
    const query = asString(args.query);
    const sort = (["newest", "oldest"].includes(asString(args.sort)) ? args.sort : "relevance") as SortOrder;
    const limit = asLimit(args.limit, 50);
    const offset = searchOffset(asCursor(args.cursor), query, sort);
    const all = runSearch(sessions, query, sort);
    const hits = all.slice(offset, offset + limit);
    return {
      hits,
      next_cursor: offset + limit < all.length ? `${sort}|${query}|${offset + limit}` : null,
    };
  };

  const forget = (list: SampleSession[]) => ({
    blobs_removed: list.reduce((n, s) => n + Object.keys(s.patches).length, 0),
    source_paths: list.map((s) => s.source_path),
  });

  return {
    // Sessions and repositories
    list_detected_agents: () => SAMPLE_AGENTS.map(detectedAgent),
    list_sessions: (a) => summaries().slice(0, asLimit(a.limit, 50)),
    list_sessions_page: (a) => sessionPage(summaries(), asLimit(a.limit, 50), asCursor(a.cursor)),
    list_repositories: () =>
      SAMPLE_REPOSITORIES.map(repositorySummary).sort(
        (x, y) => y.session_count - x.session_count || x.display_name.localeCompare(y.display_name),
      ),
    list_repository_sessions: (a) => inRepository(asString(a.id)).slice(0, asLimit(a.limit, 50)),
    list_repository_sessions_page: (a) =>
      sessionPage(inRepository(asString(a.id)), asLimit(a.limit, 50), asCursor(a.cursor)),
    get_session: (a) => {
      const found = find(a.id);
      return found ? detail(found) : null;
    },
    list_session_messages_page: (a): MessagePage => {
      const found = find(a.id);
      if (!found) return { messages: [], next_cursor: null };
      const limit = asLimit(a.limit, 200);
      const offset = messageOffset(asCursor(a.cursor));
      return {
        messages: found.messages.slice(offset, offset + limit),
        next_cursor: offset + limit < found.messages.length ? `m:${offset + limit}` : null,
      };
    },
    get_git_snapshot: (a) => find(a.id)?.git ?? [],
    get_file_patch: (a) => {
      const id = asString(a.id);
      for (const s of sessions) if (id in s.patches) return s.patches[id];
      return null;
    },
    session_secret_count: (a) => find(a.id)?.secret_count ?? 0,
    relink_segment_repository: (a) => {
      const repo = SAMPLE_REPOSITORIES.find((r) => r.id === asString(a.repositoryId));
      if (!repo) throw new Error(`unknown repository ${asString(a.repositoryId)}`);
      for (const s of sessions) {
        const segment = s.segments.find((seg) => seg.id === asString(a.segmentId));
        if (segment) {
          segment.repository_id = repo.id;
          segment.resolution_confidence = "high";
          return null;
        }
      }
      throw new Error(`unknown segment ${asString(a.segmentId)}`);
    },

    // Agent roots
    add_agent_root: (a) => {
      const agent = asString(a.agentId);
      const path = asString(a.path);
      const current = customRoots.get(agent) ?? [];
      if (!current.includes(path)) customRoots.set(agent, [...current, path]);
      return null;
    },
    remove_agent_root: (a) => {
      const agent = asString(a.agentId);
      customRoots.set(
        agent,
        (customRoots.get(agent) ?? []).filter((p) => p !== asString(a.path)),
      );
      return null;
    },

    // Folders
    list_folders: () =>
      folders
        .map(folderSummary)
        .sort((x, y) => x.position - y.position || x.name.localeCompare(y.name)),
    create_folder: (a) => {
      const name = asString(a.name).trim().slice(0, 80);
      if (!name) throw new Error("A folder needs a name.");
      const folder = {
        id: `folder-new-${nextFolder++}`,
        name,
        position: folders.reduce((max, f) => Math.max(max, f.position), -1) + 1,
      };
      folders = [...folders, folder];
      return folderSummary(folder);
    },
    rename_folder: (a) => {
      const name = asString(a.name).trim().slice(0, 80);
      if (!name) throw new Error("A folder needs a name.");
      folders = folders.map((f) => (f.id === asString(a.id) ? { ...f, name } : f));
      return null;
    },
    delete_folder: (a) => {
      const id = asString(a.id);
      folders = folders.filter((f) => f.id !== id);
      assignments = new Map([...assignments].filter(([, folder]) => folder !== id));
      return null;
    },
    set_session_folder: (a) => {
      const sessionId = asString(a.sessionId);
      if (a.folderId == null) assignments.delete(sessionId);
      else assignments.set(sessionId, asString(a.folderId));
      return null;
    },
    list_folder_sessions_page: (a) =>
      sessionPage(inFolder(asString(a.id)), asLimit(a.limit, 50), asCursor(a.cursor)),

    // Search
    search: (a) => runSearch(sessions, asString(a.query), "relevance").slice(0, asLimit(a.limit, 50)),
    search_page: (a) => searchPageFor(a),

    // Export and forget
    export_session_markdown: (a) => {
      const found = find(a.id);
      return found ? toMarkdown(found) : null;
    },
    save_session_export: (a) => {
      const found = find(a.id);
      return found ? downloadText(`${found.summary.id}.md`, toMarkdown(found)) : false;
    },
    forget_session: (a) => {
      const found = find(a.id);
      sessions = sessions.filter((s) => s !== found);
      assignments.delete(asString(a.id));
      return forget(found ? [found] : []);
    },
    forget_everything: () => {
      const report = forget(sessions);
      sessions = [];
      folders = [];
      assignments = new Map();
      settings = new Map();
      return report;
    },

    // Scanning: the sample logs stay "on disk", so a rescan after "forget
    // everything" re-ingests them, and an unchanged archive is all skipped.
    rescan: async () => {
      const all = buildSampleSessions(now);
      const present = new Set(sessions.map((s) => s.summary.id));
      const chunk = Math.ceil(all.length / 4);
      const tally: ScanProgress = { discovered: all.length, ingested: 0, skipped: 0, failed: 0, enriched: 0, done: false };
      for (let i = 0; i < all.length; i += chunk) {
        for (const session of all.slice(i, i + chunk)) {
          if (present.has(session.summary.id)) {
            tally.skipped += 1;
          } else {
            sessions = [...sessions, session];
            tally.ingested += 1;
            if (session.segments.some((seg) => seg.repository_id !== null)) tally.enriched += 1;
          }
        }
        sessions.sort(
          (x, y) => (y.summary.started_at ?? 0) - (x.summary.started_at ?? 0) || x.summary.id.localeCompare(y.summary.id),
        );
        emit("scan_progress", { ...tally });
        await sleep(scanStepMs);
      }
      emit("scan_progress", { ...tally, done: true });
      const { discovered, ingested, skipped, failed, enriched } = tally;
      return { discovered, ingested, skipped, failed, enriched };
    },

    // Settings and backups
    get_setting: (a) => settings.get(asString(a.key)) ?? null,
    set_setting: (a) => {
      settings.set(asString(a.key), asString(a.valueJson));
      return null;
    },
    get_backup_schedule: () => ({ ...backup }),
    set_backup_schedule: (a) => {
      const interval = asString(a.interval);
      if (!BACKUP_INTERVALS.includes(interval)) throw new Error(`unknown backup interval "${interval}"`);
      backup = { interval, keep: Math.max(1, Math.floor(Number(a.keep) || 1)) };
      return null;
    },
    backup_now: () => null,

    // Usage and updates
    get_token_totals: () => ({
      total_input_tokens: sessions.reduce((n, s) => n + (s.summary.total_input_tokens ?? 0), 0),
      total_output_tokens: sessions.reduce((n, s) => n + (s.summary.total_output_tokens ?? 0), 0),
      total_cache_tokens: sessions.reduce((n, s) => n + (s.summary.total_cache_tokens ?? 0), 0),
      est_cost_usd: sessions.reduce((n, s) => n + (s.summary.est_cost_usd ?? 0), 0),
    }),
    check_for_updates: () => ({
      update_available: false,
      current_version: "0.0.0-sample",
      latest_version: null,
      release_notes: null,
      published_at: null,
    }),

    // The native folder picker (`@tauri-apps/plugin-dialog`).
    "plugin:dialog|open": () => SAMPLE_ROOT_PICK,
  };
}

// ── Installation ─────────────────────────────────────────────────────────

/**
 * Route `invoke` and `listen` to the sample archive. Events use a small bus of
 * our own: the stock `shouldMockEvents` unlisten matches on the wrong field and
 * leaves dead callbacks behind, which warns on every later emit.
 */
export function installIpcMock(options?: MockOptions): void {
  const listeners = new Map<string, Set<number>>();
  const emit: MockEmit = (event, payload) => {
    const internals = (window as unknown as {
      __TAURI_INTERNALS__: { runCallback: (id: number, data: unknown) => void };
    }).__TAURI_INTERNALS__;
    for (const id of listeners.get(event) ?? []) {
      internals.runCallback(id, { event, id, payload });
    }
  };
  const handlers = createMockHandlers(emit, options);

  mockIPC(async (cmd, payload) => {
    const args = (payload ?? {}) as Args;
    switch (cmd) {
      case "plugin:event|listen": {
        const id = args.handler as number;
        listeners.set(asString(args.event), (listeners.get(asString(args.event)) ?? new Set()).add(id));
        return id;
      }
      case "plugin:event|unlisten":
        listeners.get(asString(args.event))?.delete(args.eventId as number);
        return null;
      case "plugin:event|emit":
        emit(asString(args.event), args.payload);
        return null;
    }
    const handler = Object.prototype.hasOwnProperty.call(handlers, cmd) ? handlers[cmd] : undefined;
    if (!handler) throw new Error(`The browser sample-data mock has no handler for "${cmd}".`);
    return handler(args);
  });
}
