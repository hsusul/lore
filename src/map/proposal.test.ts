// @vitest-environment node
import { describe, expect, it } from "vitest";

import { receipts, why } from "./proposal";

const body = `The agent pushed without formatting, and the person wanted \`cargo fmt\` run first.

Receipts:
- "always run cargo fmt before pushing" (claude-code, session 0193a, 2026-10-12)
- "you forgot fmt again" (codex, session 0194c, 2026-10-20)
not a receipt
`;

describe("proposal body", () => {
  it("reads what went wrong from before the receipts", () => {
    expect(why(body)).toBe("The agent pushed without formatting, and the person wanted `cargo fmt` run first.");
    expect(why("Only a sentence.\n")).toBe("Only a sentence.");
  });

  it("reads each quoted receipt and where it was said", () => {
    expect(receipts(body)).toEqual([
      { said: "always run cargo fmt before pushing", where: "claude-code, session 0193a, 2026-10-12" },
      { said: "you forgot fmt again", where: "codex, session 0194c, 2026-10-20" },
    ]);
    expect(receipts("No receipts here.")).toEqual([]);
  });
});
