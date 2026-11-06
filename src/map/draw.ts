import { CENTRE, type Cluster, type Dot, type MapModel } from "./layout";

/** Map units to screen pixels: screen = map * s + t. */
export interface View {
  s: number;
  tx: number;
  ty: number;
}

export interface Palette {
  /** Lessons, and the selected dot's halo. */
  ink: string;
  /** Notes. */
  quiet: string;
  name: string;
  count: string;
  ground: string;
  font: string;
}

export interface Frame {
  width: number;
  height: number;
  /** Width the card covers on the right; the map fits in what is left. */
  inset: number;
}

const MIN_S = 0.35;
const MAX_S = 6;

export const clampScale = (s: number) => Math.max(MIN_S, Math.min(MAX_S, s));

/** The whole map, centred in the space left of the card. */
export function fit(model: MapModel, frame: Frame): View {
  const [left, top, right, bottom] = model.bounds;
  const w = Math.max(1, frame.width - frame.inset - 48);
  const h = Math.max(1, frame.height - 48);
  const s = clampScale(Math.min(1.6, w / (right - left), h / (bottom - top)));
  const cx = (left + right) / 2;
  const cy = (top + bottom) / 2;
  return { s, tx: (frame.width - frame.inset) / 2 - cx * s, ty: frame.height / 2 - cy * s };
}

/** Zoomed onto one cluster, in the space left of the card. */
export function onto(cluster: Cluster, home: View, frame: Frame): View {
  const s = clampScale(Math.max(2.25, home.s * 1.8, Math.min(MAX_S, (frame.height * 0.32) / Math.max(cluster.r, 1))));
  return { s, tx: (frame.width - frame.inset) / 2 - cluster.x * s, ty: frame.height / 2 - cluster.y * s };
}

/** Zoom by `factor` about the screen point (x, y). */
export function zoomAbout(view: View, factor: number, x: number, y: number): View {
  const s = clampScale(view.s * factor);
  const k = s / view.s;
  return { s, tx: x - (x - view.tx) * k, ty: y - (y - view.ty) * k };
}

const dotRadius = (d: Dot, s: number) => d.r * (s < 1 ? s * 0.9 : Math.sqrt(s));

/** The dot under the screen point, if any. */
export function dotAt(model: MapModel, view: View, x: number, y: number): Dot | null {
  let best: Dot | null = null;
  let bestD = Infinity;
  for (const d of model.dots) {
    const dist = Math.hypot(d.x * view.s + view.tx - x, d.y * view.s + view.ty - y);
    if (dist < Math.max(9, dotRadius(d, view.s) + 4) && dist < bestD) {
      best = d;
      bestD = dist;
    }
  }
  return best;
}

/** The cluster under the screen point, its name included. */
export function clusterAt(model: MapModel, view: View, x: number, y: number): Cluster | null {
  const mx = (x - view.tx) / view.s;
  const my = (y - view.ty) / view.s;
  return model.clusters.find((c) => Math.hypot(mx - c.x, my - c.y) <= c.r + 24) ?? null;
}

const short = (text: string, n: number) => (text.length > n ? `${text.slice(0, n - 1).trimEnd()}…` : text);

/**
 * Draw the map. Far out, each cluster shows its name and how many lessons wait
 * in it; close in, names give way to the names of the dots themselves.
 */
export function draw(
  g: CanvasRenderingContext2D,
  model: MapModel,
  view: View,
  frame: Frame,
  home: View,
  palette: Palette,
  selectedId: string | null,
  focus: string | null,
): void {
  const { s, tx, ty } = view;
  const near = s > home.s * 1.35;
  const far = s < home.s * 1.15;
  g.clearRect(0, 0, frame.width, frame.height);
  const visible = (x: number, y: number) => x > -20 && x < frame.width + 20 && y > -20 && y < frame.height + 20;

  const labels: { d: Dot; x: number; y: number; r: number; selected: boolean }[] = [];
  for (const d of model.dots) {
    const x = d.x * s + tx;
    const y = d.y * s + ty;
    if (!visible(x, y)) continue;
    const r = dotRadius(d, s);
    const selected = d.id === selectedId;
    const dim = focus && d.area !== focus ? 0.35 : 1;
    if (selected) {
      g.globalAlpha = 0.22;
      g.fillStyle = palette.ink;
      g.beginPath();
      g.arc(x, y, r + 9, 0, 2 * Math.PI);
      g.fill();
    }
    // A skill is ink when the person re-prompted after it, faint when it never ran.
    const ink = d.item.kind === "lesson" || (d.item.kind === "skill" && d.item.skill.reprompts.length > 0);
    const faint = d.item.kind === "skill" && d.item.skill.uses === 0;
    g.globalAlpha = dim * (faint ? 0.35 : 1);
    g.fillStyle = ink ? palette.ink : palette.quiet;
    g.beginPath();
    g.arc(x, y, selected ? r + 1.5 : r, 0, 2 * Math.PI);
    g.fill();
    if (selected || (near && (!focus || d.area === focus))) labels.push({ d, x, y, r, selected });
  }

  g.textBaseline = "middle";
  g.textAlign = "center";
  if (!near) {
    for (const c of model.clusters) {
      const x = c.x * s + tx;
      const y = c.y * s + ty + c.r * s + 5 * Math.sqrt(s) + 16;
      if (!visible(x, y)) continue;
      const size = c.key === CENTRE ? 15 : 13;
      g.globalAlpha = focus && c.key !== focus ? 0.4 : 0.9;
      g.fillStyle = palette.name;
      g.font = `500 ${size}px ${palette.font}`;
      g.fillText(c.name, x, y);
      if (far && c.lessons > 0) {
        g.font = `400 ${11}px ${palette.font}`;
        g.fillStyle = palette.count;
        g.fillText(String(c.lessons), x, y + 15);
      }
    }
  }

  // Selected first, then lessons, then notes; a label that would cover another, or a dot, is left out.
  const placed: { x: number; y: number; w: number; h: number }[] = [];
  const overlaps = (a: (typeof placed)[number], b: (typeof placed)[number]) =>
    a.x < b.x + b.w && b.x < a.x + a.w && a.y < b.y + b.h && b.y < a.y + a.h;
  labels.sort((a, b) => Number(b.selected) - Number(a.selected) || Number(b.d.item.kind === "lesson") - Number(a.d.item.kind === "lesson"));
  g.textAlign = "left";
  g.lineJoin = "round";
  g.lineWidth = 4;
  for (const { d, x, y, r, selected } of labels) {
    g.font = `${selected ? 500 : 400} ${selected ? 13 : 12}px ${palette.font}`;
    const text = short(d.label, selected ? 40 : 28);
    const w = g.measureText(text).width;
    const right = { x: x + r + 8, y: y - 8, w, h: 16 };
    const left = { x: x - r - 8 - w, y: y - 8, w, h: 16 };
    const box = [right, left].find((b) => b.x > 8 && b.x + b.w < frame.width - frame.inset - 8 && !placed.some((p) => overlaps(p, b)));
    if (!box && !selected) continue;
    const b = box ?? right;
    placed.push(b);
    g.globalAlpha = selected ? 1 : 0.9;
    g.strokeStyle = palette.ground;
    g.strokeText(text, b.x, y);
    g.fillStyle = selected ? palette.name : palette.count;
    g.fillText(text, b.x, y);
  }
  g.globalAlpha = 1;
}
