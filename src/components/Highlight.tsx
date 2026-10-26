import { Fragment, type ReactNode } from "react";

import { HIGHLIGHT_END, HIGHLIGHT_START } from "../ipc";

/** Render a search snippet, turning the backend's highlight markers into marks. */
export function Snippet({ text }: { text: string }) {
  const nodes: ReactNode[] = [];
  let rest = text;
  let key = 0;
  while (rest.length > 0) {
    const start = rest.indexOf(HIGHLIGHT_START);
    if (start === -1) {
      nodes.push(<Fragment key={key++}>{rest}</Fragment>);
      break;
    }
    if (start > 0) nodes.push(<Fragment key={key++}>{rest.slice(0, start)}</Fragment>);
    const from = start + HIGHLIGHT_START.length;
    const end = rest.indexOf(HIGHLIGHT_END, from);
    if (end === -1) {
      nodes.push(<Fragment key={key++}>{rest.slice(from)}</Fragment>);
      break;
    }
    nodes.push(<mark key={key++}>{rest.slice(from, end)}</mark>);
    rest = rest.slice(end + HIGHLIGHT_END.length);
  }
  return <>{nodes}</>;
}

function escapeRegExp(text: string): string {
  return text.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
}

/** Mark every case-insensitive occurrence of the search terms in plain text. */
export function Highlight({ text, terms }: { text: string; terms: string[] }) {
  const usable = terms.map((t) => t.replace(/^["']+|["']+$/g, "")).filter((t) => t.length > 1);
  if (usable.length === 0) return <>{text}</>;
  const pattern = new RegExp(`(${usable.map(escapeRegExp).join("|")})`, "gi");
  const parts = text.split(pattern);
  return (
    <>
      {parts.map((part, index) =>
        index % 2 === 1 ? <mark key={index}>{part}</mark> : <Fragment key={index}>{part}</Fragment>,
      )}
    </>
  );
}
