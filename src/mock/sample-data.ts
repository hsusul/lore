// One fixed in-memory archive for the browser-only dev mock (`npm run dev`
// outside the Tauri shell). Every record is typed with the generated
// `lore-ipc` DTOs, so a contract change breaks this file at typecheck time.
//
// The content is deterministic. Only the clock anchor moves: each timestamp is
// `now` minus a fixed offset, so "2h ago" stays "2h ago" whenever the page loads.
//
// This module is reachable only from `ipc-mock.ts`, which `main.tsx` imports
// behind `import.meta.env.DEV`, so none of it ships in a production build.

import type {
  BackupScheduleDto,
  DetectedAgent,
  FileEventDto,
  FolderSummary,
  GitObservationDto,
  MessageDto,
  MessagePartDto,
  RepositorySummary,
  SegmentDto,
  SessionSummary,
} from "../ipc";

const MIN = 60_000;
const HOUR = 60 * MIN;
const DAY = 24 * HOUR;

/** Everything the mock knows about one archived session. */
export interface SampleSession {
  summary: SessionSummary;
  parse_note: string | null;
  segments: SegmentDto[];
  /** The full timeline; `get_session` returns only the first page of it. */
  messages: MessageDto[];
  file_events: FileEventDto[];
  git: GitObservationDto[];
  /** Recorded patch text keyed by file event id. */
  patches: Record<string, string>;
  secret_count: number;
  /** The original agent log, reported by `forget_session` (Lore never deletes it). */
  source_path: string;
}

export type SampleRepository = Omit<RepositorySummary, "session_count">;
export type SampleFolder = Omit<FolderSummary, "session_count">;

export const SAMPLE_REPOSITORIES: SampleRepository[] = [
  {
    id: "repo-acme-storefront",
    display_name: "acme-storefront",
    identity_confidence: "confirmed",
    primary_path: "/Users/you/code/acme-storefront",
    is_missing: false,
    worktree_count: 2,
  },
  {
    id: "repo-pricing-service",
    display_name: "pricing-service",
    identity_confidence: "high",
    primary_path: "/Users/you/code/pricing-service",
    is_missing: false,
    worktree_count: 1,
  },
  {
    id: "repo-notes-site",
    display_name: "notes-site",
    identity_confidence: "medium",
    primary_path: "/Users/you/code/notes-site",
    is_missing: false,
    worktree_count: 1,
  },
  {
    id: "repo-legacy-admin",
    display_name: "legacy-admin",
    identity_confidence: "low",
    primary_path: "/Users/you/code/legacy-admin",
    is_missing: true,
    worktree_count: 0,
  },
];

/** Folders and which sessions start filed in them. */
export const SAMPLE_FOLDERS: SampleFolder[] = [
  { id: "folder-billing", name: "Billing rework", position: 0 },
  { id: "folder-reading", name: "Worth re-reading", position: 1 },
];
export const SAMPLE_FOLDER_ASSIGNMENTS: Record<string, string> = {
  "ses-claude-rounding": "folder-billing",
  "ses-codex-reducer": "folder-billing",
  "ses-claude-retry": "folder-billing",
  "ses-claude-webhook": "folder-reading",
};

export const SAMPLE_AGENTS: Omit<DetectedAgent, "session_count">[] = [
  {
    id: "claude-code",
    display_name: "Claude Code",
    installed: true,
    version: "2.1.0",
    roots: ["/Users/you/.claude/projects"],
    custom_roots: [],
  },
  {
    id: "codex",
    display_name: "Codex",
    installed: true,
    version: "0.4.2",
    roots: ["/Users/you/.codex/sessions"],
    custom_roots: [],
  },
];

export const SAMPLE_SETTINGS: Record<string, string> = {
  theme: JSON.stringify("system"),
  "ui.density": JSON.stringify("comfortable"),
};

export const SAMPLE_BACKUP_SCHEDULE: BackupScheduleDto = { interval: "weekly", keep: 7 };

// ── Builders ─────────────────────────────────────────────────────────────

type Agent = "claude-code" | "codex";
type Turn = readonly [role: "user" | "assistant", text: string, tool?: ToolCall];
interface ToolCall {
  name: string;
  input: Record<string, unknown>;
}
interface FileSpec {
  path: string;
  kind: string;
  added: number;
  removed: number;
  patch?: string;
}
interface SessionSpec {
  id: string;
  agent: Agent;
  title: string;
  ago: number;
  minutes: number;
  repo: string | null;
  turns: readonly Turn[];
  files?: readonly FileSpec[];
  status?: "ok" | "partial" | "failed";
  note?: string;
  secrets?: number;
}

const MODEL: Record<Agent, string> = {
  "claude-code": "claude-sonnet-4-5",
  codex: "gpt-5-codex",
};
const PROVIDER: Record<Agent, string> = { "claude-code": "anthropic", codex: "openai" };
const BRANCH: Record<string, string> = {
  "repo-acme-storefront": "feature/checkout-totals",
  "repo-pricing-service": "main",
  "repo-notes-site": "docs/quick-start",
  "repo-legacy-admin": "master",
};

function fakeSha(seed: string): string {
  let h = 2166136261;
  let out = "";
  for (let round = 0; out.length < 40; round += 1) {
    for (let i = 0; i < seed.length; i += 1) {
      h = Math.imul(h ^ (seed.charCodeAt(i) + round), 16777619) >>> 0;
    }
    out += h.toString(16).padStart(8, "0");
  }
  return out.slice(0, 40);
}

function toMessage(sessionId: string, seq: number, turn: Turn, ts: number, agent: Agent): MessageDto {
  const [role, text, tool] = turn;
  const parts: MessagePartDto[] = [
    { ordinal: 0, kind: "text", text, content_json: null, searchable: true },
  ];
  if (tool) {
    parts.push({
      ordinal: 1,
      kind: "tool_use",
      text: null,
      content_json: JSON.stringify(tool),
      searchable: true,
    });
  }
  return {
    id: `${sessionId}-m${seq}`,
    seq,
    role,
    event_kind: "message",
    is_sidechain: false,
    ts,
    model: role === "assistant" ? MODEL[agent] : null,
    parts,
  };
}

function build(spec: SessionSpec, now: number, repos: SampleRepository[]): SampleSession {
  const started = now - spec.ago;
  const ended = started + spec.minutes * MIN;
  const step = (spec.minutes * MIN) / Math.max(spec.turns.length, 1);
  const messages = spec.turns.map((turn, seq) =>
    toMessage(spec.id, seq, turn, Math.round(started + seq * step), spec.agent),
  );
  const toolCalls = messages.filter((m) => m.parts.some((p) => p.kind === "tool_use")).length;
  const repo = repos.find((r) => r.id === spec.repo) ?? null;

  const fileEvents: FileEventDto[] = [];
  const patches: Record<string, string> = {};
  (spec.files ?? []).forEach((file, index) => {
    const id = `${spec.id}-f${index + 1}`;
    fileEvents.push({
      id,
      path: file.path,
      change_kind: file.kind,
      old_path: null,
      lines_added: file.added,
      lines_removed: file.removed,
      source: spec.agent === "codex" ? "agent_tool_input" : "agent_patch",
      has_patch: file.patch !== undefined,
    });
    if (file.patch !== undefined) patches[id] = file.patch;
  });

  const inputTokens = messages.length * 1_850;
  const outputTokens = messages.length * 540;
  const summary: SessionSummary = {
    id: spec.id,
    agent_id: spec.agent,
    title: spec.title,
    started_at: started,
    ended_at: ended,
    message_count: messages.length,
    tool_call_count: toolCalls,
    primary_model: MODEL[spec.agent],
    parse_status: spec.status ?? "ok",
    total_input_tokens: inputTokens,
    total_output_tokens: outputTokens,
    total_cache_tokens: inputTokens * 3,
    est_cost_usd: Number(((inputTokens * 3 + outputTokens * 15) / 1_000_000).toFixed(4)),
  };

  const segments: SegmentDto[] = [
    {
      id: `${spec.id}-seg1`,
      seq_start: 0,
      seq_end: Math.max(messages.length - 1, 0),
      cwd: repo?.primary_path ?? null,
      model: MODEL[spec.agent],
      provider: PROVIDER[spec.agent],
      repository_id: repo?.id ?? null,
      resolution_confidence: repo ? "high" : "unresolved",
    },
  ];

  const git: GitObservationDto[] = repo
    ? [
        {
          segment_id: segments[0].id,
          source: "agent_recorded",
          event_ts: started,
          observed_at: started,
          temporal_confidence: "exact_event",
          branch: BRANCH[repo.id] ?? "main",
          commit_sha: null,
          remote_url_norm: `github.com/acme/${repo.display_name}`,
          is_dirty: null,
          commit_exists: null,
        },
        {
          segment_id: segments[0].id,
          source: "lore_captured",
          event_ts: null,
          observed_at: ended + 2 * MIN,
          temporal_confidence: "near_event",
          branch: BRANCH[repo.id] ?? "main",
          commit_sha: fakeSha(spec.id),
          remote_url_norm: `github.com/acme/${repo.display_name}`,
          is_dirty: spec.files !== undefined,
          commit_exists: null,
        },
        {
          segment_id: segments[0].id,
          source: "lore_reverified",
          event_ts: null,
          observed_at: ended + HOUR,
          temporal_confidence: "retrospective",
          branch: BRANCH[repo.id] ?? "main",
          commit_sha: fakeSha(spec.id),
          remote_url_norm: `github.com/acme/${repo.display_name}`,
          is_dirty: false,
          commit_exists: !repo.is_missing,
        },
      ]
    : [];

  return {
    summary,
    parse_note: spec.note ?? null,
    segments,
    messages,
    file_events: fileEvents,
    git,
    patches,
    secret_count: spec.secrets ?? 0,
    source_path:
      spec.agent === "claude-code"
        ? `/Users/you/.claude/projects/${spec.repo ?? "scratch"}/${spec.id}.jsonl`
        : `/Users/you/.codex/sessions/${spec.id}.jsonl`,
  };
}

// ── Featured sessions (hand-written) ─────────────────────────────────────

const MONEY_PATCH = `--- a/src/lib/money.ts
+++ b/src/lib/money.ts
@@ -1,6 +1,9 @@
 const USD = new Intl.NumberFormat("en-US", { style: "currency", currency: "USD" });

 /** Format integer cents as a localized price. */
 export function formatPrice(cents: number): string {
-  return USD.format(cents / 100);
+  // Round half away from zero so 0.5 cents never renders as a free item.
+  const rounded = Math.sign(cents) * Math.round(Math.abs(cents));
+  return USD.format(rounded / 100);
 }
`;

const MONEY_TEST_PATCH = `--- a/src/lib/money.test.ts
+++ b/src/lib/money.test.ts
@@ -4,3 +4,8 @@ describe("formatPrice", () => {
   it("formats whole dollars", () => {
     expect(formatPrice(1200)).toBe("$12.00");
   });
+
+  it("rounds fractional cents away from zero", () => {
+    expect(formatPrice(0.5)).toBe("$0.01");
+    expect(formatPrice(-0.5)).toBe("-$0.01");
+  });
 });
`;

const RETRY_PATCH = `--- a/src/client.rs
+++ b/src/client.rs
@@ -18,9 +18,22 @@ impl PricingClient {
-    pub async fn quote(&self, cart: &Cart) -> Result<Quote, Error> {
-        self.http.post(&self.url).json(cart).send().await?.json().await
-    }
+    pub async fn quote(&self, cart: &Cart) -> Result<Quote, Error> {
+        let mut delay = Duration::from_millis(100);
+        for attempt in 0..MAX_ATTEMPTS {
+            match self.http.post(&self.url).json(cart).send().await {
+                Ok(resp) if resp.status().is_success() => return Ok(resp.json().await?),
+                Ok(resp) if !resp.status().is_server_error() => return Err(Error::Rejected(resp.status())),
+                _ if attempt + 1 == MAX_ATTEMPTS => break,
+                _ => tokio::time::sleep(delay).await,
+            }
+            delay *= 2;
+        }
+        Err(Error::Unavailable)
+    }
 }
`;

const REDUCER_PATCH = `--- a/src/checkout/Cart.tsx
+++ b/src/checkout/Cart.tsx
@@ -1,12 +1,24 @@
-import { useMemo, useState } from "react";
+import { useMemo, useReducer } from "react";

 import { formatPrice } from "../lib/money";
+import { cartReducer } from "./cartReducer";

-const [lines, setLines] = useState(SAMPLE);
+const [lines, dispatch] = useReducer(cartReducer, SAMPLE);
`;

const FEATURED: SessionSpec[] = [
  {
    id: "ses-claude-rounding",
    agent: "claude-code",
    title: "Fix rounding bug in checkout totals",
    ago: 2 * HOUR,
    minutes: 38,
    repo: "repo-acme-storefront",
    files: [
      { path: "src/lib/money.ts", kind: "edit", added: 3, removed: 1, patch: MONEY_PATCH },
      { path: "src/lib/money.test.ts", kind: "edit", added: 5, removed: 0, patch: MONEY_TEST_PATCH },
    ],
    turns: [
      ["user", "The checkout total is off by a cent when a coupon applies. Can you find out why?"],
      [
        "assistant",
        "I'll start with how the cart turns cents into a displayed price.",
        { name: "Read", input: { file_path: "src/lib/money.ts" } },
      ],
      [
        "assistant",
        "Found it. formatPrice divides raw cents by 100 without rounding, so a discounted line of 0.5 cents renders as a free item and the total drifts.",
      ],
      ["user", "Fix it, and add a test that would have caught it."],
      [
        "assistant",
        "I rounded half away from zero before formatting and added a regression test for positive and negative fractional cents.",
        { name: "Edit", input: { file_path: "src/lib/money.ts" } },
      ],
      [
        "assistant",
        "The unit tests pass and the coupon example now totals correctly.",
        { name: "Bash", input: { command: "npm test -- money" } },
      ],
    ],
  },
  {
    id: "ses-claude-retry",
    agent: "claude-code",
    title: "Add retry with backoff to the pricing client",
    ago: 1 * DAY + 3 * HOUR,
    minutes: 52,
    repo: "repo-pricing-service",
    secrets: 1,
    files: [{ path: "src/client.rs", kind: "edit", added: 13, removed: 3, patch: RETRY_PATCH }],
    turns: [
      ["user", "Quotes fail whenever the pricing service restarts. Add retries with exponential backoff."],
      [
        "assistant",
        "I'll retry only on connection errors and 5xx responses, doubling the delay from 100ms, and give up after four attempts.",
        { name: "Edit", input: { file_path: "src/client.rs" } },
      ],
      ["user", "Does it leak the API key into the retry logs?"],
      [
        "assistant",
        "No. The request URL is logged without its query string, and Lore flagged one key in the pasted config, which is redacted here as [REDACTED].",
      ],
      [
        "assistant",
        "All client tests pass, including one that simulates a restart mid-request.",
        { name: "Bash", input: { command: "cargo test -p pricing client" } },
      ],
    ],
  },
  {
    id: "ses-codex-reducer",
    agent: "codex",
    title: "Migrate cart state to a reducer",
    ago: 2 * DAY + 5 * HOUR,
    minutes: 27,
    repo: "repo-acme-storefront",
    files: [
      { path: "src/checkout/Cart.tsx", kind: "edit", added: 8, removed: 4, patch: REDUCER_PATCH },
      { path: "src/checkout/cartReducer.ts", kind: "create", added: 34, removed: 0 },
    ],
    turns: [
      ["user", "Cart.tsx has five useState calls that must stay in sync. Move them into a reducer."],
      [
        "assistant",
        "I'll model add, remove, and set-quantity as actions so the cart can only move between valid states.",
        { name: "apply_patch", input: { path: "src/checkout/cartReducer.ts" } },
      ],
      [
        "assistant",
        "The component now dispatches actions. I kept the exported props unchanged so callers do not move.",
        { name: "apply_patch", input: { path: "src/checkout/Cart.tsx" } },
      ],
      ["user", "Run the checkout tests."],
      ["assistant", "Checkout tests pass: 14 passed, 0 failed.", { name: "shell", input: { command: "npm test -- checkout" } }],
    ],
  },
  {
    id: "ses-codex-flaky",
    agent: "codex",
    title: "Investigate flaky checkout test",
    ago: 3 * DAY + 2 * HOUR,
    minutes: 44,
    repo: "repo-acme-storefront",
    turns: [
      ["user", "The checkout coupon test fails about one run in ten on CI. Why?"],
      [
        "assistant",
        "The test reads the clock twice, so a run that crosses a second boundary expires the coupon mid-test.",
        { name: "shell", input: { command: "npm test -- coupon --repeat 50" } },
      ],
      ["user", "Freeze the clock in that test."],
      ["assistant", "Done. The test now uses fake timers, and 200 repeated runs passed."],
    ],
  },
  {
    id: "ses-claude-webhook",
    agent: "claude-code",
    title: "Explain the webhook signature check",
    ago: 4 * DAY,
    minutes: 12,
    repo: "repo-pricing-service",
    turns: [
      ["user", "Walk me through how the webhook signature check works before I touch it."],
      [
        "assistant",
        "The sender signs the raw request body with a shared secret using HMAC-SHA256 and puts the hex digest in a header. The handler recomputes the digest and compares the two in constant time.",
      ],
      ["user", "Why constant time?"],
      [
        "assistant",
        "A normal string comparison returns at the first differing byte, so response timing would reveal how much of a forged signature was correct.",
      ],
    ],
  },
  {
    id: "ses-claude-quickstart",
    agent: "claude-code",
    title: "Write a README quick start",
    ago: 5 * DAY + 6 * HOUR,
    minutes: 19,
    repo: "repo-notes-site",
    files: [{ path: "README.md", kind: "edit", added: 22, removed: 2 }],
    turns: [
      ["user", "Our README has no quick start. Draft one that gets a new contributor running in five minutes."],
      [
        "assistant",
        "I added install, dev server, and test commands in the order a new contributor needs them, and a short troubleshooting list.",
        { name: "Edit", input: { file_path: "README.md" } },
      ],
    ],
  },
  {
    id: "ses-codex-release-notes",
    agent: "codex",
    title: "Draft release notes for 2.4",
    ago: 6 * DAY + 1 * HOUR,
    minutes: 16,
    repo: "repo-acme-storefront",
    turns: [
      ["user", "Summarize the merged pull requests since 2.3 into release notes."],
      [
        "assistant",
        "Highlights: coupon rounding fix, cart reducer migration, and a faster checkout page. I grouped the rest under maintenance.",
      ],
    ],
  },
  {
    id: "ses-claude-csv",
    agent: "claude-code",
    title: "Port admin export to CSV streaming",
    ago: 9 * DAY,
    minutes: 61,
    repo: "repo-legacy-admin",
    status: "partial",
    note: "Skipped 3 records Lore could not safely normalize.",
    turns: [
      ["user", "The admin export loads every row into memory. Stream it as CSV instead."],
      [
        "assistant",
        "I replaced the array build with a row iterator that writes each line as it arrives, so memory stays flat.",
        { name: "Edit", input: { file_path: "admin/export.py" } },
      ],
      ["user", "Does it still escape commas inside names?"],
      ["assistant", "Yes, the csv writer quotes any field containing a comma, a quote, or a newline."],
    ],
  },
  {
    id: "ses-codex-deploy",
    agent: "codex",
    title: "Triage failing deploy pipeline",
    ago: 12 * DAY,
    minutes: 8,
    repo: "repo-pricing-service",
    status: "failed",
    note: "Unrecognized log format; only the first records were readable.",
    turns: [
      ["user", "The deploy job fails at the migration step. What changed?"],
      ["assistant", "The migration lock timed out because a long report query held a table lock."],
    ],
  },
  {
    id: "ses-claude-regex",
    agent: "claude-code",
    title: "Quick question about regex lookbehind",
    ago: 14 * DAY,
    minutes: 4,
    repo: null,
    turns: [
      ["user", "Does JavaScript support lookbehind in regular expressions?"],
      [
        "assistant",
        "Yes, in every current browser and in Node 10 and later. Variable-length lookbehind works too, unlike in many other engines.",
      ],
    ],
  },
  {
    id: "ses-codex-rename",
    agent: "codex",
    title: "Rename the sku field across the codebase",
    ago: 16 * DAY,
    minutes: 23,
    repo: null,
    turns: [
      ["user", "Rename sku to product_code everywhere, including the database column."],
      [
        "assistant",
        "I renamed it in the types, the API handlers, and the fixtures, and wrote a migration that renames the column without dropping data.",
        { name: "shell", input: { command: "git grep -l sku" } },
      ],
    ],
  },
];

// ── A long timeline, to exercise message paging ──────────────────────────

const LONG_STEPS = [
  "backfill the orders table in batches of 5,000 rows",
  "verify row counts against the replica",
  "swap the read path to the new column",
  "drop the shadow index once traffic is clean",
];

function longMigration(): SessionSpec {
  const turns: Turn[] = [
    ["user", "Plan the zero-downtime migration for the orders table, then walk me through it step by step."],
  ];
  for (let step = 1; step <= 250; step += 1) {
    const action = LONG_STEPS[(step - 1) % LONG_STEPS.length];
    turns.push(
      step % 2 === 1
        ? ["assistant", `Step ${step}: ${action}. Nothing blocks writes while this runs.`]
        : ["user", `Step ${step - 1} looks good. Continue.`],
    );
  }
  return {
    id: "ses-claude-migration",
    agent: "claude-code",
    title: "Zero-downtime migration plan for the orders table",
    ago: 20 * DAY,
    minutes: 190,
    repo: "repo-pricing-service",
    turns,
  };
}

// ── Filler sessions, so the list and search both need a second page ──────

const VERBS: [string, string][] = [
  ["tidy", "tidied"],
  ["review", "reviewed"],
  ["profile", "profiled"],
  ["document", "documented"],
  ["simplify", "simplified"],
];
const NOUNS = [
  "retry helper",
  "feature flag loader",
  "invoice template",
  "session cookie",
  "image resizer",
  "search index",
  "audit log",
  "rate limiter",
  "config parser",
  "email digest",
  "price formatter",
  "login form",
];
const FILLER_REPOS = [
  "repo-acme-storefront",
  "repo-pricing-service",
  "repo-notes-site",
  null,
];
const FILLER_COUNT = 110;

function fillerSessions(): SessionSpec[] {
  const out: SessionSpec[] = [];
  for (let i = 0; i < FILLER_COUNT; i += 1) {
    const [verb, past] = VERBS[i % VERBS.length];
    const noun = NOUNS[(i * 7) % NOUNS.length];
    const agent: Agent = i % 3 === 0 ? "codex" : "claude-code";
    out.push({
      id: `ses-sample-${String(i + 1).padStart(3, "0")}`,
      agent,
      title: `${verb[0].toUpperCase()}${verb.slice(1)} the ${noun}`,
      ago: 7 * DAY + i * 11 * HOUR + (i % 5) * 17 * MIN,
      minutes: 5 + (i % 9) * 3,
      repo: FILLER_REPOS[i % FILLER_REPOS.length],
      turns: [
        ["user", `Can you ${verb} the ${noun}? It keeps coming up in review.`],
        ["assistant", `Done. I ${past} the ${noun} and ran the tests; everything passes.`],
      ],
    });
  }
  return out;
}

/** The whole archive, newest first, anchored to `now`. */
export function buildSampleSessions(now: number = Date.now()): SampleSession[] {
  return [...FEATURED, longMigration(), ...fillerSessions()]
    .map((spec) => build(spec, now, SAMPLE_REPOSITORIES))
    .sort(
      (a, b) =>
        (b.summary.started_at ?? 0) - (a.summary.started_at ?? 0) ||
        a.summary.id.localeCompare(b.summary.id),
    );
}
