import type { NoteDto, ProposalDto, SkillUsageDto } from "../ipc";
import { receipts } from "./proposal";

/**
 * The map: what holds in every repository sits in the centre, named for the
 * person, and each repository is a cluster around it. Every dot is a real
 * lesson an agent filed for review, an accepted note, or one installed skill
 * in the Skills cluster. Coordinates are map units; the view scales them to
 * the window.
 */

export const CENTRE = "global";
export const SKILLS = "skills";

export type Item =
  | { kind: "lesson"; lesson: ProposalDto }
  | { kind: "note"; note: NoteDto }
  | { kind: "skill"; skill: SkillUsageDto };

export interface Area {
  key: string;
  name: string;
  lessons: ProposalDto[];
  notes: NoteDto[];
  skills: SkillUsageDto[];
}

export interface Dot {
  id: string;
  area: string;
  label: string;
  item: Item;
  /** Radius in map units: a lesson grows with the conversations it came from. */
  r: number;
  x: number;
  y: number;
}

export interface Cluster {
  key: string;
  name: string;
  x: number;
  y: number;
  /** Radius of the dots, without the room left for the name below. */
  r: number;
  /** The count under its name: lessons waiting, or skills that need a look. */
  lessons: number;
}

export interface MapModel {
  clusters: Cluster[];
  dots: Dot[];
  /** [left, top, right, bottom], names included. */
  bounds: [number, number, number, number];
}

/** Space between neighbouring dots in a cluster. */
export const SPACE = 7.5;
/** Room around a cluster for its name and the gap to the next one. */
const PAD = 20;
const GAP = 6;
/** A cluster's name hangs below it, so it packs as a circle a little lower. */
const LIFT = 9;
const GOLDEN = 2.39996;

/** A scope, as the name of its repository; an unnamed repository keeps its scope key. */
function areaOf(item: { scope: string; repo_name: string | null }): string {
  if (item.scope === CENTRE) return CENTRE;
  return item.repo_name ?? item.scope;
}

/** A skill the person re-prompted after: it needs a look. */
export const needsALook = (skill: SkillUsageDto) => skill.reprompts.length > 0;

/**
 * Group lessons and notes into areas, plus the installed skills as one more:
 * the centre first, then areas by size, then name.
 */
export function areasOf(lessons: ProposalDto[], notes: NoteDto[], person: string, skills: SkillUsageDto[] = []): Area[] {
  const byKey = new Map<string, Area>();
  const area = (key: string): Area => {
    let found = byKey.get(key);
    if (!found) {
      const name = key === CENTRE ? person : key.startsWith("repo:") ? "Unlinked repository" : key;
      found = { key, name, lessons: [], notes: [], skills: [] };
      byKey.set(key, found);
    }
    return found;
  };
  area(CENTRE);
  for (const lesson of lessons) area(areaOf(lesson)).lessons.push(lesson);
  for (const note of notes) area(areaOf(note)).notes.push(note);
  if (skills.length) {
    const skillArea = { key: SKILLS, name: "Skills", lessons: [], notes: [], skills };
    byKey.set(SKILLS, skillArea);
  }
  const size = (a: Area) => a.lessons.length + a.notes.length + a.skills.length;
  return [...byKey.values()].sort(
    (a, b) => Number(b.key === CENTRE) - Number(a.key === CENTRE) || size(b) - size(a) || a.name.localeCompare(b.name),
  );
}

/** Skills are many and small, so they pack tighter than lessons and notes. */
const SKILL_SPACE = 4;

export const clusterRadius = (n: number, space = SPACE) => space * Math.sqrt(n);

const spaceOf = (a: Area) => (a.key === SKILLS ? SKILL_SPACE : SPACE);

/**
 * Pack the clusters: the centre at the origin, then each repository, biggest
 * first, at the free spot closest to the centre that touches what is placed.
 */
function pack(areas: Area[]): { key: string; x: number; y: number }[] {
  const placed: { x: number; y: number; r: number }[] = [];
  const out: { key: string; x: number; y: number }[] = [];
  const free = (x: number, y: number, r: number) => placed.every((q) => Math.hypot(q.x - x, q.y - y) >= q.r + r + GAP - 0.01);
  areas.forEach((a, i) => {
    const r = clusterRadius(a.lessons.length + a.notes.length + a.skills.length, spaceOf(a)) + PAD;
    if (i === 0) {
      placed.push({ x: 0, y: LIFT, r });
      out.push({ key: a.key, x: 0, y: 0 });
      return;
    }
    let best: { x: number; y: number; d: number } | null = null;
    const tryAt = (x: number, y: number) => {
      if (!free(x, y, r)) return;
      const d = Math.hypot(x, y);
      if (!best || d < best.d) best = { x, y, d };
    };
    for (const q of placed) {
      for (let t = 0; t < 48; t++) {
        const angle = (t / 48) * 2 * Math.PI + i;
        tryAt(q.x + (q.r + r + GAP) * Math.cos(angle), q.y + (q.r + r + GAP) * Math.sin(angle));
      }
    }
    // Where it touches two placed clusters at once: the tightest fits.
    for (let u = 0; u < placed.length; u++) {
      for (let v = u + 1; v < placed.length; v++) {
        const A = placed[u];
        const B = placed[v];
        const ra = A.r + r + GAP;
        const rb = B.r + r + GAP;
        const dx = B.x - A.x;
        const dy = B.y - A.y;
        const dd = Math.hypot(dx, dy);
        if (dd > ra + rb || dd < Math.abs(ra - rb) || dd === 0) continue;
        const l = (ra * ra - rb * rb + dd * dd) / (2 * dd);
        const h = Math.sqrt(Math.max(0, ra * ra - l * l));
        const mx = A.x + (dx * l) / dd;
        const my = A.y + (dy * l) / dd;
        tryAt(mx + (h * dy) / dd, my - (h * dx) / dd);
        tryAt(mx - (h * dy) / dd, my + (h * dx) / dd);
      }
    }
    const spot: { x: number; y: number } = best ?? { x: placed[placed.length - 1].x + 2 * r, y: 0 };
    placed.push({ x: spot.x, y: spot.y, r });
    out.push({ key: a.key, x: spot.x, y: spot.y - LIFT });
  });
  return out;
}

/** A lesson backed by more receipts is drawn larger. */
const lessonRadius = (lesson: ProposalDto) => 2.6 + Math.min(3, Math.max(0, receipts(lesson.body).length - 1)) * 0.6;
const NOTE_RADIUS = 2.2;
/** A skill that runs more is drawn larger. */
const skillRadius = (skill: SkillUsageDto) => 1.2 + Math.min(2.4, Math.sqrt(skill.uses) * 0.25);

/** Lay out every area: clusters packed, and each cluster's dots in a sunflower, lessons in the middle. */
export function layout(areas: Area[]): MapModel {
  const spots = new Map(pack(areas).map((s) => [s.key, s]));
  const clusters: Cluster[] = [];
  const dots: Dot[] = [];
  for (const a of areas) {
    const spot = spots.get(a.key);
    if (!spot) continue;
    const items: Omit<Dot, "x" | "y">[] = [
      ...a.lessons.map((lesson) => ({
        id: `lesson:${lesson.id}`,
        area: a.key,
        label: lesson.title,
        item: { kind: "lesson", lesson } as Item,
        r: lessonRadius(lesson),
      })),
      ...a.notes.map((note) => ({
        id: `note:${note.id}`,
        area: a.key,
        label: note.title,
        item: { kind: "note", note } as Item,
        r: NOTE_RADIUS,
      })),
      // Skills that need a look sit in the middle, the rest by use.
      ...[...a.skills]
        .sort((p, q) => Number(needsALook(q)) - Number(needsALook(p)) || q.uses - p.uses)
        .map((skill) => ({
          id: `skill:${skill.skill}`,
          area: a.key,
          label: skill.skill,
          item: { kind: "skill", skill } as Item,
          r: skillRadius(skill),
        })),
    ];
    items.forEach((d, k) => {
      const angle = k * GOLDEN;
      const rr = spaceOf(a) * Math.sqrt(k + 0.5);
      dots.push({ ...d, x: spot.x + rr * Math.cos(angle), y: spot.y + rr * Math.sin(angle) });
    });
    clusters.push({
      key: a.key,
      name: a.name,
      x: spot.x,
      y: spot.y,
      r: clusterRadius(items.length, spaceOf(a)),
      lessons: a.key === SKILLS ? a.skills.filter(needsALook).length : a.lessons.length,
    });
  }
  const bounds = clusters.reduce<[number, number, number, number]>(
    (b, c) => [Math.min(b[0], c.x - c.r - PAD), Math.min(b[1], c.y - c.r - PAD), Math.max(b[2], c.x + c.r + PAD), Math.max(b[3], c.y + c.r + PAD + 30)],
    [Infinity, Infinity, -Infinity, -Infinity],
  );
  return { clusters, dots, bounds };
}
