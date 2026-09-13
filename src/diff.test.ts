import { describe, expect, it } from "vitest";

import { parseUnifiedDiff } from "./diff";

const MODIFIED = `diff --git a/src/a.rs b/src/a.rs
index 1111111..2222222 100644
--- a/src/a.rs
+++ b/src/a.rs
@@ -1,4 +1,5 @@ fn main() {
 one
-two
+TWO
+two and a half
 three
 four
@@ -10,2 +11,2 @@
 ten
-eleven
+ELEVEN
`;

describe("parseUnifiedDiff", () => {
  it("parses hunks with old and new line numbers", () => {
    const [file] = parseUnifiedDiff(MODIFIED);
    expect(file.path).toBe("src/a.rs");
    expect(file.status).toBe("modified");
    expect(file.additions).toBe(3);
    expect(file.deletions).toBe(2);
    expect(file.hunks).toHaveLength(2);

    const [first, second] = file.hunks;
    expect(first.header).toBe("@@ -1,4 +1,5 @@ fn main() {");
    expect(first.lines.map((l) => [l.type, l.oldNo, l.newNo, l.text])).toEqual([
      ["context", 1, 1, "one"],
      ["del", 2, null, "two"],
      ["add", null, 2, "TWO"],
      ["add", null, 3, "two and a half"],
      ["context", 3, 4, "three"],
      ["context", 4, 5, "four"],
    ]);
    expect(second.lines.map((l) => [l.type, l.oldNo, l.newNo])).toEqual([
      ["context", 10, 11],
      ["del", 11, null],
      ["add", null, 12],
    ]);
  });

  it("recognizes new, deleted, renamed, and binary files", () => {
    const text = [
      "diff --git a/new.txt b/new.txt",
      "new file mode 100644",
      "--- /dev/null",
      "+++ b/new.txt",
      "@@ -0,0 +1,2 @@",
      "+hello",
      "+world",
      "\\ No newline at end of file",
      "diff --git a/old.txt b/old.txt",
      "deleted file mode 100644",
      "--- a/old.txt",
      "+++ /dev/null",
      "@@ -1 +0,0 @@",
      "-bye",
      "diff --git a/x.ts b/y.ts",
      "similarity index 100%",
      "rename from x.ts",
      "rename to y.ts",
      "diff --git a/logo.png b/logo.png",
      "Binary files a/logo.png and b/logo.png differ",
    ].join("\n");
    const files = parseUnifiedDiff(text);
    expect(files.map((f) => [f.path, f.status, f.binary])).toEqual([
      ["new.txt", "added", false],
      ["old.txt", "deleted", false],
      ["y.ts", "renamed", false],
      ["logo.png", "modified", true],
    ]);
    expect(files[0].hunks[0].lines.map((l) => [l.type, l.newNo])).toEqual([
      ["add", 1],
      ["add", 2],
      ["meta", null],
    ]);
    expect(files[1].hunks[0].lines[0]).toEqual({ type: "del", text: "bye", oldNo: 1, newNo: null });
    expect(files[2].oldPath).toBe("x.ts");
  });

  it("keeps what it can from truncated input", () => {
    const cut = MODIFIED.slice(0, MODIFIED.indexOf("+two and") + 6);
    const files = parseUnifiedDiff(cut);
    expect(files).toHaveLength(1);
    const lines = files[0].hunks[0].lines;
    expect(lines[lines.length - 1]).toEqual({ type: "add", text: "two a", oldNo: null, newNo: 3 });
    expect(parseUnifiedDiff("")).toEqual([]);
  });

  it("treats +++/--- lines inside a hunk as content, not headers", () => {
    const text = ["--- a/f", "+++ b/f", "@@ -1,1 +1,1 @@", "---- old", "+++ new"].join("\n");
    const [file] = parseUnifiedDiff(text);
    expect(file.hunks[0].lines.map((l) => [l.type, l.text])).toEqual([
      ["del", "--- old"],
      ["add", "++ new"],
    ]);
  });
});
