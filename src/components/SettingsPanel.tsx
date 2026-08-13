import { useEffect } from "react";

import type { DetectedAgent } from "../ipc";

/**
 * Settings & privacy: an honest statement of the threat boundary, the detected
 * agents, and the "Forget everything" control. Modal overlay (Esc to close).
 */
export default function SettingsPanel({
  open,
  agents,
  onForgetEverything,
  onClose,
}: {
  open: boolean;
  agents: DetectedAgent[];
  onForgetEverything: () => void;
  onClose: () => void;
}) {
  useEffect(() => {
    function onKey(event: KeyboardEvent) {
      if (event.key === "Escape") onClose();
    }
    if (open) window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [open, onClose]);

  if (!open) return null;

  return (
    <div className="palette__backdrop" role="presentation" onClick={onClose}>
      <div
        className="settings"
        role="dialog"
        aria-modal="true"
        aria-label="Settings"
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
          {agents.length === 0 ? (
            <p className="empty">No agents ingested yet.</p>
          ) : (
            <ul className="settings__agents">
              {agents.map((agent) => (
                <li key={agent.id}>
                  <span>{agent.display_name}</span>
                  <span className="nav-item__count">{agent.session_count} sessions</span>
                </li>
              ))}
            </ul>
          )}
        </section>

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
