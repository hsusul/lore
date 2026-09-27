import { useEffect, useRef, useState, type FormEvent } from "react";

import {
  createTask,
  type CreateTaskRequest,
  type TaskAgent,
  type TaskDto,
  type TaskEffort,
  type TaskPermission,
} from "../../ipc";
import AgentMenu from "./AgentMenu";
import { SlidersIcon } from "./icons";
import { deriveTitle, errorText, parseClaims } from "./state";

type Props = {
  workspace: string | null;
  onCreated: (task: TaskDto) => void;
  onOpenFolder: () => void;
  /** Text to start the prompt with (the palette's "New agent: …"). */
  initialPrompt?: string;
  /** Focus the prompt when the form appears, caret at the end. */
  autoFocus?: boolean;
  /** Offer One per line: each non-empty line of the prompt launches its own agent. */
  allowSplit?: boolean;
};

/**
 * Launch an agent in the open repository. The prompt comes first; the title is
 * optional and defaults to the prompt's first line.
 */
export default function NewAgentForm({
  workspace,
  onCreated,
  onOpenFolder,
  initialPrompt = "",
  autoFocus = false,
  allowSplit = false,
}: Props) {
  const [title, setTitle] = useState("");
  const [prompt, setPrompt] = useState(initialPrompt);
  const [agent, setAgent] = useState<TaskAgent>("claude_code");
  const [permission, setPermission] = useState<TaskPermission>("edits");
  const [model, setModel] = useState<string | null>(null);
  const [effort, setEffort] = useState<TaskEffort | null>(null);
  const [claimsText, setClaimsText] = useState("");
  const [autoHandoff, setAutoHandoff] = useState(true);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [options, setOptions] = useState(false);
  const [split, setSplit] = useState(false);
  const promptRef = useRef<HTMLTextAreaElement>(null);

  useEffect(() => {
    const el = promptRef.current;
    if (!autoFocus || !el) return;
    el.focus();
    el.setSelectionRange(el.value.length, el.value.length);
  }, [autoFocus]);

  const claims = parseClaims(claimsText);
  const autoTitle = deriveTitle(prompt);
  const lines = prompt
    .split("\n")
    .map((line) => line.trim())
    .filter(Boolean);
  // With One per line on, every line is its own task; otherwise the prompt is one.
  const prompts = allowSplit && split && lines.length > 1 ? lines : [prompt.trim()];

  async function submit(event: FormEvent) {
    event.preventDefault();
    if (!workspace) return;
    if (!prompts[0]) {
      setError("Describe what the agent should do.");
      promptRef.current?.focus();
      return;
    }
    if (prompts.length > 1 && (title.trim() || claims.length > 0)) {
      setError("A title and owned paths belong to one agent. Turn off One per line to use them.");
      return;
    }
    const requestFor = (text: string): CreateTaskRequest => {
      const request: CreateTaskRequest = {
        repo_path: workspace,
        title: prompts.length > 1 ? deriveTitle(text) : title.trim() || autoTitle,
        prompt: text,
        agent,
        permission,
        auto_handoff: autoHandoff,
      };
      if (claims.length > 0) request.claims = claims;
      if (model) request.model = model;
      if (effort) request.effort = effort;
      return request;
    };
    setBusy(true);
    setError(null);
    // One at a time, so a failure leaves the lines it did not launch in the box.
    const created: TaskDto[] = [];
    try {
      for (const text of prompts) created.push(await createTask(requestFor(text)));
      setTitle("");
      setPrompt("");
      setClaimsText("");
    } catch (e) {
      if (created.length > 0) {
        setPrompt(prompts.slice(created.length).join("\n"));
        setError(`Launched ${created.length} of ${prompts.length}. ${errorText(e)}`);
      } else {
        setError(errorText(e));
      }
    } finally {
      setBusy(false);
    }
    for (const task of created) onCreated(task);
  }

  const disabled = !workspace;
  const launchLabel = prompts.length > 1 ? `Launch ${prompts.length} agents` : "Launch";
  return (
    <form className="new-agent" aria-label="New agent" onSubmit={(e) => void submit(e)}>
      {disabled && (
        <p className="wb-note">
          Open a folder to launch agents in it.{" "}
          <button type="button" className="wb-link" onClick={onOpenFolder}>
            Open folder…
          </button>
        </p>
      )}
      <fieldset disabled={disabled} className="new-agent__fields">
        <div className="composer__box">
          <textarea
            ref={promptRef}
            className="composer__input new-agent__prompt"
            aria-label="Prompt"
            rows={3}
            placeholder={
              split
                ? "One task per line. Each gets its own agent, worktree and branch."
                : "Describe a task. The agent gets its own worktree and branch."
            }
            value={prompt}
            onChange={(e) => setPrompt(e.target.value)}
            onKeyDown={(e) => {
              // Enter launches; Shift+Enter is a new line. Not while an input method is composing.
              if (e.key === "Enter" && !e.shiftKey && !e.nativeEvent.isComposing) {
                e.preventDefault();
                e.currentTarget.form?.requestSubmit();
              }
            }}
          />
          {options && (
            <div className="new-agent__options">
              <label className="new-agent__field">
                <span>Title</span>
                <input
                  className="new-agent__name"
                  type="text"
                  aria-label="Title"
                  placeholder={autoTitle || "From the prompt's first line"}
                  value={title}
                  onChange={(e) => setTitle(e.target.value)}
                />
              </label>
              <label className="new-agent__field">
                <span>Owns</span>
                <textarea
                  rows={1}
                  className="new-agent__claims"
                  aria-label="Owns (files or folders)"
                  placeholder="e.g. src/parser/, docs/SCHEMA.md"
                  value={claimsText}
                  onChange={(e) => setClaimsText(e.target.value)}
                />
              </label>
              <p className="new-agent__hint">
                Other agents are told not to touch owned paths, and a commit that changes another agent&rsquo;s files
                needs confirmation. End a folder with <span className="mono">/</span>.
              </p>
            </div>
          )}
          {claims.length > 0 && (
            <ul className="claim-chips" aria-label="Owned paths">
              {claims.map((claim) => (
                <li key={claim} className="claim-chip mono">
                  {claim}
                </li>
              ))}
            </ul>
          )}
          {error && (
            <p className="wb-note wb-note--error" role="alert">
              {error}
            </p>
          )}
          <div className="composer__bar">
            <AgentMenu
              label="Agent"
              agent={agent}
              onAgentChange={setAgent}
              permission={permission}
              onPermissionChange={setPermission}
              autoHandoff={autoHandoff}
              onAutoHandoffChange={setAutoHandoff}
              model={model}
              onModelChange={setModel}
              effort={effort}
              onEffortChange={setEffort}
              disabled={disabled}
            />
            <button
              type="button"
              className={`composer__tool${options || claims.length > 0 || title ? " composer__tool--on" : ""}`}
              aria-expanded={options}
              onClick={() => setOptions((v) => !v)}
            >
              <SlidersIcon />
              Options
            </button>
            {allowSplit && (
              <button
                type="button"
                className={`composer__tool${split ? " composer__tool--on" : ""}`}
                aria-pressed={split}
                title="Launch one agent for each line"
                onClick={() => setSplit((v) => !v)}
              >
                One per line
              </button>
            )}
            <span className="composer__spacer" />
            <button
              type="submit"
              className="wb-btn wb-btn--primary new-agent__launch"
              disabled={busy}
              title="Launch (Enter; Shift+Enter for a new line)"
              aria-label={busy ? "Launching…" : launchLabel}
            >
              {busy ? "Launching…" : launchLabel}
              <kbd>↵</kbd>
            </button>
          </div>
        </div>
      </fieldset>
    </form>
  );
}
