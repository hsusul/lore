// Small shared formatting helpers for the UI.

const AGENT_LABELS: Record<string, string> = {
  "claude-code": "Claude Code",
  codex: "Codex",
};

/** Clamp a number between min and max bounds. */
export function clamp(value: number, min: number, max: number): number {
  if (!Number.isFinite(value)) return min;
  return Math.min(Math.max(value, min), max);
}

/** Short, human agent label (falls back to the raw id). */
export function agentLabel(agentId: string): string {
  return AGENT_LABELS[agentId] ?? agentId;
}

/** Absolute local time, or "" for null / non-finite timestamps. */
export function formatTime(ms: number | null): string {
  if (ms == null || !Number.isFinite(ms)) return "";
  return new Date(ms).toLocaleString(undefined, {
    year: "numeric",
    month: "short",
    day: "numeric",
    hour: "2-digit",
    minute: "2-digit",
  });
}

function formatShortDate(ms: number): string {
  return new Date(ms).toLocaleDateString(undefined, {
    month: "short",
    day: "numeric",
  });
}

/** Compact relative time ("just now", "5m", "3h", "2d", "4w", or a date). */
export function formatRelative(ms: number | null): string {
  if (ms == null || !Number.isFinite(ms)) return "";
  const diff = Date.now() - ms;
  if (diff < -60_000) {
    return formatShortDate(ms);
  }
  if (diff <= 45_000) return "just now";
  const seconds = Math.round(diff / 1000);
  const minutes = Math.round(seconds / 60);
  if (minutes < 60) return `${minutes}m`;
  const hours = Math.round(minutes / 60);
  if (hours < 24) return `${hours}h`;
  const days = Math.round(hours / 24);
  if (days < 7) return `${days}d`;
  const weeks = Math.round(days / 7);
  if (weeks < 5) return `${weeks}w`;
  return formatShortDate(ms);
}

const DAY_MS = 86_400_000;

function startOfDay(ms: number): number {
  const d = new Date(ms);
  return new Date(d.getFullYear(), d.getMonth(), d.getDate()).getTime();
}

/**
 * A date as Mail's message list writes it (spec 9.2): the time today ("2:00 PM"),
 * "Yesterday", the weekday within the past week, then a short date ("10/19/26").
 */
export function formatMailDate(ms: number | null, now: number = Date.now()): string {
  if (ms == null || !Number.isFinite(ms)) return "";
  const days = Math.round((startOfDay(now) - startOfDay(ms)) / DAY_MS);
  if (days === 0) return new Date(ms).toLocaleTimeString(undefined, { hour: "numeric", minute: "2-digit" });
  if (days === 1) return "Yesterday";
  if (days > 1 && days < 7) return new Date(ms).toLocaleDateString(undefined, { weekday: "long" });
  return new Date(ms).toLocaleDateString(undefined, { month: "numeric", day: "numeric", year: "2-digit" });
}

const AGO_STEPS: [Intl.RelativeTimeFormatUnit, number][] = [
  ["hour", 60],
  ["day", 24],
  ["week", 7],
  ["month", 4.35],
  ["year", 12],
];

/** Relative time in words for tooltips ("just now", "2 minutes ago", "yesterday"). */
export function formatAgo(ms: number, now: number = Date.now()): string {
  let value = (now - ms) / 60_000;
  if (value < 1) return "just now";
  let unit: Intl.RelativeTimeFormatUnit = "minute";
  for (const [next, size] of AGO_STEPS) {
    if (value / size < 1) break;
    value /= size;
    unit = next;
  }
  return new Intl.RelativeTimeFormat(undefined, { numeric: "auto" }).format(-Math.round(value), unit);
}

/** Compact token count formatting (e.g. 1.2k, 45k, 1.4M). */
export function formatTokens(tokens: number | null): string {
  if (tokens == null || !Number.isFinite(tokens) || tokens <= 0) return "";
  if (tokens < 1_000) return `${tokens}`;
  if (tokens < 1_000_000) return `${(tokens / 1_000).toFixed(1).replace(/\.0$/, "")}k`;
  return `${(tokens / 1_000_000).toFixed(1).replace(/\.0$/, "")}M`;
}
