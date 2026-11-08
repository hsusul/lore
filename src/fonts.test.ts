// @vitest-environment node
import postcss from "postcss";
import { describe, expect, it } from "vitest";

import stylesCss from "./styles.css?raw";
import tokensCss from "./tokens.css?raw";

/** The @font-face rules that styles.css pulls in through package imports, with their weights and sources. */
async function importedFaces(): Promise<{ family: string; weight: string; src: string }[]> {
  const specs: string[] = [];
  postcss.parse(stylesCss).walkAtRules("import", (rule) => {
    const spec = rule.params.replace(/^["']|["']$/g, "");
    if (!spec.startsWith(".")) specs.push(spec);
  });
  const faces: { family: string; weight: string; src: string }[] = [];
  for (const spec of specs) {
    const { default: css } = (await import(/* @vite-ignore */ `${spec}?raw`)) as { default: string };
    postcss.parse(css).walkAtRules("font-face", (rule) => {
      const face = { family: "", weight: "", src: "" };
      rule.walkDecls((d) => {
        if (d.prop === "font-family") face.family = d.value.replace(/^["']|["']$/g, "");
        if (d.prop === "font-weight") face.weight = d.value;
        if (d.prop === "src") face.src = d.value;
      });
      faces.push(face);
    });
  }
  return faces;
}

describe("bundled fonts", () => {
  it("bundles Figtree, the Mist face, at the weights Mist uses and no others", async () => {
    const family = /--mist-sans:\s*"([^"]+)"/.exec(tokensCss)?.[1];
    expect(family).toBe("Figtree");
    const faces = (await importedFaces()).filter((f) => f.family === family);
    expect([...new Set(faces.map((f) => f.weight))].sort()).toEqual(["400", "500", "600"]);
  });

  it("loads every bundled face from a local file, never the network", async () => {
    const urls = (await importedFaces()).flatMap((f) => [...f.src.matchAll(/url\(([^)]+)\)/g)].map((m) => m[1]));
    expect(urls.length).toBeGreaterThan(0);
    expect(urls.filter((u) => !u.startsWith("./files/"))).toEqual([]);
  });
});
