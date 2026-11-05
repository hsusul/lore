/**
 * A proposal's body as the lore-lessons skill writes it: one sentence on what
 * the agent did and what the person wanted, then a Receipts list, each item
 * the person's words in quotes and, in brackets, the agent, session and date.
 */
export interface Receipt {
  said: string;
  where: string;
}

const RECEIPTS = /^receipts:\s*$/i;
const ITEM = /^\s*[-*]\s+["“](.+?)["”]\s*(?:\((.+)\))?\s*$/;

/** The body before its Receipts list, on one line. */
export function why(body: string): string {
  const lines = body.split("\n");
  const end = lines.findIndex((l) => RECEIPTS.test(l.trim()));
  return (end < 0 ? lines : lines.slice(0, end)).join(" ").replace(/\s+/g, " ").trim();
}

/** The quoted receipts in the body, in order. */
export function receipts(body: string): Receipt[] {
  const lines = body.split("\n");
  const start = lines.findIndex((l) => RECEIPTS.test(l.trim()));
  if (start < 0) return [];
  return lines.slice(start + 1).flatMap((l) => {
    const m = ITEM.exec(l);
    return m ? [{ said: m[1], where: m[2] ?? "" }] : [];
  });
}
