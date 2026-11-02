// @vitest-environment node
import postcss from "postcss";
import tailwindcss from "tailwindcss";
import { describe, expect, it } from "vitest";

import config from "../tailwind.config";

/** The CSS Tailwind emits for exactly these classes under Lore's theme. */
async function compile(classes: string): Promise<string> {
  const result = await postcss([tailwindcss({ ...config, content: [{ raw: `<div class="${classes}"></div>` }] })]).process(
    "@tailwind utilities;",
    { from: undefined },
  );
  return result.css;
}

describe("Tailwind theme", () => {
  it("emits nothing for stock Tailwind values", async () => {
    const css = await compile("bg-blue-500 text-red-600 bg-white p-7 m-12 rounded-xl rounded-md shadow-lg text-2xl text-sm font-medium font-light max-w-md sm:flex md:block ease-in-out duration-300 bg-label/50");
    expect(css.trim()).toBe("");
  });

  it("maps token classes onto the tokens in tokens.css", async () => {
    const css = await compile("bg-bg-sidebar text-label-2 p-2.5 rounded-control text-headline shadow-focus h-toolbar");
    expect(css).toContain("background-color: var(--bg-sidebar)");
    expect(css).toContain("color: var(--label-2)");
    expect(css).toContain("padding: var(--s-10)");
    expect(css).toContain("border-radius: var(--r-control)");
    expect(css).toContain("font-size: var(--t-headline-size)");
    expect(css).toContain("line-height: var(--t-headline-line)");
    expect(css).toContain("font-weight: 600");
    expect(css).toContain("--tw-shadow: 0 0 0 var(--focus-w) var(--focus-ring)");
    expect(css).toContain("height: var(--toolbar-h)");
  });

  it("styles Radix state attributes through named variants", async () => {
    const css = await compile("data-highlighted:bg-selected data-open:animate-popover-in group-data-highlighted:text-on-selected");
    expect(css).toContain("[data-highlighted]");
    expect(css).toContain('[data-state~="open"]');
    expect(css).toContain(".group[data-highlighted]");
  });
});
