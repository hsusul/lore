// @vitest-environment node
import { describe, expect, it } from "vitest";

/**
 * Spec 2.6 and 17.24: components take every color, size, space and radius from a token.
 * A hex color anywhere, or a px literal or arbitrary Tailwind value (`p-[13px]`,
 * `data-[x]:`, `[&>svg]:`) in a string, in a component file fails this test. No file
 * under src/components/ui needs an exception: Radix takes its offsets as numbers.
 */
const HEX = /(?<![\w&])#(?:[0-9a-f]{8}|[0-9a-f]{6}|[0-9a-f]{3,4})\b/gi;
const STRING = /"[^"\n]*"|'[^'\n]*'|`[^`]*`/g;
const PX = /\b\d+(?:\.\d+)?px\b/g;
// A class token with a bracketed part. Backslashes mark a regex that happens to sit between backticks.
const BRACKETED_CLASS = /^[^\s\\]*\[[^\]\\]*\][^\s\\]*$/;

export function findViolations(source: string): string[] {
  const found = [...source.matchAll(HEX)].map((m) => `hex color: ${m[0]}`);
  for (const literal of source.match(STRING) ?? []) {
    for (const m of literal.matchAll(PX)) found.push(`px literal: ${m[0]}`);
    for (const token of literal.slice(1, -1).split(/\s+/)) {
      if (BRACKETED_CLASS.test(token)) found.push(`arbitrary Tailwind value: ${token}`);
    }
  }
  return found;
}

const SOURCES: Record<string, string> = import.meta.glob(["./components/**/*.{ts,tsx}", "!./components/**/*.test.{ts,tsx}", "./App.tsx"], {
  query: "?raw",
  import: "default",
  eager: true,
});

describe("design guards", () => {
  it("catches each kind of raw value", () => {
    expect(findViolations('<div style={{ color: "#0064e1" }} />')).toEqual(["hex color: #0064e1"]);
    expect(findViolations('style={{ width: "13px" }}')).toEqual(["px literal: 13px"]);
    expect(findViolations('className="p-[13px] text-label"')).toEqual(["px literal: 13px", "arbitrary Tailwind value: p-[13px]"]);
    expect(findViolations('className="w-[--sidebar]"')).toEqual(["arbitrary Tailwind value: w-[--sidebar]"]);
    expect(findViolations('className="data-[state=open]:bg-fill"')).toEqual(["arbitrary Tailwind value: data-[state=open]:bg-fill"]);
    expect(findViolations("className={clsx(`[&>svg]:size-icon`)}")).toEqual(["arbitrary Tailwind value: [&>svg]:size-icon"]);
  });

  it("passes token classes, ids, and array indexing", () => {
    expect(findViolations('className="p-2.5 bg-bg-sidebar data-open:animate-popover-in" items[index] }, [scope]); const [a] = b;')).toEqual([]);
  });

  it("finds no raw values in components or App.tsx", () => {
    const files = Object.keys(SOURCES);
    const report = files.flatMap((file) => findViolations(SOURCES[file]).map((v) => `${file}: ${v}`));
    expect(files).toContain("./App.tsx");
    expect(files).toContain("./components/ui/button.tsx");
    expect(files.some((file) => file.endsWith(".test.tsx"))).toBe(false);
    expect(report).toEqual([]);
  });
});
