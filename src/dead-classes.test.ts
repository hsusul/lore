// @vitest-environment node
import postcss from "postcss";
import tailwindcss from "tailwindcss";
import { describe, expect, it } from "vitest";

import config from "../tailwind.config";
import legacyCss from "./styles.css?raw";

/**
 * Lore's theme drops every stock Tailwind value, so a utility outside the tokens
 * (`left-1.5` before inset had a spacing scale, `rounded-md`) compiles to nothing and
 * fails silently. This finds utility-looking class tokens in component strings that
 * emit no CSS. Class names defined in styles.css are the legacy component rules.
 */
const SOURCES: Record<string, string> = import.meta.glob(["./components/**/*.{ts,tsx}", "!./components/**/*.test.{ts,tsx}", "./App.tsx"], {
  query: "?raw",
  import: "default",
  eager: true,
});

const UTILITY =
  /^(?:[\w-]+(?:\/\w+)?:)*-?(?:bg|text|p[xytrbl]?|m[xytrbl]?|gap|w|h|size|min-w|min-h|max-w|max-h|rounded|shadow|border|grid|col|row|items|justify|self|inset|top|left|right|bottom|z|font|leading|opacity|animate|duration|ease|transition|rotate|stroke|caret|auto-cols|select|whitespace|overflow|pointer-events|touch|flex|shrink|grow|truncate|outline|tabular-nums)(?:-|$)/;
// Bare words count only when they are utilities themselves; "bottom" is a Radix side prop.
const STANDALONE = new Set(["flex", "grid", "truncate", "border", "grow", "shrink"]);
const STRING = /"[^"\n]*"|'[^'\n]*'|`[^`]*`/g;
const legacy = new Set([...legacyCss.matchAll(/\.([a-z][\w-]*)/g)].map((m) => m[1]));

function candidates(source: string): string[] {
  return (source.match(STRING) ?? [])
    .flatMap((literal) => literal.slice(1, -1).split(/\s+/))
    .filter((token) => UTILITY.test(token) && (/[-:]/.test(token) || STANDALONE.has(token)))
    .filter((token) => !legacy.has(token) && !token.includes("${"));
}

async function emitted(classes: string[]): Promise<string> {
  const result = await postcss([tailwindcss({ ...config, content: [{ raw: classes.join(" ") }] })]).process(
    "@tailwind components; @tailwind utilities;",
    { from: undefined },
  );
  return result.css;
}

const selector = (token: string) => `.${token.replace(/([:./])/g, "\\$1")}`;

describe("component classes", () => {
  it("flags a utility the theme does not define", async () => {
    const css = await emitted(["left-1.5", "rounded-md", "rounded-control"]);
    expect(["left-1.5", "rounded-md", "rounded-control"].filter((t) => !css.includes(selector(t)))).toEqual(["rounded-md"]);
  });

  it("all compile to CSS", async () => {
    const all = [...new Set(Object.values(SOURCES).flatMap(candidates))];
    const css = await emitted(all);
    expect(all.length).toBeGreaterThan(100);
    expect(all.filter((token) => !css.includes(selector(token)))).toEqual([]);
  });
});
