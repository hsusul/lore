const MAX_LINES = 1_000;

type LineKind = "file" | "hunk" | "add" | "del" | "context";

interface DiffLine {
  kind: LineKind;
  text: string;
  oldNo: number | null;
  newNo: number | null;
}

const HUNK = /^@@ -(\d+)(?:,\d+)? \+(\d+)(?:,\d+)? @@/;
const FILE_HEADER = /^(diff --git |index |\+\+\+ |--- |\*\*\* (Begin|End|Update|Add|Delete) )/;

/** True when text looks like a unified diff or an apply_patch envelope. */
export function looksLikePatch(text: string): boolean {
  return /^(@@ |diff --git |\*\*\* Begin Patch|--- \S)/m.test(text);
}

/** Classify each line and carry old/new line numbers through unified hunks. */
export function parseDiff(text: string): DiffLine[] {
  let oldNo: number | null = null;
  let newNo: number | null = null;
  return text
    .replace(/\n$/, "")
    .split("\n")
    .map((line): DiffLine => {
      const hunk = HUNK.exec(line);
      if (hunk) {
        oldNo = Number(hunk[1]);
        newNo = Number(hunk[2]);
        return { kind: "hunk", text: line, oldNo: null, newNo: null };
      }
      if (line.startsWith("@@")) return { kind: "hunk", text: line, oldNo: null, newNo: null };
      if (line.startsWith("diff --git ") || line.startsWith("*** ")) {
        oldNo = null;
        newNo = null;
      }
      // Inside a numbered hunk, `--- x` is a removed line that began with `--`.
      if (oldNo === null && FILE_HEADER.test(line)) {
        return { kind: "file", text: line, oldNo: null, newNo: null };
      }
      if (line.startsWith("+")) {
        const row = { kind: "add" as const, text: line, oldNo: null, newNo };
        if (newNo !== null) newNo += 1;
        return row;
      }
      if (line.startsWith("-")) {
        const row = { kind: "del" as const, text: line, oldNo, newNo: null };
        if (oldNo !== null) oldNo += 1;
        return row;
      }
      const row = { kind: "context" as const, text: line, oldNo, newNo };
      if (oldNo !== null) oldNo += 1;
      if (newNo !== null) newNo += 1;
      return row;
    });
}

/** A patch with a line-number gutter and add/remove tinting. */
export default function Diff({ text, label = "Patch" }: { text: string; label?: string }) {
  const all = parseDiff(text);
  const lines = all.slice(0, MAX_LINES);
  const hidden = all.length - lines.length;
  return (
    <figure className="diff" aria-label={label}>
      <table className="diff__table">
        <tbody>
          {lines.map((line, index) => (
            <tr key={index} className={`diff__line diff__line--${line.kind}`}>
              <td className="diff__no" aria-hidden="true">
                {line.oldNo ?? ""}
              </td>
              <td className="diff__no" aria-hidden="true">
                {line.newNo ?? ""}
              </td>
              <td className="diff__code">{line.text || " "}</td>
            </tr>
          ))}
        </tbody>
      </table>
      {hidden > 0 && <figcaption className="diff__more">{hidden} more lines not shown</figcaption>}
    </figure>
  );
}
