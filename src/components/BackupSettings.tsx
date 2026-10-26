import { useEffect, useState } from "react";

import { clamp } from "../format";
import {
  backupNow,
  getBackupSchedule,
  setBackupSchedule,
  type BackupInterval,
} from "../ipc";

const INTERVALS: { value: BackupInterval; label: string }[] = [
  { value: "off", label: "Off" },
  { value: "daily", label: "Daily" },
  { value: "weekly", label: "Weekly" },
];

/**
 * Backup cadence controls: automatic interval (off/daily/weekly), how many
 * newest copies to retain, and an on-demand "Back up now". Reads and persists
 * through the settings-backed schedule; backups are Lore-owned local snapshots,
 * removed by "Forget everything".
 */
export default function BackupSettings() {
  const [interval, setIntervalValue] = useState<BackupInterval>("off");
  const [keep, setKeep] = useState(7);
  const [status, setStatus] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    let alive = true;
    getBackupSchedule()
      .then((s) => {
        if (alive) {
          setIntervalValue(s.interval as BackupInterval);
          setKeep(s.keep);
        }
      })
      .catch(() => {});
    return () => {
      alive = false;
    };
  }, []);

  async function persist(nextInterval: BackupInterval, nextKeep: number) {
    setIntervalValue(nextInterval);
    setKeep(nextKeep);
    setStatus(null);
    try {
      await setBackupSchedule(nextInterval, nextKeep);
    } catch (e) {
      setStatus(`Could not save: ${String(e)}`);
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
    <section aria-labelledby="backups-heading" aria-describedby="backups-desc">
      <h3 id="backups-heading" className="section-title">
        Backups
      </h3>
      <p id="backups-desc" className="empty">
        Local, Lore-owned snapshots of your archive, kept private on this machine and removed by
        "Forget everything".
      </p>
      <div className="settings__row">
        <label htmlFor="backup-interval">Automatic backups</label>
        <select
          id="backup-interval"
          value={interval}
          onChange={(event) => void persist(event.target.value as BackupInterval, keep)}
        >
          {INTERVALS.map((option) => (
            <option key={option.value} value={option.value}>
              {option.label}
            </option>
          ))}
        </select>
      </div>
      <div className="settings__row">
        <label htmlFor="backup-keep">Keep newest</label>
        <input
          id="backup-keep"
          type="number"
          min={1}
          max={100}
          value={keep}
          disabled={interval === "off"}
          onChange={(event) =>
            void persist(interval, clamp(Number(event.target.value) || 1, 1, 100))
          }
        />
      </div>
      <button
        type="button"
        className="btn--ghost"
        onClick={() => void runNow()}
        disabled={busy}
        aria-busy={busy}
      >
        {busy ? "Backing up…" : "Back up now"}
      </button>
      {status && (
        <p className="settings__status" role="status">
          {status}
        </p>
      )}
    </section>
  );
}
