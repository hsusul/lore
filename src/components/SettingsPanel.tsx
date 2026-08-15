import { useEffect, useRef } from "react";

import BackupSettings from "./BackupSettings";
import { useFocusTrap } from "../focus-trap";
import type { DetectedAgent } from "../ipc";

/**
 * Settings & privacy: an honest statement of the threat boundary, the detected
 * agents, and the "Forget everything" control. Modal overlay (Esc to close).
 */
export default function SettingsPanel({
  open,
  agents,
  rootBusy,
  onAddAgentRoot,
  onRemoveAgentRoot,
  onForgetEverything,
  onClose,
}: {
  open: boolean;
  agents: DetectedAgent[];
  rootBusy: string | null;
  onAddAgentRoot: (agentId: string, displayName: string) => void;
  onRemoveAgentRoot: (agentId: string, path: string) => void;
  onForgetEverything: () => void;
  onClose: () => void;
}) {
  const dialogRef = useRef<HTMLDivElement>(null);
  const restoreFocusRef = useRef<HTMLElement | null>(null);
  useFocusTrap(dialogRef, open);

  useEffect(() => {
    function onKey(event: KeyboardEvent) {
      if (event.key === "Escape") onClose();
    }
    if (open) window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [open, onClose]);

  // Dialog focus management: on open, remember what had focus and move focus
  // into the dialog so keyboard and screen-reader users land inside it; on
  // close, return focus to the element that opened it.
  useEffect(() => {
    if (!open) return;
    restoreFocusRef.current = document.activeElement as HTMLElement | null;
    dialogRef.current?.focus();
    return () => restoreFocusRef.current?.focus?.();
  }, [open]);

  if (!open) return null;

  return (
    <div className="palette__backdrop" role="presentation" onClick={onClose}>
      <div
        ref={dialogRef}
        className="settings"
        role="dialog"
        aria-modal="true"
        aria-label="Settings"
        tabIndex={-1}
        onClick={(event) => event.stopPropagation()}
      >
        <header className="settings__head">
          <h2>Settings</h2>
          <button className="icon-btn" aria-label="Close settings" onClick={onClose}>
            ✕
          </button>
        </header>

        <section aria-labelledby="privacy-heading">
          <h3 id="privacy-heading" className="section-title">
            Privacy
          </h3>
          <ul className="settings__facts">
            <li>Your archive stays on this machine. Lore opens no network connection by default.</li>
            <li>
              Flagged secrets are <strong>redacted</strong> from search and default exports — this
              is not a guarantee your data is secret-free. The canonical local copy stays faithful.
            </li>
            <li>
              File permissions are not encryption. Use FileVault and a locked login; Lore does not
              protect against another process running as you, or disk theft without disk encryption.
            </li>
            <li>Original agent logs and exports you keep are outside Lore and are never deleted.</li>
          </ul>
        </section>

        <section aria-labelledby="agents-heading">
          <h3 id="agents-heading" className="section-title">
            Agents
          </h3>
          <p className="empty">
            Lore checks these folders read-only. Add another location if your agent history lives
            somewhere else.
          </p>
          {agents.length === 0 ? (
            <p className="empty">Agent status is unavailable.</p>
          ) : (
            <ul className="settings__agents">
              {agents.map((agent) => {
                const busy = rootBusy === agent.id;
                return (
                  <li key={agent.id}>
                    <div className="settings__agent-head">
                      <strong>{agent.display_name}</strong>
                      <span className={`settings__agent-status${agent.installed ? " is-detected" : ""}`}>
                        {agent.installed ? "detected" : "not detected"}
                      </span>
                      <span className="nav-item__count">{agent.session_count} sessions</span>
                    </div>
                    <ul className="settings__roots">
                      {agent.roots.map((root) => {
                        const custom = agent.custom_roots.includes(root);
                        return (
                          <li key={root}>
                            <code title={root}>{root}</code>
                            {custom ? (
                              <>
                                <span className="settings__root-kind">custom</span>
                                <button
                                  className="btn--ghost settings__root-remove"
                                  type="button"
                                  aria-label={`Remove ${root} from ${agent.display_name}`}
                                  disabled={busy}
                                  onClick={() => onRemoveAgentRoot(agent.id, root)}
                                >
                                  Remove
                                </button>
                              </>
                            ) : (
                              <span className="settings__root-kind">default</span>
                            )}
                          </li>
                        );
                      })}
                    </ul>
                    <button
                      className="btn--ghost settings__add-root"
                      type="button"
                      disabled={busy}
                      onClick={() => onAddAgentRoot(agent.id, agent.display_name)}
                    >
                      {busy ? "Updating folders…" : `Add another ${agent.display_name} folder`}
                    </button>
                  </li>
                );
              })}
            </ul>
          )}
        </section>

        <BackupSettings />

        <section aria-labelledby="data-heading">
          <h3 id="data-heading" className="section-title">
            Data
          </h3>
          <p className="empty">
            Forget everything removes all ingested sessions, repositories, findings, and blobs from
            Lore. Original agent logs are not touched. Secure block-level erasure is not guaranteed
            on SSDs.
          </p>
          <button className="btn--danger" onClick={onForgetEverything}>
            Forget everything
          </button>
        </section>
      </div>
    </div>
  );
}
