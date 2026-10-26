// @vitest-environment node
import postcss, { type AtRule, type Rule } from "postcss";
import { describe, expect, it } from "vitest";

import tokensCss from "./tokens.css?raw";

/**
 * WCAG 2.2 contrast for every text and background pair the tokens are drawn in (spec 13),
 * in light, dark, and Increase Contrast. Translucent tokens composite over the surface
 * they sit on. Values are the fallbacks; WebKit's keywords resolve to the same measured
 * colors. Ported from docs/design/bakeoff/final/tools/contrast.mjs.
 */
const root = postcss.parse(tokensCss);

type Tokens = Record<string, string>;

function declarations(rule: Rule): Tokens {
  const out: Tokens = {};
  rule.walkDecls((d) => {
    if (d.prop.startsWith("--")) out[d.prop.slice(2)] = d.value;
  });
  return out;
}

/** The rule with `selector` directly inside the at-rule chain `media` (outermost first). */
function block(selector: string, ...media: string[]): Tokens {
  let found: Rule | undefined;
  root.walkRules((rule) => {
    const chain: string[] = [];
    for (let p = rule.parent; p && p.type === "atrule"; p = p.parent) chain.unshift((p as AtRule).params);
    if (rule.selector === selector && chain.join("|") === media.join("|")) found = rule;
  });
  if (!found) throw new Error(`no ${selector} under ${media.join(" > ") || "the top level"}`);
  return declarations(found);
}

const DARK = "(prefers-color-scheme: dark)";
const MORE = "(prefers-contrast: more)";
const light = block(":root");
const dark = { ...light, ...block(':root:not([data-theme="light"])', DARK) };
const themes: [string, Tokens][] = [
  ["light", light],
  ["dark", dark],
  ["light, Increase Contrast", { ...light, ...block(':root,\n  :root:not([data-theme="light"])', MORE) }],
  ["dark, Increase Contrast", { ...dark, ...block(':root:not([data-theme="light"])', MORE, DARK) }],
];

type RGBA = [number, number, number, number];

function color(value: string, tokens: Tokens): RGBA {
  const ref = value.match(/^var\(--([\w-]+)\)$/);
  if (ref) return color(tokens[ref[1]], tokens);
  const hex = value.match(/^#([0-9a-f]{6})$/i);
  if (hex) return [0, 2, 4].map((i) => parseInt(hex[1].slice(i, i + 2), 16)).concat(1) as RGBA;
  const rgba = value.match(/^rgba?\(([^)]+)\)$/);
  if (rgba) {
    const [r, g, b, a = 1] = rgba[1].split(",").map(Number);
    return [r, g, b, a];
  }
  throw new Error(`cannot read color ${value}`);
}

const over = ([r, g, b, a]: RGBA, [R, G, B]: RGBA): RGBA => [r * a + R * (1 - a), g * a + G * (1 - a), b * a + B * (1 - a), 1];
const channel = (c: number) => ((c /= 255) <= 0.04045 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4);
const luminance = ([r, g, b]: RGBA) => 0.2126 * channel(r) + 0.7152 * channel(g) + 0.0722 * channel(b);

function ratio(fg: string, stack: string[], tokens: Tokens): number {
  let bg = color(tokens[stack[0]], tokens);
  for (const layer of stack.slice(1)) bg = over(color(tokens[layer], tokens), bg);
  const [hi, lo] = [luminance(over(color(tokens[fg], tokens), bg)), luminance(bg)].sort((p, q) => q - p);
  return (hi + 0.05) / (lo + 0.05);
}

const TEXT = 4.5;
const NON_TEXT = 3;
const content = ["bg-content"];

/** [what, foreground, background stack bottom to top, minimum] */
const PAIRS: [string, string, string[], number][] = [
  ["primary text on content", "label", content, TEXT],
  ["primary text on toolbar", "label", ["bg-toolbar"], TEXT],
  ["primary text on sidebar", "label", ["bg-sidebar"], TEXT],
  ["primary text on window", "label", ["bg-window"], TEXT],
  ["primary text on popover", "label", ["bg-popover"], TEXT],
  ["primary text on user turn", "label", ["bg-content", "fill-2"], TEXT],
  ["primary text on form group", "label", ["bg-content", "fill-3"], TEXT],
  ["primary text on token", "label", ["bg-content", "fill"], TEXT],
  ["typed text in search field", "label", ["bg-toolbar", "fill-2"], TEXT],
  ["primary text on unfocused selection", "label", ["selected-unfocused"], TEXT],
  ["search hit on content", "label", ["bg-content", "mark"], TEXT],
  ["code on added line", "label", ["bg-content", "diff-add-bg"], TEXT],
  ["code on deleted line", "label", ["bg-content", "diff-del-bg"], TEXT],
  ["secondary text on content", "label-2", content, TEXT],
  ["toolbar subtitle", "label-2", ["bg-toolbar"], TEXT],
  ["sidebar headers and counts", "label-2", ["bg-sidebar"], TEXT],
  ["secondary text on popover", "label-2", ["bg-popover"], TEXT],
  ["line numbers in code", "label-2", ["bg-content", "fill-2"], TEXT],
  ["line numbers on added line", "label-2", ["bg-content", "diff-add-bg"], TEXT],
  ["line numbers on deleted line", "label-2", ["bg-content", "diff-del-bg"], TEXT],
  ["form row detail", "label-2", ["bg-content", "fill-3"], TEXT],
  ["filter token key in search field", "token-key", ["bg-toolbar", "fill-2", "fill"], TEXT],
  ["text on focused selection", "on-selected", ["selected"], TEXT],
  ["default button label", "on-selected", ["accent-fill"], TEXT],
  ["destructive button label", "on-selected", ["red-fill"], TEXT],
  ["link on content", "link", content, TEXT],
  ["added count on content", "diff-add-fg", content, TEXT],
  ["deleted count on content", "diff-del-fg", content, TEXT],
  ["added sign on added line", "diff-add-fg", ["bg-content", "diff-add-bg"], TEXT],
  ["deleted sign on deleted line", "diff-del-fg", ["bg-content", "diff-del-bg"], TEXT],
  ["focus ring on content", "focus-ring", content, NON_TEXT],
  ["focus ring on toolbar", "focus-ring", ["bg-toolbar"], NON_TEXT],
  ["focus ring on sidebar", "focus-ring", ["bg-sidebar"], NON_TEXT],
  ["selected segment ring on its track", "seg-ring", ["bg-content", "fill-2"], NON_TEXT],
  ["toolbar icon on toolbar", "label-2", ["bg-toolbar"], NON_TEXT],
];

describe("token contrast", () => {
  for (const [theme, tokens] of themes) {
    it(`meets WCAG AA in ${theme}`, () => {
      const failing = PAIRS.filter(([, fg, stack, min]) => ratio(fg, stack, tokens) < min).map(
        ([what, fg, stack, min]) => `${what}: --${fg} on ${stack.join(" + ")} is ${ratio(fg, stack, tokens).toFixed(2)}:1, needs ${min}:1`,
      );
      expect(failing).toEqual([]);
    });
  }

  it("fails a pair that is known to be too faint", () => {
    expect(ratio("label-3", content, light)).toBeLessThan(TEXT);
    expect(ratio("label", content, light)).toBeGreaterThan(TEXT);
  });
});

describe("token blocks", () => {
  it("gives the Settings dark override the same values as the system dark appearance", () => {
    expect(block(':root[data-theme="dark"]')).toEqual(block(':root:not([data-theme="light"])', DARK));
    expect(block(':root[data-theme="dark"]', MORE)).toEqual(block(':root:not([data-theme="light"])', MORE, DARK));
  });
});
