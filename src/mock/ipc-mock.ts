// Dev-only in-memory stand-in for the Tauri commands, so `npm run dev` in a
// plain browser shows a realistic workbench. Loaded only by src/ipc.ts behind
// `import.meta.env.DEV` (and never in tests unless a test imports it), so it is
// not part of production builds. Export names and signatures mirror src/ipc.ts.

import type {
  ActivityDto,
  ContinueTaskRequest,
  CreateTaskRequest,
  DirEntryDto,
  FileContentDto,
  MergeResultDto,
  TaskDiffDto,
  TaskDto,
} from "../ipc";

const WORKSPACE = "/Users/you/code/acme-web";
const WORKTREES = "/Users/you/.lore/worktrees/acme-web";
const START = Date.now();

const delay = <T>(value: T, ms = 60): Promise<T> =>
  new Promise((resolve) => setTimeout(() => resolve(value), ms));

// ── Files ────────────────────────────────────────────────────────────────

const FILES: Record<string, string> = {
  "README.md": `# acme-web

The Acme storefront: a Vite + React client and a small Rust pricing service.

## Getting started

\`\`\`sh
npm install
npm run dev
cargo run -p pricing
\`\`\`

See \`docs/ARCHITECTURE.md\` for how checkout talks to pricing.
`,
  "package.json": `{
  "name": "acme-web",
  "private": true,
  "type": "module",
  "scripts": {
    "dev": "vite",
    "build": "tsc --noEmit && vite build",
    "test": "vitest run"
  }
}
`,
  "src/main.tsx": `import React from "react";
import ReactDOM from "react-dom/client";

import { App } from "./App";

// Mount the storefront.
ReactDOM.createRoot(document.getElementById("root")!).render(
  <React.StrictMode>
    <App />
  </React.StrictMode>,
);
`,
  "src/App.tsx": `import { Cart } from "./checkout/Cart";

export function App() {
  return (
    <main className="shell">
      <h1>Acme</h1>
      <Cart />
    </main>
  );
}
`,
  "src/checkout/Cart.tsx": `import { useMemo, useState } from "react";

import { formatPrice } from "../lib/money";

type Line = { sku: string; name: string; cents: number; qty: number };

const SAMPLE: Line[] = [
  { sku: "A-100", name: "Anvil", cents: 12900, qty: 1 },
  { sku: "R-220", name: "Rocket skates", cents: 4999, qty: 2 },
];

export function Cart() {
  const [lines, setLines] = useState(SAMPLE);
  // Subtotal in cents; tax is applied by the pricing service.
  const subtotal = useMemo(() => lines.reduce((n, l) => n + l.cents * l.qty, 0), [lines]);

  return (
    <section aria-label="Cart">
      {lines.map((line) => (
        <div key={line.sku}>
          {line.name} × {line.qty}
          <button onClick={() => setLines(lines.filter((l) => l.sku !== line.sku))}>Remove</button>
        </div>
      ))}
      <p>Subtotal: {formatPrice(subtotal)}</p>
    </section>
  );
}
`,
  "src/lib/money.ts": `const USD = new Intl.NumberFormat("en-US", { style: "currency", currency: "USD" });

/** Format integer cents as a localized price. */
export function formatPrice(cents: number): string {
  return USD.format(cents / 100);
}
`,
  "services/pricing/Cargo.toml": `[package]
name = "pricing"
version = "0.1.0"
edition = "2021"

[dependencies]
serde = { version = "1", features = ["derive"] }
`,
  "services/pricing/src/lib.rs": `//! Price quotes for a cart, in integer cents.

use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize)]
pub struct Line {
    pub sku: String,
    pub cents: u64,
    pub qty: u32,
}

#[derive(Debug, Serialize, PartialEq)]
pub struct Quote {
    pub subtotal: u64,
    pub tax: u64,
    pub total: u64,
}

/// Quote a cart at a tax rate given in basis points.
pub fn quote(lines: &[Line], tax_bps: u64) -> Quote {
    let subtotal: u64 = lines.iter().map(|l| l.cents * u64::from(l.qty)).sum();
    let tax = subtotal * tax_bps / 10_000;
    Quote { subtotal, tax, total: subtotal + tax }
}
`,
  "docs/ARCHITECTURE.md": `# Architecture

- **client/** React storefront, talks to pricing over HTTP.
- **services/pricing** Rust crate that quotes carts in integer cents.

Money is always integer cents end to end; formatting happens only in the UI.
`,
};

function listDir(relPath: string): DirEntryDto[] {
  const prefix = relPath ? `${relPath}/` : "";
  const dirs = new Set<string>();
  const files: DirEntryDto[] = [];
  for (const path of Object.keys(FILES)) {
    if (!path.startsWith(prefix)) continue;
    const rest = path.slice(prefix.length);
    const slash = rest.indexOf("/");
    if (slash >= 0) dirs.add(rest.slice(0, slash));
    else files.push({ name: rest, rel_path: path, is_dir: false });
  }
  const folders = [...dirs].sort().map((name) => ({ name, rel_path: prefix + name, is_dir: true }));
  return [...folders, ...files.sort((a, b) => a.name.localeCompare(b.name))];
}

// ── Tasks ────────────────────────────────────────────────────────────────

type MockTask = {
  dto: TaskDto;
  activity: ActivityDto[];
  /** Events that stream in while the task runs, one per tick. */
  script: ActivityDto[];
  startedAt: number;
  emitted: number;
  diff: string;
  /** Continuations finish once their script has streamed in. */
  finishAfterScript?: boolean;
};

const USER_BRANCH = "main";

const TICK_MS = 2200;

const CLAUDE_SCRIPT: ActivityDto[] = [
  { kind: "tool", text: "Read services/pricing/src/lib.rs" },
  { kind: "message", text: "The tax is truncated with integer division, so 0.5¢ always rounds down. I'll switch to banker's rounding and add tests at the boundary." },
  { kind: "tool", text: "Edit services/pricing/src/lib.rs" },
  { kind: "tool", text: "Bash cargo test -p pricing" },
  { kind: "output", text: "running 4 tests\ntest quote_rounds_half_even ... ok\ntest quote_zero_cart ... ok\ntest quote_large_cart ... ok\ntest quote_basic ... ok" },
  { kind: "message", text: "Tests pass. Now updating the storefront copy so the subtotal note matches the new rounding." },
  { kind: "tool", text: "Edit src/checkout/Cart.tsx" },
  { kind: "tool", text: "Bash npm test -- checkout" },
  { kind: "output", text: " ✓ src/checkout/Cart.test.tsx (3 tests) 41ms" },
];

const CODEX_DIFF = `diff --git a/src/lib/money.ts b/src/lib/money.ts
index 3b1f2c4..9e0d7aa 100644
--- a/src/lib/money.ts
+++ b/src/lib/money.ts
@@ -1,6 +1,14 @@
-const USD = new Intl.NumberFormat("en-US", { style: "currency", currency: "USD" });
+const formatters = new Map<string, Intl.NumberFormat>();
${" "}
-/** Format integer cents as a localized price. */
-export function formatPrice(cents: number): string {
-  return USD.format(cents / 100);
+/** Format integer cents as a localized price, caching one formatter per locale and currency. */
+export function formatPrice(cents: number, currency = "USD", locale = "en-US"): string {
+  const key = \`\${locale}:\${currency}\`;
+  let fmt = formatters.get(key);
+  if (!fmt) {
+    fmt = new Intl.NumberFormat(locale, { style: "currency", currency });
+    formatters.set(key, fmt);
+  }
+  return fmt.format(cents / 100);
 }
diff --git a/src/lib/money.test.ts b/src/lib/money.test.ts
new file mode 100644
index 0000000..4c2a91e
--- /dev/null
+++ b/src/lib/money.test.ts
@@ -0,0 +1,12 @@
+import { describe, expect, it } from "vitest";
+
+import { formatPrice } from "./money";
+
+describe("formatPrice", () => {
+  it("formats USD by default", () => {
+    expect(formatPrice(12900)).toBe("$129.00");
+  });
+  it("supports other currencies", () => {
+    expect(formatPrice(4999, "EUR", "de-DE")).toBe("49,99 €");
+  });
+});
diff --git a/src/checkout/Cart.tsx b/src/checkout/Cart.tsx
index 7a0c9d1..b52e310 100644
--- a/src/checkout/Cart.tsx
+++ b/src/checkout/Cart.tsx
@@ -14,7 +14,7 @@ export function Cart() {
   const [lines, setLines] = useState(SAMPLE);
   // Subtotal in cents; tax is applied by the pricing service.
   const subtotal = useMemo(() => lines.reduce((n, l) => n + l.cents * l.qty, 0), [lines]);
-
+  const currency = "USD";
   return (
     <section aria-label="Cart">
       {lines.map((line) => (
@@ -22,7 +22,7 @@ export function Cart() {
           <button onClick={() => setLines(lines.filter((l) => l.sku !== line.sku))}>Remove</button>
         </div>
       ))}
-      <p>Subtotal: {formatPrice(subtotal)}</p>
+      <p>Subtotal: {formatPrice(subtotal, currency)}</p>
     </section>
   );
 }
`;

function task(partial: Partial<TaskDto> & Pick<TaskDto, "id" | "title" | "agent" | "state">): TaskDto {
  const slug = partial.id;
  return {
    prompt: "",
    repo_path: WORKSPACE,
    worktree_path: `${WORKTREES}/${slug}`,
    branch: `lore/${slug}`,
    base_commit: "4f9c2e1a7b3d",
    created_at_ms: START,
    exit_code: null,
    commits_ahead: 0,
    changed_files: [],
    last_activity: null,
    permission: "edits",
    runs: 1,
    uncommitted_count: 0,
    ...partial,
  };
}

const tasks: MockTask[] = [
  {
    dto: task({
      id: "fix-tax-rounding",
      title: "Fix tax rounding at half cents",
      prompt: "Tax on some carts is off by one cent. Find out why and fix it with tests.",
      agent: "claude_code",
      state: "running",
      created_at_ms: START - 4 * 60_000,
      commits_ahead: 1,
      changed_files: ["services/pricing/src/lib.rs"],
      permission: "auto",
      uncommitted_count: 1,
    }),
    activity: [
      { kind: "message", text: "I'll start by looking at how the pricing service computes tax." },
      { kind: "tool", text: "Grep \"tax_bps\" services/" },
      { kind: "output", text: "services/pricing/src/lib.rs:21: pub fn quote(lines: &[Line], tax_bps: u64) -> Quote {" },
    ],
    script: CLAUDE_SCRIPT,
    startedAt: START,
    emitted: 0,
    diff: `diff --git a/services/pricing/src/lib.rs b/services/pricing/src/lib.rs
index 1c2d3e4..5f6a7b8 100644
--- a/services/pricing/src/lib.rs
+++ b/services/pricing/src/lib.rs
@@ -20,6 +20,8 @@ pub struct Quote {
 /// Quote a cart at a tax rate given in basis points.
 pub fn quote(lines: &[Line], tax_bps: u64) -> Quote {
     let subtotal: u64 = lines.iter().map(|l| l.cents * u64::from(l.qty)).sum();
-    let tax = subtotal * tax_bps / 10_000;
+    // Round half to even so 0.5 cent does not always round down.
+    let raw = subtotal * tax_bps;
+    let tax = (raw + 5_000 - u64::from(raw % 20_000 == 5_000)) / 10_000;
     Quote { subtotal, tax, total: subtotal + tax }
 }
`,
  },
  {
    dto: task({
      id: "currency-formatting",
      title: "Multi-currency price formatting",
      prompt: "Let formatPrice take a currency and locale, cache formatters, add tests.",
      agent: "codex",
      state: "finished",
      created_at_ms: START - 52 * 60_000,
      exit_code: 0,
      commits_ahead: 2,
      changed_files: ["src/lib/money.ts", "src/lib/money.test.ts", "src/checkout/Cart.tsx"],
      last_activity: "Committed 2 changes on lore/currency-formatting",
      runs: 2,
    }),
    activity: [
      { kind: "message", text: "Plan: widen formatPrice's signature with defaults so existing callers keep working, cache one Intl.NumberFormat per locale/currency, then cover it with tests." },
      { kind: "tool", text: "shell: rg -n formatPrice src" },
      { kind: "output", text: "src/lib/money.ts:4:export function formatPrice(cents: number): string {\nsrc/checkout/Cart.tsx:31:      <p>Subtotal: {formatPrice(subtotal)}</p>" },
      { kind: "tool", text: "apply_patch src/lib/money.ts" },
      { kind: "tool", text: "apply_patch src/lib/money.test.ts" },
      { kind: "tool", text: "shell: npm test -- money" },
      { kind: "output", text: " ✓ src/lib/money.test.ts (2 tests) 12ms\n Test Files  1 passed (1)" },
      { kind: "tool", text: "shell: git commit -am \"money: format any currency\"" },
      { kind: "result", text: "Done. formatPrice now accepts currency and locale (defaulting to USD/en-US), caches formatters, and has tests. Cart passes its currency explicitly. 2 commits on lore/currency-formatting." },
    ],
    script: [],
    startedAt: START,
    emitted: 0,
    diff: CODEX_DIFF,
  },
  {
    dto: task({
      id: "upgrade-vite",
      title: "Upgrade to Vite 6",
      prompt: "Upgrade vite and plugin-react to the latest major and fix the build.",
      agent: "claude_code",
      state: "failed",
      created_at_ms: START - 3 * 3600_000,
      exit_code: 1,
      changed_files: ["package.json"],
      last_activity: "npm ERR! ERESOLVE could not resolve",
      uncommitted_count: 1,
      attention: "Usage limit reached for Claude Code. It resets at 3:00 PM; continue the task then, or hand it off to Codex.",
    }),
    activity: [
      { kind: "message", text: "I'll bump vite and @vitejs/plugin-react together, then reinstall." },
      { kind: "tool", text: "Edit package.json" },
      { kind: "tool", text: "Bash npm install" },
      { kind: "output", text: "npm ERR! code ERESOLVE\nnpm ERR! ERESOLVE could not resolve\nnpm ERR! peer vite@\"^4 || ^5\" from vite-plugin-legacy@2.1.0" },
      { kind: "error", text: "Agent exited with code 1: dependency conflict between vite@6 and vite-plugin-legacy@2.1.0 (peer vite ^4 || ^5)." },
    ],
    script: [],
    startedAt: START,
    emitted: 0,
    diff: `diff --git a/package.json b/package.json
index 2a3b4c5..6d7e8f9 100644
--- a/package.json
+++ b/package.json
@@ -12,7 +12,7 @@
   "devDependencies": {
-    "vite": "^5.4.0",
-    "@vitejs/plugin-react": "^4.3.1"
+    "vite": "^6.0.0",
+    "@vitejs/plugin-react": "^4.3.4"
   }
 }
`,
  },
  {
    dto: task({
      id: "cart-empty-state",
      title: "Cart empty state copy",
      prompt: "Show a friendly empty state in the cart with a link back to the catalog.",
      agent: "codex",
      state: "finished",
      created_at_ms: START - 75 * 60_000,
      exit_code: 0,
      commits_ahead: 0,
      changed_files: ["src/checkout/Cart.tsx", "src/checkout/EmptyCart.tsx"],
      last_activity: "Added EmptyCart and rendered it when the cart has no lines.",
      uncommitted_count: 2,
    }),
    activity: [
      { kind: "tool", text: "shell: rg -n \"lines.map\" src/checkout" },
      { kind: "tool", text: "apply_patch src/checkout/EmptyCart.tsx" },
      { kind: "tool", text: "apply_patch src/checkout/Cart.tsx" },
      { kind: "result", text: "Added EmptyCart and rendered it when the cart has no lines. Changes are not committed yet." },
    ],
    script: [],
    startedAt: START,
    emitted: 0,
    diff: `diff --git a/src/checkout/Cart.tsx b/src/checkout/Cart.tsx
index 7a0c9d1..c11e2f0 100644
--- a/src/checkout/Cart.tsx
+++ b/src/checkout/Cart.tsx
@@ -16,6 +16,7 @@ export function Cart() {
   const subtotal = useMemo(() => lines.reduce((n, l) => n + l.cents * l.qty, 0), [lines]);
 
+  if (lines.length === 0) return <EmptyCart />;
   return (
     <section aria-label="Cart">
`,
  },
  {
    dto: task({
      id: "readme-badges",
      title: "Add CI badges to README",
      prompt: "Add build and coverage badges to the README.",
      agent: "claude_code",
      state: "finished",
      created_at_ms: START - 26 * 3600_000,
      exit_code: 0,
      commits_ahead: 1,
      changed_files: ["README.md"],
      last_activity: "Added build and coverage badges.",
      merged_into: USER_BRANCH,
    }),
    activity: [
      { kind: "tool", text: "Edit README.md" },
      { kind: "result", text: "Added build and coverage badges." },
    ],
    script: [],
    startedAt: START,
    emitted: 0,
    diff: "",
  },
];

/** Other unmerged tasks in the same repository that changed the same files. */
function withOverlaps(dto: TaskDto): TaskDto {
  if (dto.merged_into) return { ...dto, overlaps: [] };
  const overlaps = tasks
    .map((t) => t.dto)
    .filter((o) => o.id !== dto.id && o.repo_path === dto.repo_path && !o.merged_into)
    .map((o) => ({
      task_id: o.id,
      title: o.title,
      files: o.changed_files.filter((f) => dto.changed_files.includes(f)),
    }))
    .filter((o) => o.files.length > 0);
  return { ...dto, overlaps };
}

/** Stream scripted events into running tasks based on elapsed time. */
function advance(t: MockTask) {
  if (t.dto.state !== "running") return;
  const due = Math.min(t.script.length, Math.floor((Date.now() - t.startedAt) / TICK_MS));
  if (due <= t.emitted) return;
  t.activity.push(...t.script.slice(t.emitted, due));
  t.emitted = due;
  const last = t.activity[t.activity.length - 1];
  t.dto = { ...t.dto, last_activity: last ? last.text.split("\n")[0].slice(0, 120) : null };
  if (t.finishAfterScript && t.emitted >= t.script.length) {
    t.finishAfterScript = false;
    t.dto = {
      ...t.dto,
      state: "finished",
      exit_code: 0,
      uncommitted_count: Math.max(1, t.dto.uncommitted_count ?? 0),
      attention: undefined,
    };
  }
}

function find(id: string): MockTask {
  const t = tasks.find((x) => x.dto.id === id);
  if (!t) throw new Error(`No task with id ${id}`);
  return t;
}

// ── Commands (same names and signatures as src/ipc.ts) ───────────────────

export function chooseRepositoryDirectory(): Promise<string | null> {
  return delay(WORKSPACE);
}

export function listTasks(): Promise<TaskDto[]> {
  tasks.forEach(advance);
  return delay(tasks.map((t) => withOverlaps(t.dto)));
}

export function createTask(request: CreateTaskRequest): Promise<TaskDto> {
  const slug =
    request.title
      .toLowerCase()
      .replace(/[^a-z0-9]+/g, "-")
      .replace(/^-|-$/g, "")
      .slice(0, 40) || "task";
  let id = slug;
  for (let i = 2; tasks.some((t) => t.dto.id === id); i++) id = `${slug}-${i}`;
  const created: MockTask = {
    dto: task({
      id,
      title: request.title,
      prompt: request.prompt,
      agent: request.agent,
      state: "running",
      created_at_ms: Date.now(),
      repo_path: request.repo_path,
      permission: request.permission ?? "edits",
    }),
    activity: [{ kind: "message", text: `Starting on: ${request.prompt}` }],
    script: [
      { kind: "tool", text: request.agent === "codex" ? "shell: git status --short" : "Bash git status --short" },
      { kind: "output", text: "(clean)" },
      { kind: "message", text: "Reading the relevant code before making changes." },
      { kind: "tool", text: request.agent === "codex" ? "shell: rg -n TODO src" : "Grep \"TODO\" src/" },
    ],
    startedAt: Date.now(),
    emitted: 0,
    diff: "",
  };
  tasks.unshift(created);
  return delay(created.dto, 250);
}

export function stopTask(id: string): Promise<TaskDto> {
  const t = find(id);
  if (t.dto.state === "running") {
    t.activity.push({ kind: "error", text: "Stopped by user." });
    t.dto = { ...t.dto, state: "stopped", last_activity: "Stopped by user." };
  }
  return delay(t.dto);
}

export function continueTask(request: ContinueTaskRequest): Promise<TaskDto> {
  const t = find(request.id);
  if (t.dto.state === "running") return Promise.reject(new Error("task is still running"));
  if (t.dto.merged_into) return Promise.reject(new Error("task is already merged"));
  const agent = request.agent ?? t.dto.agent;
  const handoff = agent !== t.dto.agent;
  t.activity.push({
    kind: "message",
    text: handoff ? `Handed off to ${agent === "codex" ? "Codex" : "Claude Code"}: ${request.prompt}` : request.prompt,
  });
  t.script = [
    { kind: "tool", text: agent === "codex" ? "shell: git diff --stat" : "Bash git diff --stat" },
    { kind: "message", text: "Picking up where the previous run left off." },
    { kind: "result", text: "Done with the follow-up." },
  ];
  t.emitted = 0;
  t.startedAt = Date.now();
  t.finishAfterScript = true;
  t.dto = {
    ...t.dto,
    agent,
    state: "running",
    exit_code: null,
    runs: (t.dto.runs ?? 1) + 1,
    attention: undefined,
    last_activity: request.prompt.slice(0, 120),
  };
  return delay(withOverlaps(t.dto), 200);
}

export function commitTask(id: string, message: string): Promise<TaskDto> {
  const t = find(id);
  if (t.dto.state === "running") return Promise.reject(new Error("task is still running"));
  if (!t.dto.uncommitted_count) return Promise.reject(new Error("nothing to commit"));
  const subject = message.trim() || t.dto.title;
  t.activity.push({ kind: "tool", text: `git commit -m "${subject}"` });
  t.dto = { ...t.dto, commits_ahead: t.dto.commits_ahead + 1, uncommitted_count: 0, last_activity: `Committed: ${subject}` };
  return delay(withOverlaps(t.dto), 200);
}

export function mergeTask(id: string): Promise<MergeResultDto> {
  const t = find(id);
  if (t.dto.state === "running") return Promise.reject(new Error("task is still running"));
  if (t.dto.uncommitted_count) return Promise.reject(new Error("task has uncommitted changes; commit them first"));
  if (t.dto.commits_ahead === 0) return Promise.reject(new Error("task has no commits to merge"));
  // A task conflicts with an overlapping task that was already merged.
  const conflicts = tasks
    .filter((o) => o !== t && o.dto.merged_into && o.dto.repo_path === t.dto.repo_path)
    .flatMap((o) => o.dto.changed_files.filter((f) => t.dto.changed_files.includes(f)));
  if (conflicts.length > 0) {
    return delay(
      {
        merged: false,
        into_branch: USER_BRANCH,
        conflicts: [...new Set(conflicts)],
        message: `Merging ${t.dto.branch} into ${USER_BRANCH} conflicted; the merge was aborted.`,
      },
      300,
    );
  }
  t.dto = { ...t.dto, merged_into: USER_BRANCH };
  return delay(
    {
      merged: true,
      into_branch: USER_BRANCH,
      conflicts: [],
      message: `Merged ${t.dto.branch} into ${USER_BRANCH} (${t.dto.commits_ahead} ${t.dto.commits_ahead === 1 ? "commit" : "commits"}).`,
    },
    300,
  );
}

export function discardTask(id: string): Promise<void> {
  const i = tasks.findIndex((t) => t.dto.id === id);
  if (i >= 0) tasks.splice(i, 1);
  return delay(undefined);
}

export function taskLoadWarning(): Promise<string | null> {
  return delay(null);
}

export function openTaskWorktree(id: string): Promise<void> {
  find(id);
  return delay(undefined);
}

export function openWorkspace(path: string): Promise<string> {
  return delay(path.startsWith(WORKTREES) ? path : WORKSPACE);
}

export function listWorkspaceDir(_root: string, relPath: string): Promise<DirEntryDto[]> {
  return delay(listDir(relPath));
}

export function readWorkspaceFile(_root: string, relPath: string): Promise<FileContentDto> {
  const text = FILES[relPath];
  if (text === undefined) return Promise.reject(new Error(`No such file: ${relPath}`));
  return delay({ rel_path: relPath, text, size: text.length, truncated: false });
}

export function taskDiff(id: string): Promise<TaskDiffDto> {
  return delay({ text: find(id).diff, truncated: false });
}

export function taskActivity(id: string): Promise<ActivityDto[]> {
  const t = find(id);
  advance(t);
  return delay([...t.activity]);
}
