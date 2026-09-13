// Deliberately tiny syntax tinting: comments, strings, numbers, and keywords
// for common languages. Not a parser; skipped for large files.

import type { ReactNode } from "react";

const MAX_HIGHLIGHT_CHARS = 200_000;

const C_LIKE = new Set([
  "ts", "tsx", "js", "jsx", "mjs", "cjs", "rs", "go", "java", "kt", "swift", "c", "h", "cc", "cpp",
  "hpp", "cs", "css", "scss", "json", "jsonc", "php", "dart", "scala",
]);
const HASH = new Set(["py", "sh", "bash", "zsh", "toml", "yaml", "yml", "rb", "pl", "r", "mk", "dockerfile"]);

const KEYWORDS = new Set(
  (
    "as async await break case catch class const continue crate def default do elif else enum export " +
    "extends false fn for from func function if impl import in interface let loop match mod module " +
    "mut new nil None null package private protected pub public return self Self static struct super " +
    "switch this throw true True False try type typeof use var void where while with yield"
  ).split(" "),
);

const STRINGS = String.raw`"(?:[^"\\\n]|\\.)*"|'(?:[^'\\\n]|\\.)*'|\x60(?:[^\x60\\]|\\.)*\x60`;
const RUST_STRINGS = String.raw`"(?:[^"\\]|\\.)*"|'(?:[^'\\\n]|\\.)'`;
const WORDS = String.raw`\b[A-Za-z_]\w*\b|\b\d[\w.]*\b`;

function languageOf(fileName: string): "c" | "hash" | null {
  const lower = fileName.toLowerCase();
  if (lower === "dockerfile" || lower === "makefile") return "hash";
  const ext = lower.includes(".") ? lower.slice(lower.lastIndexOf(".") + 1) : "";
  if (C_LIKE.has(ext)) return "c";
  if (HASH.has(ext)) return "hash";
  return null;
}

/** Tokenize `text` into tinted spans, or return it unchanged when unsupported. */
export function highlight(text: string, fileName: string): ReactNode {
  const lang = languageOf(fileName);
  if (!lang || text.length > MAX_HIGHLIGHT_CHARS) return text;
  const isRust = fileName.toLowerCase().endsWith(".rs");
  const comment = lang === "c" ? String.raw`\/\/[^\n]*|\/\*[\s\S]*?\*\/` : String.raw`#[^\n]*`;
  const re = new RegExp(`(${comment})|(${isRust ? RUST_STRINGS : STRINGS})|(${WORDS})`, "g");

  const out: ReactNode[] = [];
  let last = 0;
  let plain = "";
  for (const m of text.matchAll(re)) {
    const index = m.index ?? 0;
    const [token, isComment, isString] = m;
    let cls: string | null = null;
    if (isComment) cls = "tok-comment";
    else if (isString) cls = "tok-string";
    else if (/^\d/.test(token)) cls = "tok-number";
    else if (KEYWORDS.has(token)) cls = "tok-keyword";
    if (!cls) continue;
    plain += text.slice(last, index);
    if (plain) out.push(plain);
    plain = "";
    out.push(
      <span key={index} className={cls}>
        {token}
      </span>,
    );
    last = index + token.length;
  }
  plain += text.slice(last);
  if (plain) out.push(plain);
  return out;
}
