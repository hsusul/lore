import { describe, expect, it } from "vitest";

import { agentLabel, clamp, formatRelative, formatTime, formatTokens } from "./format";

describe("format helpers", () => {
  describe("agentLabel", () => {
    it("maps known agent ids to friendly display names", () => {
      expect(agentLabel("claude-code")).toBe("Claude");
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

  describe("formatRelative", () => {
    it("returns empty string for null and non-finite values", () => {
      expect(formatRelative(null)).toBe("");
      expect(formatRelative(NaN)).toBe("");
      expect(formatRelative(Infinity)).toBe("");
      expect(formatRelative(-Infinity)).toBe("");
    });

    it("handles future timestamps or clock skew gracefully without negative numbers", () => {
      const now = Date.now();
      expect(formatRelative(now + 10_000)).toBe("just now");
      expect(formatRelative(now + 60_000)).toBe("just now");
      const futureDate = formatRelative(now + 100 * 24 * 60 * 60 * 1000);
      expect(futureDate).not.toBe("just now");
      expect(futureDate.length).toBeGreaterThan(0);
    });

    it("formats recent seconds as 'just now'", () => {
      const now = Date.now();
      expect(formatRelative(now - 10_000)).toBe("just now");
      expect(formatRelative(now - 40_000)).toBe("just now");
    });

    it("formats minutes, hours, days, and weeks", () => {
      const now = Date.now();
      expect(formatRelative(now - 5 * 60 * 1000)).toBe("5m");
      expect(formatRelative(now - 3 * 60 * 60 * 1000)).toBe("3h");
      expect(formatRelative(now - 2 * 24 * 60 * 60 * 1000)).toBe("2d");
      expect(formatRelative(now - 14 * 24 * 60 * 60 * 1000)).toBe("2w");
    });
  });

  describe("formatTokens", () => {
    it("formats small and large token counts compactly", () => {
      expect(formatTokens(null)).toBe("");
      expect(formatTokens(0)).toBe("");
      expect(formatTokens(500)).toBe("500");
      expect(formatTokens(1_500)).toBe("1.5k");
      expect(formatTokens(24_000)).toBe("24k");
      expect(formatTokens(1_200_000)).toBe("1.2M");
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
