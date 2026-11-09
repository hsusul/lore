// Sample memory for the browser-only mock: accepted notes, lessons waiting for
// review, and skill usage, spread over the sample repositories so the map has
// something to show. Every name, rule, and quote here is made up.

import type { DroppedRuleDto, EnforceRuleDto, NoteDto, ProposalDto, SkillUsageDto } from "../ipc";

const DAY = 24 * 60 * 60 * 1000;

/** A repository's scope and display name, as the vault records them. */
const REPOS: Record<string, { scope: string; name: string }> = {
  storefront: { scope: "repo:4f1c2a9e0b7d3c55", name: "acme-storefront" },
  pricing: { scope: "repo:9a0e6b1d2c3f4e88", name: "pricing-service" },
  site: { scope: "repo:1b2c3d4e5f607182", name: "notes-site" },
  admin: { scope: "repo:7e6d5c4b3a291807", name: "legacy-admin" },
  infra: { scope: "repo:c0ffee12ab34cd56", name: "infra" },
  mobile: { scope: "repo:a1b2c3d4e5f60718", name: "mobile-app" },
  pipeline: { scope: "repo:5e4d3c2b1a090807", name: "data-pipeline" },
  design: { scope: "repo:0a1b2c3d4e5f6071", name: "design-system" },
  cli: { scope: "repo:f0e1d2c3b4a59687", name: "dev-cli" },
};

type Where = keyof typeof REPOS | "global";

const scopeOf = (where: Where) =>
  where === "global"
    ? { scope: "global", repo_name: null }
    : { scope: REPOS[where].scope, repo_name: REPOS[where].name };

const slug = (title: string) =>
  title
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, "-")
    .replace(/^-|-$/g, "");

/** [where, type, title, excerpt] */
const NOTES: [Where, string, string, string][] = [
  ["global", "convention", "Never push to main", "Open a PR, even for a one-line fix."],
  ["global", "convention", "Use pnpm, not npm", "Every JS repo here has a pnpm lockfile."],
  ["global", "convention", "Run the tests before saying done", "Say which tests ran and what they printed."],
  ["global", "convention", "No co-author trailers in commits", "Commits carry the person's name only."],
  ["global", "pitfall", "Bound cargo jobs on this laptop", "Run cargo with CARGO_BUILD_JOBS=4 or the machine stalls."],
  ["global", "convention", "Ask before deleting files you did not create", "Move them aside and say where instead."],
  ["global", "decision", "Conventional commit titles", "type(scope): summary, lowercase, no period."],
  ["global", "howto", "Rebase, do not merge main into a branch", "git fetch && git rebase origin/main."],
  ["global", "convention", "Keep PR descriptions short", "Why, what changed, how it was checked."],
  ["global", "pitfall", "Do not run brew upgrade", "It breaks the pinned toolchains."],
  ["global", "convention", "Prefer editing a file to rewriting it", "Small diffs are easier to review."],
  ["global", "fact", "Work repos live on the external SSD", "/Volumes/Work, not ~/code."],
  ["storefront", "convention", "Money is integer cents", "Never a float. Format only at the edge."],
  ["storefront", "pitfall", "Cart reducer must not mutate state", "Return a new cart; the memo depends on it."],
  ["storefront", "howto", "Seed the dev database", "pnpm db:seed after every migration."],
  ["storefront", "convention", "Components in PascalCase files", "One component per file under src/components."],
  ["storefront", "decision", "Checkout stays server-rendered", "No client-only checkout steps."],
  ["storefront", "pitfall", "Stripe webhooks need the raw body", "Do not parse JSON before verifying the signature."],
  ["storefront", "convention", "Feature flags go through flags.ts", "No direct env reads in components."],
  ["storefront", "fact", "Staging resets every night", "Do not keep test data there."],
  ["storefront", "convention", "Snapshot tests are banned", "Assert on what the user sees."],
  ["storefront", "pitfall", "Images need explicit sizes", "Layout shift fails the perf budget."],
  ["pricing", "convention", "Round half to even", "Banker's rounding everywhere in pricing."],
  ["pricing", "pitfall", "Retry only idempotent calls", "POST /charge is not idempotent."],
  ["pricing", "decision", "Prices are versioned, never edited", "Add a new version row instead."],
  ["pricing", "howto", "Regenerate the OpenAPI client", "make client after changing api.yaml."],
  ["pricing", "convention", "Errors carry a code", "Return { code, message }, never a bare string."],
  ["pricing", "pitfall", "Time zones come from the store", "Never the server's local time."],
  ["pricing", "fact", "Currency table lives in shared/", "Do not copy it into the service."],
  ["site", "convention", "Posts use sentence case titles", "Not Title Case."],
  ["site", "howto", "Preview drafts", "pnpm dev, then /drafts."],
  ["site", "pitfall", "Do not commit generated og images", "CI builds them."],
  ["site", "convention", "Links are relative", "The site moves between domains."],
  ["site", "decision", "No client JS on article pages", "Static HTML only."],
  ["admin", "pitfall", "PHP 7.4 only", "The host cannot run 8.x yet."],
  ["admin", "convention", "Do not touch the reports module", "It is being replaced."],
  ["admin", "howto", "Run it locally", "docker compose up admin-db first."],
  ["infra", "pitfall", "Plan before apply", "Paste the terraform plan in the PR."],
  ["infra", "convention", "One module per service", "No shared mega-module."],
  ["infra", "pitfall", "Never edit state by hand", "Use terraform state mv."],
  ["infra", "fact", "Prod is eu-west-1", "Staging is eu-west-2."],
  ...extra("global", [
    "Say what you did not check",
    "Read the error before retrying",
    "No new dependencies without asking",
    "Keep secrets out of logs",
    "Use the repo's formatter, not your own style",
    "Write dates as YYYY-MM-DD",
    "Stop and ask when the plan changes",
    "Do not silence lint rules",
    "One change per PR",
    "Delete dead code instead of commenting it out",
    "Name branches type/short-summary",
    "Link the issue in the PR",
    "Do not force-push shared branches",
    "Pin tool versions in CI",
    "Prefer the standard library",
    "Leave TODOs with an owner",
  ]),
  ...extra("storefront", [
    "Prices render through formatMoney",
    "Every page needs a loading state",
    "Use the Button component, not a raw button",
    "Track events through analytics.ts",
    "Coupons validate on the server",
    "Product slugs never change",
    "Cart totals round once, at the end",
    "Search uses the index, not SQL LIKE",
    "Address forms follow the country format",
    "Run e2e against the preview URL",
    "Inventory reads are eventually consistent",
    "Promo banners come from the CMS",
  ]),
  ...extra("pricing", [
    "Tax is computed per line",
    "Discounts apply before tax",
    "Log the price version with every quote",
    "Backfills run in batches of 1,000",
    "Feature tests use the fixed clock",
    "No ORM lazy loads in handlers",
    "Quotes expire after 15 minutes",
    "Currency codes are ISO 4217",
  ]),
  ...extra("mobile", [
    "Test on the smallest supported phone",
    "Strings go through i18n",
    "No network calls on the main thread",
    "Bump the build number every release",
    "Deep links are listed in links.ts",
    "Use the shared haptics helper",
    "Respect reduced motion",
    "Images load through the cache",
    "Push tokens refresh on login",
    "Dark mode is checked in every review",
  ]),
  ...extra("pipeline", [
    "Jobs must be idempotent",
    "Partition by event date, not load date",
    "Schema changes need a backfill plan",
    "Never read prod from a notebook",
    "Late events land in the next partition",
    "Every table has an owner",
    "Dedupe on event id",
    "Use UTC everywhere",
  ]),
  ...extra("design", [
    "Spacing uses the 4px scale",
    "Every component has a story",
    "Colors come from tokens",
    "Icons are 24-unit strokes",
    "Focus rings are never removed",
    "Breaking changes need a codemod",
    "Contrast meets AA",
  ]),
  ...extra("cli", [
    "Exit codes are documented",
    "--json output is stable",
    "Errors go to stderr",
    "No color when not a TTY",
    "Every flag has a long form",
  ]),
  ...extra("site", ["Alt text on every image", "Feeds stay valid RSS", "Code blocks name their language"]),
  ...extra("infra", ["Alarms page the owner, not a channel", "Rotate keys every 90 days", "Backups are restored monthly"]),
  ...extra("admin", ["Exports are CSV with a header", "Sessions time out after 30 minutes"]),
];

/** Notes with only a title, cycling through the note types. */
function extra(where: Where, titles: string[]): [Where, string, string, string][] {
  const types = ["convention", "pitfall", "howto", "decision", "fact"];
  return titles.map((title, i) => [where, types[i % types.length], title, ""]);
}

/** [where, agent, title, why, receipts as [said, days ago]] */
const LESSONS: [Where, string, string, string, [string, number][]][] = [
  [
    "global",
    "claude-code",
    "Show the failing test before the fix",
    "The agent fixed the bug first and wrote a passing test after, so nothing showed the test catches the bug.",
    [
      ["write the failing test first, then fix it", 2],
      ["no, I want to see it fail before you change the code", 9],
      ["again: red test first", 16],
    ],
  ],
  [
    "global",
    "codex",
    "Quote paths that have spaces",
    "The agent ran a shell command on an unquoted path under /Volumes/990 Pro and it split in two.",
    [
      ["quote the path, it has a space in it", 3],
      ["you need quotes around that path", 11],
    ],
  ],
  [
    "storefront",
    "claude-code",
    "Use the design tokens, not raw hex colors",
    "The agent styled the banner with hard-coded colors that ignore dark mode.",
    [
      ["use the tokens from tokens.css, not hex", 1],
      ["no raw colors please, dark mode breaks", 6],
    ],
  ],
  [
    "pricing",
    "codex",
    "Add a migration for every schema change",
    "The agent changed the model and left the database to drift.",
    [
      ["you forgot the migration", 4],
      ["every schema change needs a migration file", 13],
    ],
  ],
  [
    "site",
    "claude-code",
    "Keep posts under 80 characters per line",
    "The agent reflowed a post into long lines that break the diff view.",
    [["wrap at 80 like the other posts", 5]],
  ],
  [
    "infra",
    "codex",
    "Tag every resource with owner and service",
    "The agent created a bucket with no tags, and the cost report could not place it.",
    [
      ["add the owner and service tags", 7],
      ["untagged again, every resource needs tags", 12],
    ],
  ],
  [
    "global",
    "claude-code",
    "Ask before installing a new package",
    "The agent added a dependency to solve a ten-line problem.",
    [
      ["don't add a package for that, just write it", 1],
      ["ask me before adding dependencies", 8],
    ],
  ],
  [
    "global",
    "codex",
    "Say which commands you did not run",
    "The agent reported the change as tested when only the build had run.",
    [
      ["you said tested but you only built it", 2],
      ["tell me what you didn't run", 10],
      ["don't say done if the tests didn't run", 15],
    ],
  ],
  [
    "global",
    "claude-code",
    "Keep commits to one change",
    "The agent bundled a refactor and a bug fix into one commit.",
    [["split that into two commits", 4]],
  ],
  [
    "global",
    "claude-code",
    "Never edit generated files",
    "The agent hand-edited the generated API client instead of the schema.",
    [
      ["that file is generated, change the schema", 3],
      ["don't touch anything under gen/", 14],
    ],
  ],
  [
    "storefront",
    "codex",
    "Every price goes through formatMoney",
    "The agent formatted a price with toFixed and showed $12.5.",
    [
      ["use formatMoney, not toFixed", 1],
      ["prices always go through formatMoney", 9],
    ],
  ],
  [
    "storefront",
    "claude-code",
    "Test checkout in both currencies",
    "The agent checked the fix in USD only, and EUR still rounded wrong.",
    [["check it in EUR too", 2]],
  ],
  [
    "storefront",
    "claude-code",
    "Keep the cart reducer pure",
    "The agent pushed into state.items and the total stopped updating.",
    [
      ["don't mutate state in the reducer", 5],
      ["again, return a new cart", 11],
    ],
  ],
  [
    "pricing",
    "claude-code",
    "Use the fixed clock in tests",
    "The agent called Date.now in a test that then failed at midnight.",
    [["use the test clock, this is flaky at midnight", 3]],
  ],
  [
    "pricing",
    "codex",
    "Never retry a charge",
    "The agent wrapped the charge call in the generic retry helper.",
    [
      ["no, charges are not idempotent, don't retry them", 6],
      ["remove the retry around charge", 13],
    ],
  ],
  [
    "mobile",
    "claude-code",
    "Every string goes through i18n",
    "The agent hard-coded English labels on the settings screen.",
    [
      ["those strings need to be translated", 2],
      ["use t() for every label", 7],
    ],
  ],
  [
    "mobile",
    "codex",
    "Check the small-phone layout",
    "The agent's new button pushed the form off screen on an SE.",
    [["check it on the small phone", 4]],
  ],
  [
    "pipeline",
    "claude-code",
    "Make every job safe to rerun",
    "The agent's backfill appended rows, so a rerun doubled the counts.",
    [
      ["this has to be idempotent, it doubled the data", 1],
      ["jobs must be safe to rerun", 12],
    ],
  ],
  [
    "pipeline",
    "codex",
    "Write timestamps in UTC",
    "The agent stored local time and the daily partitions split wrong.",
    [["utc please, the partitions are off", 5]],
  ],
  [
    "design",
    "claude-code",
    "Use the 4px spacing scale",
    "The agent used 10px and 14px paddings that are not on the scale.",
    [
      ["stick to the spacing scale", 3],
      ["no 10px, use space-2 or space-3", 9],
    ],
  ],
  [
    "design",
    "claude-code",
    "Never remove focus rings",
    "The agent set outline: none on the new button.",
    [["put the focus ring back", 6]],
  ],
  [
    "cli",
    "codex",
    "Send errors to stderr",
    "The agent printed an error to stdout and broke a script reading --json.",
    [
      ["errors go to stderr, it broke the json", 2],
      ["stdout is for output only", 16],
    ],
  ],
  [
    "admin",
    "claude-code",
    "Stay on PHP 7.4 syntax",
    "The agent used match expressions the host's PHP cannot run.",
    [["the server is 7.4, no match", 8]],
  ],
  [
    "site",
    "codex",
    "Give every image alt text",
    "The agent added a diagram to a post with an empty alt.",
    [["that image needs alt text", 4]],
  ],
  [
    "global",
    "codex",
    "Show the failing test before you fix it",
    "The agent changed the code before any test showed the bug.",
    [["test first, show me it fails", 1]],
  ],
];

/** A lesson filed again, by title, and the earlier lesson it repeats. */
const REPEATS: Record<string, string> = {
  "Show the failing test before you fix it": "Show the failing test before the fix",
};

/** [skill, uses, sessions, days since last use or null, re-prompts as [said, before]] */
const SKILLS: [string, number, number, number | null, [string, string][]][] = [
  ["code-review", 22, 15, 1, [["no, you missed the tests, look again", "Reviewed it, looks fine."]]],
  ["debug-first", 31, 19, 0, []],
  ["tdd", 18, 11, 2, []],
  ["frontend-design", 14, 9, 3, [["that is generic, use our tokens", "Here is a clean card layout."]]],
  ["git-workflow", 40, 28, 0, []],
  ["clean-git-commit", 26, 20, 1, []],
  ["pr-readiness", 12, 10, 4, []],
  ["security-review", 7, 6, 9, []],
  ["diagnose", 9, 7, 5, []],
  ["implementation-strategy", 11, 8, 6, []],
  ["repo-onboarding", 6, 6, 12, []],
  ["playwright-cli", 15, 9, 2, []],
  ["write-a-skill", 3, 3, 20, []],
  ["research-paper-summary", 4, 2, 30, []],
  ["minimalist-ui", 5, 4, 15, []],
  ["react-doctor", 8, 6, 7, [["you did not run it on the changed files", "React Doctor found no issues."]]],
  ["taste-skill", 0, 0, null, []],
  ["grill-me", 2, 2, 25, []],
  ["verify", 27, 21, 0, []],
  ["init", 5, 5, 18, []],
  ["ponytail", 10, 7, 3, []],
  ["ponytail-review", 6, 5, 4, []],
  ["high-end-visual-design", 7, 5, 8, []],
  ["ui-ux-pro-max", 9, 6, 6, []],
  ["to-prd", 3, 3, 22, []],
  ["to-issues", 4, 3, 19, []],
  ["triage", 6, 4, 10, []],
  ["prototype", 5, 4, 11, []],
  ["ml-experiment", 2, 1, 40, []],
  ["backend-reliability", 8, 6, 5, [["you did not add a timeout", "Added the retry loop."]]],
  ["code-change-verification", 13, 10, 1, []],
  ["issue-validation", 4, 4, 14, []],
  ["review-response", 3, 3, 9, []],
  ["caveman", 1, 1, 33, []],
  ["stitch-design-taste", 0, 0, null, []],
  ["brandkit", 0, 0, null, []],
];

const bash = (pattern: string, unless: string | null = null): EnforceRuleDto => ({ tool: "bash", pattern, unless });
const retryCharge: EnforceRuleDto = { tool: "write", pattern: "retry\\(\\s*charge", unless: null };

/** A few notes' `enforce` rules, by title, and one that does not parse. */
const NOTE_RULES: Record<string, { enforce: EnforceRuleDto[]; dropped_rules: DroppedRuleDto[] }> = {
  "Never push to main": { enforce: [bash("git push\\b.*\\bmain\\b", "--dry-run")], dropped_rules: [] },
  "Do not run brew upgrade": {
    enforce: [bash("\\bbrew upgrade\\b")],
    dropped_rules: [
      { written: "tool: bash\npattern: 'brew (upgrade", error: "`enforce[1].pattern`: `brew (upgrade` is not a valid regex: unclosed group" },
    ],
  },
  "Retry only idempotent calls": { enforce: [retryCharge], dropped_rules: [] },
};

/** Rules some lessons would add, by title; one updates a note's rules. */
const LESSON_RULES: Record<string, { enforce: EnforceRuleDto[]; updates?: string }> = {
  "Ask before installing a new package": { enforce: [bash("\\b(npm|pnpm) (add|install|i) [a-z@]")] },
  "Never edit generated files": { enforce: [{ tool: "path", pattern: "(^|/)generated/", unless: null }] },
  "Never retry a charge": {
    enforce: [retryCharge, { tool: "write", pattern: "withRetry\\(\\s*charge", unless: null }],
    updates: "Retry only idempotent calls",
  },
};

export function buildSampleNotes(): NoteDto[] {
  return NOTES.map(([where, type, title, excerpt], i) => {
    const s = scopeOf(where);
    const folder = where === "global" ? "global" : `repos/${REPOS[where].name}`;
    return {
      id: `01JD${String(i + 1).padStart(22, "0")}`,
      path: `${folder}/${slug(title)}.md`,
      title,
      note_type: type,
      excerpt,
      ...s,
      ...(NOTE_RULES[title] ?? { enforce: [], dropped_rules: [] }),
    };
  });
}

export function buildSampleProposals(now: number): ProposalDto[] {
  const notes = buildSampleNotes();
  const proposals: ProposalDto[] = LESSONS.map(([where, agent, title, reason, said], i) => {
    const rules = LESSON_RULES[title];
    const target = notes.find((n) => n.title === rules?.updates);
    const date = (days: number) => new Date(now - days * DAY).toISOString().slice(0, 10);
    const body = [
      reason,
      "",
      "Receipts:",
      ...said.map(([text, days], k) => `- "${text}" (${agent}, session s${i + 1}${k}, ${date(days)})`),
      "",
    ].join("\n");
    return {
      id: `01JE${String(i + 1).padStart(22, "0")}`,
      title,
      body,
      note_type: "convention",
      agent_id: agent,
      session_id: `s${i + 1}0`,
      proposed_ms: now - said[0][1] * DAY,
      enforce: rules?.enforce ?? [],
      updates: target?.id ?? null,
      target_enforce: target?.enforce ?? [],
      repeats: null,
      ...scopeOf(where),
    };
  });
  return proposals.map((p) => {
    const earlier = proposals.find((e) => e.title === REPEATS[p.title]);
    return earlier ? { ...p, repeats: { id: earlier.id, title: earlier.title, kind: "proposal" } } : p;
  });
}

export function buildSampleSkills(now: number): SkillUsageDto[] {
  return SKILLS.map(([skill, uses, sessions, days, reprompts]) => ({
    skill,
    uses,
    sessions,
    last_ms: days === null ? null : now - days * DAY,
    reprompts: reprompts.map(([text, preceding], k) => ({
      session_id: `k${k}`,
      agent_id: "claude-code",
      repo: "acme-storefront",
      at_ms: now - (days ?? 0) * DAY,
      text,
      preceding,
    })),
    editable_path: uses > 0 ? `/Users/you/.claude/skills/${skill}/SKILL.md` : null,
  }));
}
