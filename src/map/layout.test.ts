// @vitest-environment node
import { describe, expect, it } from "vitest";

import type { NoteDto, ProposalDto, SkillUsageDto } from "../ipc";
import { areasOf, CENTRE, layout, SKILLS } from "./layout";

/** A proposal the agent filed, with `n` receipts, scoped to `repo` or everywhere. */
const lesson = (id: string, repo: string | null, n = 1): ProposalDto => ({
  id,
  title: `Rule ${id}`,
  body: `What went wrong.\n\nReceipts:\n${Array.from({ length: n }, (_, i) => `- "said ${i}" (codex, session s${i}, 2026-10-0${i + 1})`).join("\n")}\n`,
  scope: repo ? `repo:${repo}key` : "global",
  note_type: "convention",
  agent_id: "codex",
  session_id: "s0",
  repo_name: repo,
  proposed_ms: 0,
});

const note = (
  id: string,
  scope: string,
  repo_name: string | null = null,
): NoteDto => ({
  id,
  path: `${id}.md`,
  title: `Note ${id}`,
  scope,
  note_type: "fact",
  excerpt: "",
  repo_name,
});

describe("areasOf", () => {
  it("puts each item in its scope's repository, the centre first and named for the person", () => {
    const areas = areasOf(
      [lesson("a", "beeclean", 3), lesson("b", null), lesson("c", "lore")],
      [
        note("n1", "global"),
        note("n2", "repo:016fc9809d74377f", "beeclean"),
        note("n3", "repo:aaaaaaaaaaaaaaaa"),
      ],
      "Henry",
    );
    expect(
      areas.map((a) => [
        a.key,
        a.name,
        a.lessons.map((l) => l.id),
        a.notes.map((n) => n.id),
      ]),
    ).toEqual([
      [CENTRE, "Henry", ["b"], ["n1"]],
      ["beeclean", "beeclean", ["a"], ["n2"]],
      ["lore", "lore", ["c"], []],
      ["repo:aaaaaaaaaaaaaaaa", "Unlinked repository", [], ["n3"]],
    ]);
  });

  it("keeps the centre even when nothing is in it", () => {
    expect(areasOf([], [], "Henry").map((a) => a.key)).toEqual([CENTRE]);
  });
});

describe("layout", () => {
  const areas = areasOf(
    [
      ...Array.from({ length: 30 }, (_, i) => lesson(`b${i}`, "beeclean")),
      lesson("l1", "lore", 4),
      lesson("g1", null),
    ],
    [
      note("n1", "global"),
      note("n2", "global"),
      ...Array.from({ length: 8 }, (_, i) =>
        note(`v${i}`, "repo:k", "verbatim"),
      ),
    ],
    "Henry",
  );
  const model = layout(areas);

  it("draws one dot per lesson and note, and one cluster per area", () => {
    expect(model.dots).toHaveLength(32 + 10);
    expect(model.clusters.map((c) => [c.key, c.lessons])).toEqual([
      [CENTRE, 1],
      ["beeclean", 30],
      ["verbatim", 0],
      ["lore", 1],
    ]);
    expect(model.clusters[0]).toMatchObject({ x: 0, y: 0, name: "Henry" });
  });

  it("packs clusters without overlap, each touching what was placed before", () => {
    const { clusters } = model;
    for (let i = 0; i < clusters.length; i++) {
      for (let j = i + 1; j < clusters.length; j++) {
        const [a, b] = [clusters[i], clusters[j]];
        expect(Math.hypot(a.x - b.x, a.y - b.y)).toBeGreaterThanOrEqual(
          a.r + b.r + 40,
        );
      }
    }
    const nearest = (c: (typeof clusters)[number]) =>
      Math.min(
        ...clusters
          .filter((o) => o !== c)
          .map((o) => Math.hypot(o.x - c.x, o.y - c.y) - o.r - c.r),
      );
    for (const c of clusters.slice(1)) expect(nearest(c)).toBeLessThan(60);
  });

  it("keeps every dot inside its cluster, and grows a lesson with more receipts", () => {
    for (const d of model.dots) {
      const c = model.clusters.find((k) => k.key === d.area);
      expect(c).toBeDefined();
      if (c)
        expect(Math.hypot(d.x - c.x, d.y - c.y)).toBeLessThanOrEqual(
          c.r + 0.01,
        );
    }
    const r = (id: string) => model.dots.find((d) => d.id === id)?.r;
    expect(r("lesson:l1")).toBeCloseTo(4.4);
    expect(r("lesson:g1")).toBeCloseTo(2.6);
    expect(r("note:n1")).toBeCloseTo(2.2);
  });

  it("bounds every cluster and the name below it", () => {
    const [left, top, right, bottom] = model.bounds;
    for (const c of model.clusters) {
      expect(c.x - c.r).toBeGreaterThan(left);
      expect(c.x + c.r).toBeLessThan(right);
      expect(c.y - c.r).toBeGreaterThan(top);
      expect(c.y + c.r + 30).toBeLessThan(bottom);
    }
  });
});

describe("the Skills cluster", () => {
  const skill = (
    name: string,
    uses: number,
    reprompted: boolean,
  ): SkillUsageDto => ({
    skill: name,
    uses,
    sessions: uses,
    last_ms: null,
    reprompts: reprompted
      ? [
          {
            session_id: "s",
            agent_id: "codex",
            repo: null,
            at_ms: 0,
            text: "no, again",
            preceding: null,
          },
        ]
      : [],
    editable_path: null,
  });

  it("is one more cluster, one dot per skill, counting those that need a look", () => {
    const model = layout(
      areasOf([], [note("n1", "global")], "Henry", [
        skill("review", 22, true),
        skill("idle", 0, false),
        skill("fmt", 4, false),
      ]),
    );
    const skills = model.clusters.find((c) => c.key === SKILLS);
    expect(skills).toMatchObject({ name: "Skills", lessons: 1 });
    expect(
      model.dots.filter((d) => d.area === SKILLS).map((d) => d.id),
    ).toEqual(["skill:review", "skill:fmt", "skill:idle"]);
    expect(model.dots.find((d) => d.id === "skill:idle")?.r).toBeCloseTo(1.2);
  });

  it("is left out when there are no skills", () => {
    expect(areasOf([], [], "Henry", []).map((a) => a.key)).toEqual([CENTRE]);
  });
});
