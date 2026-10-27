import { describe, expect, it } from "vitest";

import { agentLabel, clamp, formatAgo, formatDayAndTime, formatDuration, formatMailDate, formatTime, plural } from "./format";

describe("format helpers", () => {
  describe("formatAgo", () => {
    const now = Date.UTC(2026, 9, 26, 12, 0, 0);

    it("says how long ago in words", () => {
      expect(formatAgo(now - 20_000, now)).toBe("just now");
      expect(formatAgo(now - 2 * 60_000, now)).toBe("2 minutes ago");
      expect(formatAgo(now - 3 * 3_600_000, now)).toBe("3 hours ago");
      expect(formatAgo(now - 26 * 3_600_000, now)).toBe("yesterday");
      expect(formatAgo(now - 15 * 86_400_000, now)).toBe("2 weeks ago");
    });
  });

  describe("formatMailDate", () => {
    const now = new Date(2026, 9, 26, 16, 30).getTime();

    it("writes dates the way Mail's message list does", () => {
      expect(formatMailDate(new Date(2026, 9, 26, 14, 0).getTime(), now)).toBe("2:00 PM");
      expect(formatMailDate(new Date(2026, 9, 25, 23, 59).getTime(), now)).toBe("Yesterday");
      expect(formatMailDate(new Date(2026, 9, 22, 9, 0).getTime(), now)).toBe("Thursday");
      expect(formatMailDate(new Date(2026, 9, 19, 9, 0).getTime(), now)).toBe("10/19/26");
      expect(formatMailDate(null, now)).toBe("");
    });
  });

  describe("agentLabel", () => {
    it("maps known agent ids to friendly display names", () => {
      expect(agentLabel("claude-code")).toBe("Claude Code");
      expect(agentLabel("codex")).toBe("Codex");
    });

    it("falls back to raw id for unknown agents", () => {
      expect(agentLabel("gemini-cli")).toBe("gemini-cli");
      expect(agentLabel("custom-agent")).toBe("custom-agent");
    });
  });

  describe("formatTime", () => {
    it("returns empty string for null and non-finite values", () => {
      expect(formatTime(null)).toBe("");
      expect(formatTime(NaN)).toBe("");
      expect(formatTime(Infinity)).toBe("");
      expect(formatTime(-Infinity)).toBe("");
    });

    it("formats a valid millisecond epoch timestamp", () => {
      const formatted = formatTime(1_700_000_000_000);
      expect(formatted).toBeTruthy();
      expect(typeof formatted).toBe("string");
    });
  });

  describe("reader dates and lengths", () => {
    const now = new Date(2026, 9, 26, 18, 0).getTime();

    it("writes the start as Mail's header does", () => {
      expect(formatDayAndTime(new Date(2026, 9, 26, 14, 0).getTime(), now)).toBe("Today at 2:00 PM");
      expect(formatDayAndTime(new Date(2026, 9, 25, 9, 5).getTime(), now)).toBe("Yesterday at 9:05 AM");
      expect(formatDayAndTime(new Date(2026, 9, 19, 14, 0).getTime(), now)).toBe("Oct 19 at 2:00 PM");
      expect(formatDayAndTime(new Date(2025, 9, 19, 14, 0).getTime(), now)).toBe("Oct 19, 2025 at 2:00 PM");
    });

    it("writes a session's length, or nothing when it is unknown", () => {
      expect(formatDuration(now, now + 38 * 60_000)).toBe("38 min");
      expect(formatDuration(now, now + 125 * 60_000)).toBe("2 hr 5 min");
      expect(formatDuration(now, now + 120 * 60_000)).toBe("2 hr");
      expect(formatDuration(now, now + 20_000)).toBe("under a minute");
      expect(formatDuration(now, null)).toBeNull();
    });

    it("counts with the right noun", () => {
      expect([plural(1, "file"), plural(1200, "match", "matches")]).toEqual(["1 file", "1,200 matches"]);
    });
  });

  describe("clamp", () => {
    it("clamps values within min and max boundaries", () => {
      expect(clamp(5, 1, 10)).toBe(5);
      expect(clamp(0, 1, 10)).toBe(1);
      expect(clamp(15, 1, 10)).toBe(10);
    });

    it("handles non-finite values by returning min", () => {
      expect(clamp(NaN, 1, 10)).toBe(1);
      expect(clamp(Infinity, 1, 10)).toBe(1);
    });
  });
});
