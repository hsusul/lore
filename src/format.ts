// Small shared formatting helpers for the UI.

const AGENT_LABELS: Record<string, string> = {
  "claude-code": "Claude",
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
