import type { DroppedRuleDto, EnforceRuleDto } from "../ipc";

const same = (a: EnforceRuleDto, b: EnforceRuleDto) =>
  a.tool === b.tool && a.pattern === b.pattern && a.unless === b.unless;

function Rule({ rule, tag }: { rule: EnforceRuleDto; tag?: string }) {
  return (
    <>
      <span>{rule.tool}</span>
      <code>{rule.pattern}</code>
      {rule.unless !== null && (
        <>
          <span>unless</span>
          <code>{rule.unless}</code>
        </>
      )}
      {tag && <span className="mist-tag">{tag}</span>}
    </>
  );
}

/**
 * The `enforce` rules a note has, or a proposal would give it: what each one
 * blocks, so a person sees a rule before accepting it. `before` is an update's
 * target note's rules: the new ones are marked, and the ones the update drops
 * are struck out. `dropped` are rules that did not parse, so block nothing.
 */
export function Blocks({
  rules,
  before,
  dropped = [],
  pending = false,
}: {
  rules: EnforceRuleDto[];
  before?: EnforceRuleDto[];
  dropped?: DroppedRuleDto[];
  pending?: boolean;
}) {
  const removed = before?.filter((b) => !rules.some((r) => same(r, b))) ?? [];
  if (rules.length + removed.length + dropped.length === 0) return null;
  return (
    <section className="mist-blocks" aria-label="Blocks">
      <div className="mist-kind">Blocks</div>
      <ul>
        {rules.map((rule, k) => (
          <li key={`r${k}`}>
            <Rule
              rule={rule}
              tag={before && !before.some((b) => same(b, rule)) ? "new" : undefined}
            />
          </li>
        ))}
        {removed.map((rule, k) => (
          <li key={`x${k}`} className="gone">
            <Rule rule={rule} tag="removed" />
          </li>
        ))}
        {dropped.map((rule, k) => (
          <li key={`d${k}`} className="gone">
            <code>{rule.written}</code>
            <span className="mist-tag">not enforced</span>
            <span className="mist-err">{rule.error}</span>
          </li>
        ))}
      </ul>
      {rules.length > 0 && (
        <p className="mist-mt">
          {pending
            ? "Once accepted, agents' tool calls that match are denied."
            : "Agents' tool calls that match are denied."}
        </p>
      )}
    </section>
  );
}
