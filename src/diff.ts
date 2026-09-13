// Parse `git diff` unified output into files, hunks, and numbered lines for
// the diff view. Tolerant of truncated input: a cut-off hunk simply ends early.

export type DiffLineType = "context" | "add" | "del" | "meta";

export type DiffLine = {
  type: DiffLineType;
  text: string;
  oldNo: number | null;
  newNo: number | null;
};

export type DiffHunk = {
  header: string;
  lines: DiffLine[];
};

export type DiffFileStatus = "added" | "deleted" | "modified" | "renamed";

export type DiffFile = {
  oldPath: string | null;
  newPath: string | null;
  /** The path to show: the new path, or the old one for deletions. */
  path: string;
  status: DiffFileStatus;
  binary: boolean;
  hunks: DiffHunk[];
  additions: number;
  deletions: number;
};

const HUNK_RE = /^@@ -(\d+)(?:,(\d+))? \+(\d+)(?:,(\d+))? @@/;

function stripPrefix(path: string): string | null {
  const trimmed = path.replace(/\t.*$/, "").trim();
  if (trimmed === "/dev/null") return null;
  if (trimmed.startsWith("a/") || trimmed.startsWith("b/")) return trimmed.slice(2);
  return trimmed;
}

function newFile(oldPath: string | null, newPath: string | null): DiffFile {
  return {
    oldPath,
    newPath,
    path: newPath ?? oldPath ?? "",
    status: "modified",
    binary: false,
    hunks: [],
    additions: 0,
    deletions: 0,
  };
}

function finish(file: DiffFile): DiffFile {
  if (file.status === "modified") {
    if (file.oldPath === null && file.newPath !== null) file.status = "added";
    else if (file.newPath === null && file.oldPath !== null) file.status = "deleted";
    else if (file.oldPath !== null && file.newPath !== null && file.oldPath !== file.newPath) {
      file.status = "renamed";
    }
  }
  file.path = file.newPath ?? file.oldPath ?? file.path;
  return file;
}

/** Parse unified diff text (as produced by `git diff`) into per-file hunks. */
export function parseUnifiedDiff(text: string): DiffFile[] {
  const files: DiffFile[] = [];
  // Casts keep TS from narrowing these to `null`; `startFile` reassigns them.
  let file = null as DiffFile | null;
  let hunk = null as DiffHunk | null;
  let oldNo = 0;
  let newNo = 0;
  let oldLeft = 0;
  let newLeft = 0;

  const lines = text.split("\n");
  // A trailing newline yields one empty final element that is not a line.
  if (lines.length > 0 && lines[lines.length - 1] === "") lines.pop();

  const startFile = (oldPath: string | null, newPath: string | null) => {
    if (file) files.push(finish(file));
    file = newFile(oldPath, newPath);
    hunk = null;
    oldLeft = 0;
    newLeft = 0;
  };

  for (const raw of lines) {
    const line = raw.endsWith("\r") ? raw.slice(0, -1) : raw;
    const inHunk = hunk !== null && (oldLeft > 0 || newLeft > 0);

    if (inHunk && file && hunk) {
      const current = file;
      const h = hunk;
      const sign = line[0];
      if (sign === "+") {
        h.lines.push({ type: "add", text: line.slice(1), oldNo: null, newNo: newNo++ });
        current.additions++;
        newLeft--;
        continue;
      }
      if (sign === "-") {
        h.lines.push({ type: "del", text: line.slice(1), oldNo: oldNo++, newNo: null });
        current.deletions++;
        oldLeft--;
        continue;
      }
      if (sign === " " || line === "") {
        h.lines.push({
          type: "context",
          text: line.slice(1),
          oldNo: oldNo++,
          newNo: newNo++,
        });
        oldLeft--;
        newLeft--;
        continue;
      }
      if (sign === "\\") {
        h.lines.push({ type: "meta", text: line, oldNo: null, newNo: null });
        continue;
      }
      // Anything else ends the hunk early (malformed or truncated counts).
      hunk = null;
    }

    if (line.startsWith("diff --git ")) {
      const rest = line.slice("diff --git ".length);
      const split = rest.lastIndexOf(" b/");
      if (split > 0) startFile(stripPrefix(rest.slice(0, split)), stripPrefix(rest.slice(split + 1)));
      else startFile(null, stripPrefix(rest));
      continue;
    }

    const hunkMatch = HUNK_RE.exec(line);
    if (hunkMatch) {
      if (!file) startFile(null, null);
      oldNo = Number(hunkMatch[1]);
      newNo = Number(hunkMatch[3]);
      oldLeft = hunkMatch[2] === undefined ? 1 : Number(hunkMatch[2]);
      newLeft = hunkMatch[4] === undefined ? 1 : Number(hunkMatch[4]);
      const h: DiffHunk = { header: line, lines: [] };
      hunk = h;
      file?.hunks.push(h);
      continue;
    }

    if (line.startsWith("\\") && file) {
      const last = file.hunks[file.hunks.length - 1];
      last?.lines.push({ type: "meta", text: line, oldNo: null, newNo: null });
      continue;
    }

    if (line.startsWith("--- ")) {
      const path = stripPrefix(line.slice(4));
      if (!file || file.hunks.length > 0) startFile(path, null);
      else file.oldPath = path;
      continue;
    }
    if (line.startsWith("+++ ")) {
      if (!file) startFile(null, null);
      if (file) file.newPath = stripPrefix(line.slice(4));
      continue;
    }

    const f = file;
    if (!f) continue;
    if (line.startsWith("new file mode")) {
      f.status = "added";
      f.oldPath = null;
    } else if (line.startsWith("deleted file mode")) {
      f.status = "deleted";
      f.newPath = null;
    } else if (line.startsWith("rename from ")) {
      f.status = "renamed";
      f.oldPath = line.slice("rename from ".length);
    } else if (line.startsWith("rename to ")) {
      f.status = "renamed";
      f.newPath = line.slice("rename to ".length);
    } else if (line.startsWith("Binary files ") || line === "GIT binary patch") {
      f.binary = true;
    }
  }

  if (file) files.push(finish(file));
  return files;
}
