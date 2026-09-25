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
import { deriveTitle, errorText, parseClaims, shortcut } from "./state";

type Props = {
  workspace: string | null;
  onCreated: (task: TaskDto) => void;
  onOpenFolder: () => void;
  /** Text to start the prompt with (the palette's "New agent: …"). */
  initialPrompt?: string;
  /** Focus the prompt when the form appears, caret at the end. */
  autoFocus?: boolean;
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
  const promptRef = useRef<HTMLTextAreaElement>(null);

  useEffect(() => {
    const el = promptRef.current;
    if (!autoFocus || !el) return;
    el.focus();
    el.setSelectionRange(el.value.length, el.value.length);
  }, [autoFocus]);

  const claims = parseClaims(claimsText);
  const autoTitle = deriveTitle(prompt);

  async function submit(event: FormEvent) {
    event.preventDefault();
    if (!workspace) return;
    const request: CreateTaskRequest = {
      repo_path: workspace,
      title: title.trim() || autoTitle,
      prompt: prompt.trim(),
      agent,
      permission,
      auto_handoff: autoHandoff,
    };
    if (claims.length > 0) request.claims = claims;
    if (model) request.model = model;
    if (effort) request.effort = effort;
    if (!request.prompt) {
      setError("Describe what the agent should do.");
      promptRef.current?.focus();
      return;
    }
    setBusy(true);
    setError(null);
    try {
      const task = await createTask(request);
      setTitle("");
      setPrompt("");
      setClaimsText("");
      onCreated(task);
    } catch (e) {
      setError(errorText(e));
    } finally {
      setBusy(false);
    }
  }

  const disabled = !workspace;
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
            placeholder="Describe a task. The agent gets its own worktree and branch."
            value={prompt}
            onChange={(e) => setPrompt(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Enter" && (e.metaKey || e.ctrlKey)) {
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
            <span className="composer__spacer" />
            <button
              type="submit"
              className="wb-btn wb-btn--primary new-agent__launch"
              disabled={busy}
              title={`Launch (${shortcut("Enter")})`}
              aria-label={busy ? "Launching…" : "Launch"}
            >
              {busy ? "Launching…" : "Launch"}
              <kbd>{shortcut("↵")}</kbd>
            </button>
          </div>
        </div>
      </fieldset>
    </form>
  );
}
