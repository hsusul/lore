import { Archive } from "lucide-react";
import { useState } from "react";

import type { DetectedAgent } from "../ipc";
import { Button } from "./ui/button";
import { DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuTrigger } from "./ui/dropdown-menu";
import { Footnote, FormGroup, FormRow } from "./ui/form";

const KNOWN = [
  { id: "claude-code", name: "Claude Code", path: "~/.claude/projects" },
  { id: "codex", name: "Codex", path: "~/.codex/sessions" },
] as const;

/** The agents Lore knows, with their folders, whether or not this Mac has them. */
function knownAgents(agents: DetectedAgent[]): DetectedAgent[] {
  return KNOWN.map(
    (known) =>
      agents.find((a) => a.id === known.id) ?? {
        id: known.id,
        display_name: known.name,
        installed: false,
        version: null,
        session_count: 0,
        roots: [known.path],
        custom_roots: [],
      },
  );
}

/**
 * First run (spec 9.13, 18.10), shown only for a loaded archive that is truly empty: one
 * centered column with the folders Lore will read, "Add Folder…" under them, and Start
 * Scanning trailing in the footer row.
 */
export default function Onboarding({
  agents,
  scanning,
  onScan,
  onAddRoot,
}: {
  agents: DetectedAgent[];
  scanning: boolean;
  onScan: () => void;
  onAddRoot: (agent: DetectedAgent) => Promise<string | null>;
}) {
  const sources = knownAgents(agents);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  async function add(agent: DetectedAgent) {
    setBusy(true);
    setError(null);
    try {
      await onAddRoot(agent);
    } catch (e) {
      setError(`Lore could not add the folder. ${String(e)}`);
    } finally {
      setBusy(false);
    }
  }

  return (
    <section className="m-auto flex w-onboard flex-col items-center py-8 text-center" aria-labelledby="onboarding-title">
      <Archive className="size-icon-xl stroke-icon text-label-2" aria-hidden="true" />
      <h2 id="onboarding-title" className="mt-4 text-title1">
        Welcome to Lore
      </h2>
      <p className="mt-2 text-body text-label-2">
        Lore keeps a private, searchable archive of your Claude Code and Codex sessions on this Mac.
      </p>
      <FormGroup label="Agent folders" className="mt-6 w-full text-left">
        {sources.flatMap((agent) =>
          agent.roots.map((root) => (
            <FormRow
              key={`${agent.id}:${root}`}
              label={agent.display_name}
              description={<code className="block truncate font-mono text-code" title={root}>{root}</code>}
            >
              <span className="text-callout text-label-2">{agent.installed ? "Found" : "Not found"}</span>
            </FormRow>
          )),
        )}
      </FormGroup>
      <div className="mt-3 flex w-full flex-col items-start text-left">
        <DropdownMenu>
          <DropdownMenuTrigger asChild>
            <Button disabled={busy}>{busy ? "Adding Folder…" : "Add Folder…"}</Button>
          </DropdownMenuTrigger>
          <DropdownMenuContent>
            {sources.map((agent) => (
              <DropdownMenuItem key={agent.id} onSelect={() => void add(agent)}>
                {agent.display_name} Folder…
              </DropdownMenuItem>
            ))}
          </DropdownMenuContent>
        </DropdownMenu>
        {error && <Footnote role="alert">{error}</Footnote>}
      </div>
      <div className="mt-6 flex w-full justify-end">
        <Button variant={scanning ? "push" : "default"} disabled={scanning} onClick={onScan}>
          {scanning ? "Scanning…" : "Start Scanning"}
        </Button>
      </div>
    </section>
  );
}
