import { useEffect, useId, useRef, useState, type FormEvent, type KeyboardEvent } from "react";

import { deleteProvider, saveProvider, type ProviderDto, type ProviderKind, type SaveProviderRequest } from "../../ipc";
import { isModelId } from "./agentOptions";
import { CloseIcon, PlusIcon } from "./icons";
import { errorText } from "./state";
import { refreshProviders, useProviders } from "./useProviders";

const FOCUSABLE =
  'button:not([disabled]), input:not([disabled]), textarea:not([disabled]), select:not([disabled]), summary, [tabindex]:not([tabindex="-1"])';

const KINDS: ProviderKind[] = ["ollama", "lmstudio", "custom"];

const KIND_LABELS: Record<ProviderKind, string> = {
  ollama: "Ollama",
  lmstudio: "LM Studio",
  custom: "Any API",
};

const KIND_HINTS: Record<ProviderKind, string> = {
  ollama: "Runs on this Mac. Start Ollama and pull each model first, e.g. ollama pull gpt-oss:20b.",
  lmstudio: "Runs on this Mac. Download each model in LM Studio and start its local server.",
  custom: "Any endpoint that speaks the OpenAI Responses API, such as OpenRouter. Codex does not use Chat Completions.",
};

const MODEL_PLACEHOLDERS: Record<ProviderKind, string> = {
  ollama: "gpt-oss:20b\nqwen3-coder:30b",
  lmstudio: "openai/gpt-oss-20b",
  custom: "qwen/qwen3-coder",
};

type Draft = {
  id?: string;
  kind: ProviderKind;
  name: string;
  baseUrl: string;
  apiKey: string;
  removeKey: boolean;
  models: string;
  hasKey: boolean;
};

/** A new provider starts on a local kind not added yet, else an API. */
function newDraft(existing: ProviderDto[]): Draft {
  const kind = KINDS.find((k) => k === "custom" || !existing.some((p) => p.kind === k)) ?? "custom";
  const name = kind === "custom" ? "" : KIND_LABELS[kind];
  return { kind, name, baseUrl: "", apiKey: "", removeKey: false, models: "", hasKey: false };
}

function draftFor(provider: ProviderDto): Draft {
  return {
    id: provider.id,
    kind: provider.kind,
    name: provider.name,
    baseUrl: provider.base_url ?? "",
    apiKey: "",
    removeKey: false,
    models: provider.models.join("\n"),
    hasKey: provider.has_api_key,
  };
}

/** Where a provider runs, its key, and its models, for the list. */
function providerSummary(provider: ProviderDto): string {
  const where =
    provider.kind === "custom"
      ? (provider.base_url ?? "").replace(/^https?:\/\//, "")
      : `${KIND_LABELS[provider.kind]} on this Mac`;
  return [where, provider.has_api_key ? "key in Keychain" : null, provider.models.join(", ")]
    .filter(Boolean)
    .join(" · ");
}

/** Model ids typed one per line (commas work too). */
function parseModels(raw: string): string[] {
  return [...new Set(raw.split(/[\s,]+/).filter(Boolean))];
}

/**
 * Other models an agent can run: a local Ollama or LM Studio, or any API that
 * speaks the OpenAI Responses API. They run through Codex; Lore stores the
 * settings and keeps API keys in the macOS Keychain.
 */
export default function ProvidersDialog({ onClose }: { onClose: () => void }) {
  const providers = useProviders();
  const dialogRef = useRef<HTMLDivElement>(null);
  const titleId = useId();
  const [opener] = useState(() => document.activeElement as HTMLElement | null);
  // With nothing saved yet, open straight on the form.
  const [draft, setDraft] = useState<Draft | null>(() => (providers.length === 0 ? newDraft(providers) : null));
  const [removing, setRemoving] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    void refreshProviders();
  }, []);

  useEffect(
    () => () => {
      if (opener && document.contains(opener)) opener.focus();
    },
    [opener],
  );

  // Focus the first field of the form, or the first control of the list.
  const editing = draft !== null;
  useEffect(() => {
    const dialog = dialogRef.current;
    if (!dialog) return;
    const target = editing
      ? dialog.querySelector<HTMLElement>(".providers__form input:checked")
      : dialog.querySelector<HTMLElement>(".providers__add");
    target?.focus();
  }, [editing]);

  function onKeyDown(event: KeyboardEvent<HTMLDivElement>) {
    if (event.key === "Escape") {
      event.preventDefault();
      event.stopPropagation();
      onClose();
      return;
    }
    if (event.key !== "Tab" || !dialogRef.current) return;
    // Keep Tab inside the dialog: it is modal.
    const items = Array.from(dialogRef.current.querySelectorAll<HTMLElement>(FOCUSABLE));
    if (items.length === 0) return;
    const first = items[0];
    const last = items[items.length - 1];
    if (event.shiftKey && document.activeElement === first) {
      event.preventDefault();
      last.focus();
    } else if (!event.shiftKey && document.activeElement === last) {
      event.preventDefault();
      first.focus();
    }
  }

  function edit(next: Draft | null) {
    setDraft(next);
    setRemoving(null);
    setError(null);
  }

  async function submit(event: FormEvent) {
    event.preventDefault();
    if (!draft || busy) return;
    const name = draft.name.trim();
    const models = parseModels(draft.models);
    const custom = draft.kind === "custom";
    const invalid = models.find((model) => !isModelId(model));
    const problem = !name
      ? "Give the provider a name."
      : custom && !/^https?:\/\/\S+$/.test(draft.baseUrl.trim())
        ? "Enter the API's base URL, starting with http:// or https://."
        : models.length === 0
          ? "Add at least one model id."
          : invalid
            ? `“${invalid}” is not a model id Codex can take. Use letters, digits, and . _ - : /, starting with a letter or digit.`
            : null;
    if (problem) {
      setError(problem);
      return;
    }
    const request: SaveProviderRequest = { name, kind: draft.kind, models };
    if (draft.id) request.id = draft.id;
    if (custom) {
      request.base_url = draft.baseUrl.trim();
      const key = draft.apiKey.trim();
      if (key) request.api_key = key;
      else if (draft.removeKey) request.api_key = "";
    }
    setBusy(true);
    setError(null);
    try {
      await saveProvider(request);
      await refreshProviders();
      edit(null);
    } catch (e) {
      setError(errorText(e));
    } finally {
      setBusy(false);
    }
  }

  async function remove(id: string) {
    setBusy(true);
    setError(null);
    try {
      await deleteProvider(id);
      await refreshProviders();
      setRemoving(null);
    } catch (e) {
      setError(errorText(e));
    } finally {
      setBusy(false);
    }
  }

  return (
    <div
      className="dialog-backdrop"
      onMouseDown={(event) => {
        if (event.target === event.currentTarget) onClose();
      }}
    >
      <div
        ref={dialogRef}
        className="dialog providers"
        role="dialog"
        aria-modal="true"
        aria-labelledby={titleId}
        onKeyDown={onKeyDown}
      >
        <header className="dialog__header">
          <h2 id={titleId} className="dialog__title">
            Model providers
          </h2>
          <span className="dialog__context">Open-source and other models, run through Codex</span>
          <button type="button" className="wb-icon-btn" aria-label="Close" title="Close (Esc)" onClick={onClose}>
            <CloseIcon />
          </button>
        </header>

        <div className="providers__body">
          {providers.length > 0 && (
            <ul className="providers__list" aria-label="Saved providers">
              {providers.map((provider) => (
                <li key={provider.id} className="providers__item">
                  <span className="providers__main">
                    <span className="providers__name">{provider.name}</span>
                    <span className="providers__meta">{providerSummary(provider)}</span>
                  </span>
                  {removing === provider.id ? (
                    <span className="providers__confirm" role="group" aria-label={`Remove ${provider.name}`}>
                      <span>Remove?</span>
                      <button type="button" className="wb-btn wb-btn--ghost wb-btn--small" onClick={() => setRemoving(null)}>
                        Keep
                      </button>
                      <button
                        type="button"
                        className="wb-btn wb-btn--danger wb-btn--small"
                        disabled={busy}
                        onClick={() => void remove(provider.id)}
                      >
                        Remove
                      </button>
                    </span>
                  ) : (
                    <span className="providers__actions">
                      <button
                        type="button"
                        className="wb-btn wb-btn--ghost wb-btn--small"
                        aria-label={`Edit ${provider.name}`}
                        onClick={() => edit(draftFor(provider))}
                      >
                        Edit
                      </button>
                      <button
                        type="button"
                        className="wb-btn wb-btn--ghost wb-btn--small"
                        aria-label={`Remove ${provider.name}`}
                        onClick={() => {
                          setRemoving(provider.id);
                          setError(null);
                        }}
                      >
                        Remove
                      </button>
                    </span>
                  )}
                </li>
              ))}
            </ul>
          )}

          {draft ? (
            <ProviderForm draft={draft} busy={busy} onChange={setDraft} onSubmit={submit} onCancel={() => edit(null)} />
          ) : (
            <button type="button" className="wb-btn wb-btn--small providers__add" onClick={() => edit(newDraft(providers))}>
              <PlusIcon />
              Add provider
            </button>
          )}

          {error && (
            <p className="wb-note wb-note--error" role="alert">
              {error}
            </p>
          )}
          {removing && (
            <p className="wb-note providers__note">
              Removing a provider deletes its key. Agents that used it ask for another model before they run again.
            </p>
          )}
        </div>
      </div>
    </div>
  );
}

function ProviderForm({
  draft,
  busy,
  onChange,
  onSubmit,
  onCancel,
}: {
  draft: Draft;
  busy: boolean;
  onChange: (draft: Draft) => void;
  onSubmit: (event: FormEvent) => void;
  onCancel: () => void;
}) {
  const kindName = useId();
  const hintId = useId();
  const keyHintId = useId();
  const custom = draft.kind === "custom";

  function setKind(kind: ProviderKind) {
    // A name still set to the old kind's default follows the new kind.
    const defaultName = draft.name.trim() === "" || KINDS.some((k) => k !== "custom" && KIND_LABELS[k] === draft.name);
    onChange({ ...draft, kind, name: defaultName ? (kind === "custom" ? "" : KIND_LABELS[kind]) : draft.name });
  }

  return (
    <form className="providers__form" aria-label={draft.id ? `Edit ${draft.name}` : "New provider"} onSubmit={onSubmit}>
      <fieldset className="providers__kinds" aria-describedby={hintId}>
        <legend>Runs on</legend>
        {KINDS.map((kind) => (
          <label key={kind} className="providers__kind">
            <input
              type="radio"
              name={kindName}
              value={kind}
              checked={draft.kind === kind}
              onChange={() => setKind(kind)}
            />
            {KIND_LABELS[kind]}
          </label>
        ))}
      </fieldset>
      <p id={hintId} className="wb-note providers__hint">
        {KIND_HINTS[draft.kind]}
      </p>
      <label className="wb-field">
        <span>Name</span>
        <input
          type="text"
          value={draft.name}
          placeholder={custom ? "OpenRouter" : KIND_LABELS[draft.kind]}
          maxLength={60}
          onChange={(e) => onChange({ ...draft, name: e.target.value })}
        />
      </label>
      {custom && (
        <>
          <label className="wb-field">
            <span>Base URL</span>
            <input
              className="providers__mono"
              type="text"
              inputMode="url"
              spellCheck={false}
              autoCapitalize="off"
              autoCorrect="off"
              value={draft.baseUrl}
              placeholder="https://openrouter.ai/api/v1"
              onChange={(e) => onChange({ ...draft, baseUrl: e.target.value })}
            />
          </label>
          <label className="wb-field">
            <span>API key</span>
            <input
              type="password"
              autoComplete="off"
              spellCheck={false}
              aria-describedby={keyHintId}
              value={draft.apiKey}
              placeholder={draft.hasKey && !draft.removeKey ? "Saved · type a new key to replace it" : "Optional"}
              onChange={(e) => onChange({ ...draft, apiKey: e.target.value, removeKey: false })}
            />
          </label>
          <p id={keyHintId} className="wb-note providers__hint">
            Saved in your macOS Keychain. Codex gets it through its environment, never on the command line.
          </p>
          {draft.hasKey && (
            <label className="providers__check">
              <input
                type="checkbox"
                checked={draft.removeKey}
                onChange={(e) => onChange({ ...draft, removeKey: e.target.checked, apiKey: "" })}
              />
              Remove the saved key
            </label>
          )}
        </>
      )}
      <label className="wb-field">
        <span>Models</span>
        <textarea
          className="providers__mono"
          rows={3}
          spellCheck={false}
          autoCapitalize="off"
          autoCorrect="off"
          value={draft.models}
          placeholder={MODEL_PLACEHOLDERS[draft.kind]}
          onChange={(e) => onChange({ ...draft, models: e.target.value })}
        />
      </label>
      <p className="wb-note providers__hint">One model id per line, spelled the way the provider names it.</p>
      <div className="providers__buttons">
        <button type="button" className="wb-btn wb-btn--ghost wb-btn--small" onClick={onCancel}>
          Cancel
        </button>
        <button type="submit" className="wb-btn wb-btn--primary wb-btn--small" disabled={busy}>
          {draft.id ? "Save" : "Add provider"}
        </button>
      </div>
    </form>
  );
}
