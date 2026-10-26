import { useEffect, useState } from "react";

import { clamp } from "../format";
import {
  backupNow,
  getBackupSchedule,
  setBackupSchedule,
  type BackupInterval,
  type DetectedAgent,
} from "../ipc";
import { Modal } from "./Modal";

function Sources({
  agents,
  busyAgent,
  onAddRoot,
  onRemoveRoot,
}: {
  agents: DetectedAgent[];
  busyAgent: string | null;
  onAddRoot: (agent: DetectedAgent) => void;
  onRemoveRoot: (agent: DetectedAgent, path: string) => void;
}) {
  return (
    <section className="settings__section" aria-labelledby="sources-heading">
      <h3 id="sources-heading" className="subhead">
        Sources
      </h3>
      <p className="quiet">Lore reads these folders and never writes to them. Add a folder if an agent keeps history somewhere else.</p>
      {agents.length === 0 ? (
        <p className="quiet">Agent status is unavailable.</p>
      ) : (
        <ul className="sources">
          {agents.map((agent) => {
            const busy = busyAgent === agent.id;
            return (
              <li key={agent.id} className="source">
                <div className="source__head">
                  <strong>{agent.display_name}</strong>
                  <span className={`flag ${agent.installed ? "flag--ok" : ""}`}>
                    {agent.installed ? "detected" : "not detected"}
                  </span>
                  {agent.version && <span className="quiet">v{agent.version}</span>}
                  <span className="count">{agent.session_count} sessions</span>
                </div>
                <ul className="source__roots">
                  {agent.roots.map((root) => {
                    const custom = agent.custom_roots.includes(root);
                    return (
                      <li key={root}>
                        <code title={root}>{root}</code>
                        <span className="quiet">{custom ? "added by you" : "default"}</span>
                        {custom && (
                          <button
                            type="button"
                            className="btn btn--quiet"
                            aria-label={`Remove ${root} from ${agent.display_name}`}
                            disabled={busy}
                            onClick={() => onRemoveRoot(agent, root)}
                          >
                            Remove
                          </button>
                        )}
                      </li>
                    );
                  })}
                </ul>
                <button type="button" className="btn" disabled={busy} onClick={() => onAddRoot(agent)}>
                  {busy ? "Updating folders…" : `Add a ${agent.display_name} folder…`}
                </button>
              </li>
            );
          })}
        </ul>
      )}
    </section>
  );
}

function Backups() {
  const [interval, setIntervalValue] = useState<BackupInterval>("off");
  const [keep, setKeep] = useState(7);
  const [status, setStatus] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    let alive = true;
    getBackupSchedule().then(
      (schedule) => {
        if (!alive) return;
        setIntervalValue(schedule.interval as BackupInterval);
        setKeep(schedule.keep);
      },
      (e: unknown) => alive && setStatus(`Could not read the backup schedule: ${String(e)}`),
    );
    return () => {
      alive = false;
    };
  }, []);

  async function save(nextInterval: BackupInterval, nextKeep: number) {
    setIntervalValue(nextInterval);
    setKeep(nextKeep);
    setStatus(null);
    try {
      await setBackupSchedule(nextInterval, nextKeep);
    } catch (e) {
      setStatus(`Could not save the schedule: ${String(e)}`);
    }
  }

  async function runNow() {
    setBusy(true);
    setStatus(null);
    try {
      await backupNow();
      setStatus("Backup created.");
    } catch (e) {
      setStatus(`Backup failed: ${String(e)}`);
    } finally {
      setBusy(false);
    }
  }

  return (
    <section className="settings__section" aria-labelledby="backups-heading">
      <h3 id="backups-heading" className="subhead">
        Backups
      </h3>
      <p className="quiet">Private copies of the archive on this machine. Forget everything removes them too.</p>
      <div className="settings__grid">
        <label htmlFor="backup-interval">Automatic backups</label>
        <select
          id="backup-interval"
          className="select"
          value={interval}
          onChange={(event) => void save(event.target.value as BackupInterval, keep)}
        >
          <option value="off">Off</option>
          <option value="daily">Daily</option>
          <option value="weekly">Weekly</option>
        </select>
        <label htmlFor="backup-keep">Copies to keep</label>
        <input
          id="backup-keep"
          className="input input--number"
          type="number"
          min={1}
          max={100}
          value={keep}
          disabled={interval === "off"}
          onChange={(event) => void save(interval, clamp(Number(event.target.value) || 1, 1, 100))}
        />
      </div>
      <div className="settings__row">
        <button type="button" className="btn" disabled={busy} aria-busy={busy} onClick={() => void runNow()}>
          {busy ? "Backing up…" : "Back up now"}
        </button>
        {status && (
          <span className="quiet" role="status">
            {status}
          </span>
        )}
      </div>
    </section>
  );
}

/** Sources, backups, the privacy boundary, and the archive-wide forget. */
export default function SettingsDialog({
  agents,
  busyAgent,
  onAddRoot,
  onRemoveRoot,
  onForgetEverything,
  onClose,
}: {
  agents: DetectedAgent[];
  busyAgent: string | null;
  onAddRoot: (agent: DetectedAgent) => void;
  onRemoveRoot: (agent: DetectedAgent, path: string) => void;
  onForgetEverything: () => void;
  onClose: () => void;
}) {
  return (
    <Modal labelledBy="settings-title" className="settings" onClose={onClose}>
      <header className="modal__head">
        <h2 id="settings-title" className="modal__title">
          Settings
        </h2>
        <button type="button" className="btn btn--quiet" aria-label="Close settings" onClick={onClose}>
          Close
        </button>
      </header>
      <div className="settings__body">
        <Sources agents={agents} busyAgent={busyAgent} onAddRoot={onAddRoot} onRemoveRoot={onRemoveRoot} />
        <Backups />
        <section className="settings__section" aria-labelledby="privacy-heading">
          <h3 id="privacy-heading" className="subhead">
            Privacy
          </h3>
          <ul className="facts-list">
            <li>The archive stays on this machine. Lore opens no network connection on its own.</li>
            <li>
              Flagged secrets are redacted from search and default exports. Redaction is a safeguard, not a promise
              that nothing secret remains.
            </li>
            <li>
              File permissions are not encryption. Use disk encryption and a locked login. Lore cannot stop another
              process running as you.
            </li>
            <li>Lore never deletes your original agent logs or exports you saved elsewhere.</li>
          </ul>
        </section>
        <section className="settings__section" aria-labelledby="data-heading">
          <h3 id="data-heading" className="subhead">
            Forget everything
          </h3>
          <p className="quiet" id="data-desc">
            Removes every session, repository, finding, and backup from Lore. Original agent logs stay where they are.
            SSDs may keep old blocks until they are reused.
          </p>
          <button type="button" className="btn btn--danger" aria-describedby="data-desc" onClick={onForgetEverything}>
            Forget everything…
          </button>
        </section>
      </div>
    </Modal>
  );
}
