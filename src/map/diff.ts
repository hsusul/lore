/** One line of a line diff: kept, removed from the old text, or added. */
export interface Line {
  op: "=" | "-" | "+";
  text: string;
}

/** The line diff from `before` to `after`, by longest common subsequence. */
export function lineDiff(before: string, after: string): Line[] {
  const a = before.split("\n");
  const b = after.split("\n");
  const n = a.length;
  const m = b.length;
  // lcs[i][j]: the longest common run of a[i..] and b[j..].
  const lcs = Array.from({ length: n + 1 }, () => new Array<number>(m + 1).fill(0));
  for (let i = n - 1; i >= 0; i--) {
    for (let j = m - 1; j >= 0; j--) {
      lcs[i][j] = a[i] === b[j] ? lcs[i + 1][j + 1] + 1 : Math.max(lcs[i + 1][j], lcs[i][j + 1]);
    }
  }
  const out: Line[] = [];
  let i = 0;
  let j = 0;
  while (i < n || j < m) {
    if (i < n && j < m && a[i] === b[j]) {
      out.push({ op: "=", text: a[i] });
      i++;
      j++;
    } else if (j < m && (i === n || lcs[i][j + 1] > lcs[i + 1][j])) {
      out.push({ op: "+", text: b[j] });
      j++;
    } else {
      out.push({ op: "-", text: a[i] });
      i++;
    }
  }
  return out;
}

/** The changed lines with `context` kept lines around each, and `null` where kept lines were left out. */
export function hunks(lines: Line[], context = 1): (Line | null)[] {
  const near = lines.map((_, k) => lines.slice(Math.max(0, k - context), k + context + 1).some((l) => l.op !== "="));
  const out: (Line | null)[] = [];
  lines.forEach((line, k) => {
    if (near[k]) out.push(line);
    else if (out.length > 0 && out[out.length - 1] !== null) out.push(null);
  });
  if (out[out.length - 1] === null) out.pop();
  return out;
}
