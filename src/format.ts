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

/** A clock time ("2:06 PM"). */
export function formatClock(ms: number): string {
  return new Date(ms).toLocaleTimeString(undefined, { hour: "numeric", minute: "2-digit" });
}

/** A day and time as Mail's message header writes it: "Today at 2:00 PM", "Yesterday at …", "Oct 19 at …". */
export function formatDayAndTime(ms: number, now: number = Date.now()): string {
  const days = Math.round((startOfDay(now) - startOfDay(ms)) / DAY_MS);
  const day =
    days === 0
      ? "Today"
      : days === 1
        ? "Yesterday"
        : new Date(ms).toLocaleDateString(undefined, {
            month: "short",
            day: "numeric",
            year: new Date(ms).getFullYear() === new Date(now).getFullYear() ? undefined : "numeric",
          });
  return `${day} at ${formatClock(ms)}`;
}

/** A session's length in words ("38 min", "2 hr 5 min"), or null when it is unknown. */
export function formatDuration(start: number | null, end: number | null): string | null {
  if (start == null || end == null || end <= start) return null;
  const minutes = Math.round((end - start) / 60_000);
  if (minutes < 1) return "under a minute";
  if (minutes < 60) return `${minutes} min`;
  const rest = minutes % 60;
  return rest === 0 ? `${Math.floor(minutes / 60)} hr` : `${Math.floor(minutes / 60)} hr ${rest} min`;
}

/** "1 file", "3 files". */
export function plural(n: number, one: string, many: string = `${one}s`): string {
  return `${n.toLocaleString()} ${n === 1 ? one : many}`;
}

const AGO_STEPS:[Intl.RelativeTimeFormatUnit, number][] = [
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
