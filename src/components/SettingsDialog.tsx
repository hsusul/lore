import { clsx } from "clsx";
import { DatabaseBackup, HardDrive, Hand, Search, Settings, type LucideIcon } from "lucide-react";
import { useEffect, useRef, useState } from "react";

import { clamp } from "../format";
import { PREFERENCES, type PreferenceName, type Preferences, type SetPreference } from "../hooks/usePreferences";
import {
  backupNow,
  getBackupSchedule,
  setBackupSchedule,
  type BackupInterval,
  type DetectedAgent,
  type ForgetReport,
  type SearchSort,
} from "../ipc";
import { SORTS } from "./SearchField";
import { Alert, type AlertSpec } from "./Sheets";
import { Button, ICON } from "./ui/button";
import { Dialog, DialogContent, DialogTitle } from "./ui/dialog";
import { Footnote, FormGroup, FormRow, GroupHeader, TextField } from "./ui/form";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "./ui/select";

export type SettingsPane = "general" | "sources" | "search" | "backups" | "privacy";

const PANES: { id: SettingsPane; label: string; icon: LucideIcon }[] = [
  { id: "general", label: "General", icon: Settings },
  { id: "sources", label: "Sources", icon: HardDrive },
  { id: "search", label: "Search", icon: Search },
  { id: "backups", label: "Backups", icon: DatabaseBackup },
  { id: "privacy", label: "Privacy", icon: Hand },
];

const BACKUP_INTERVALS: { value: BackupInterval; label: string }[] = [
  { value: "off", label: "Off" },
  { value: "daily", label: "Daily" },
  { value: "weekly", label: "Weekly" },
];

/** A message a pane shows in place, under the thing it is about. */
type Status = { tone: "info" | "error"; text: string } | null;

function StatusNote({ status }: { status: Status }) {
  if (!status) return null;
  return <Footnote role={status.tone === "error" ? "alert" : "status"}>{status.text}</Footnote>;
}

/** A macOS pop-up button in a form row. */
function PopUp<T extends string>({
  id,
  value,
  choices,
  onChange,
  disabled,
}: {
  id: string;
  value: T;
  choices: readonly { value: T; label: string }[];
  onChange: (value: T) => void;
  disabled?: boolean;
}) {
  return (
    <Select value={value} onValueChange={(next) => onChange(next as T)} disabled={disabled}>
      <SelectTrigger id={id}>
        <SelectValue />
      </SelectTrigger>
      <SelectContent>
        {choices.map((choice) => (
          <SelectItem key={choice.value} value={choice.value}>
            {choice.label}
          </SelectItem>
        ))}
      </SelectContent>
    </Select>
  );
}

function PreferenceRow({
  name,
  prefs,
  onPreference,
  description,
}: {
  name: PreferenceName;
  prefs: Preferences;
  onPreference: SetPreference;
  description?: string;
}) {
  const pref = PREFERENCES[name];
  const choices: readonly { value: string; label: string }[] = pref.choices;
  const id = `pref-${name}`;
  return (
    <FormRow label={pref.title} labelFor={id} description={description}>
      <PopUp id={id} value={prefs[name]} choices={choices} onChange={(value) => onPreference(name, value)} />
    </FormRow>
  );
}

function General({ prefs, onPreference }: { prefs: Preferences; onPreference: SetPreference }) {
  return (
    <>
      <FormGroup>
        <PreferenceRow name="theme" prefs={prefs} onPreference={onPreference} />
        <PreferenceRow
          name="density"
          prefs={prefs}
          onPreference={onPreference}
          description="Row height in the sidebar and the session list"
        />
      </FormGroup>
      <GroupHeader>Transcript</GroupHeader>
      <FormGroup>
        <PreferenceRow name="toolCalls" prefs={prefs} onPreference={onPreference} />
      </FormGroup>
      <Footnote>Collapsed shows one line per tool call. Expanded also shows each call’s input and output.</Footnote>
    </>
  );
}

function AgentSources({
  agent,
  onAddRoot,
  onRemoveRoot,
}: {
  agent: DetectedAgent;
  onAddRoot: (agent: DetectedAgent) => Promise<string | null>;
  onRemoveRoot: (agent: DetectedAgent, path: string) => Promise<void>;
}) {
  const [busy, setBusy] = useState(false);
  const [status, setStatus] = useState<Status>(null);
  const facts = [agent.installed ? "Detected" : "Not detected", agent.version && `version ${agent.version}`]
    .filter(Boolean)
    .join(", ");

  async function run(task: () => Promise<Status>, failure: string) {
    setBusy(true);
    setStatus(null);
    try {
      setStatus(await task());
    } catch (e) {
      setStatus({ tone: "error", text: `${failure} ${String(e)}` });
    } finally {
      setBusy(false);
    }
  }

  const add = () =>
    run(async () => {
      const path = await onAddRoot(agent);
      return path === null ? null : { tone: "info", text: `Lore is scanning ${path} now.` };
    }, "Lore could not add the folder.");

  const remove = (path: string) =>
    run(async () => {
      await onRemoveRoot(agent, path);
      return { tone: "info", text: `Lore no longer reads ${path}. Sessions already archived stay.` };
    }, "Lore could not remove the folder.");

  return (
    <section aria-label={agent.display_name}>
      <GroupHeader>{agent.display_name}</GroupHeader>
      <FormGroup>
        <FormRow label={facts}>
          <span className="text-callout tabular-nums text-label-2">
            {agent.session_count} {agent.session_count === 1 ? "session" : "sessions"}
          </span>
        </FormRow>
        {agent.roots.map((root) => {
          const custom = agent.custom_roots.includes(root);
          return (
            <FormRow
              key={root}
              label={<code className="block truncate font-mono text-code" title={root}>{root}</code>}
              description={custom ? "Added by you" : "Default"}
            >
              {custom && (
                <Button aria-label={`Remove ${root} from ${agent.display_name}`} disabled={busy} onClick={() => void remove(root)}>
                  Remove
                </Button>
              )}
            </FormRow>
          );
        })}
      </FormGroup>
      <div className="mt-2">
        <Button aria-label={`Add a ${agent.display_name} Folder…`} disabled={busy} onClick={() => void add()}>
          Add Folder…
        </Button>
      </div>
      <StatusNote status={status} />
    </section>
  );
}

function Sources({
  agents,
  onAddRoot,
  onRemoveRoot,
}: {
  agents: DetectedAgent[];
  onAddRoot: (agent: DetectedAgent) => Promise<string | null>;
  onRemoveRoot: (agent: DetectedAgent, path: string) => Promise<void>;
}) {
  return (
    <>
      <Footnote>
        Lore reads these folders and never writes to them. Add a folder if an agent keeps its history somewhere else.
      </Footnote>
      {agents.length === 0 ? (
        <Footnote>Agent status is unavailable.</Footnote>
      ) : (
        agents.map((agent) => <AgentSources key={agent.id} agent={agent} onAddRoot={onAddRoot} onRemoveRoot={onRemoveRoot} />)
      )}
    </>
  );
}

function SearchPane({ sort, onSort }: { sort: SearchSort; onSort: (sort: SearchSort) => void }) {
  return (
    <>
      <FormGroup>
        <FormRow label="Sort results by" labelFor="search-sort">
          <PopUp id="search-sort" value={sort} choices={SORTS} onChange={onSort} />
        </FormRow>
      </FormGroup>
      <Footnote>
        The order applies to searches in this window. Filters such as agent:codex, repo:, path:, has:patch and has:error
        narrow a search, and ⌘/ lists them all.
      </Footnote>
    </>
  );
}

function Backups() {
  const [interval, setIntervalValue] = useState<BackupInterval>("off");
  const [keep, setKeep] = useState(7);
  const [status, setStatus] = useState<Status>(null);
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    let alive = true;
    getBackupSchedule().then(
      (schedule) => {
        if (!alive) return;
        setIntervalValue(schedule.interval as BackupInterval);
        setKeep(schedule.keep);
      },
      (e: unknown) => alive && setStatus({ tone: "error", text: `Lore could not read the backup schedule. ${String(e)}` }),
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
      setStatus({ tone: "error", text: `Lore could not save the schedule. ${String(e)}` });
    }
  }

  async function runNow() {
    setBusy(true);
    setStatus(null);
    try {
      await backupNow();
      setStatus({ tone: "info", text: "Backup created." });
    } catch (e) {
      setStatus({ tone: "error", text: `The backup failed. ${String(e)}` });
    } finally {
      setBusy(false);
    }
  }

  return (
    <>
      <FormGroup>
        <FormRow label="Automatic backups" labelFor="backup-interval">
          <PopUp id="backup-interval" value={interval} choices={BACKUP_INTERVALS} onChange={(next) => void save(next, keep)} />
        </FormRow>
        <FormRow label="Copies to keep" labelFor="backup-keep">
          <TextField
            id="backup-keep"
            className="w-number-field text-right tabular-nums"
            type="number"
            min={1}
            max={100}
            value={keep}
            disabled={interval === "off"}
            onChange={(event) => void save(interval, clamp(Number(event.target.value) || 1, 1, 100))}
          />
        </FormRow>
        <FormRow label="Back up the archive now">
          <Button disabled={busy} aria-busy={busy} onClick={() => void runNow()}>
            {busy ? "Backing Up…" : "Back Up Now"}
          </Button>
        </FormRow>
      </FormGroup>
      <StatusNote status={status} />
      <Footnote>Backups are private copies of the archive on this Mac. Forget Everything removes them too.</Footnote>
    </>
  );
}

function Privacy({ onForgetEverything }: { onForgetEverything: () => Promise<ForgetReport> }) {
  const [alert, setAlert] = useState<AlertSpec | null>(null);
  const [status, setStatus] = useState<Status>(null);

  const ask = () =>
    setAlert({
      title: "Forget everything in Lore?",
      body: "Every session, repository, finding and backup leaves Lore. Your original agent logs stay where they are. This cannot be undone.",
      action: "Forget Everything",
      onConfirm: async () => {
        const report = await onForgetEverything();
        setStatus({ tone: "info", text: `Lore forgot everything and removed ${report.blobs_removed} stored files.` });
      },
    });

  return (
    <>
      <Footnote>
        The archive stays on this Mac, and Lore opens no network connection on its own. Flagged secrets are redacted from
        search and default exports; redaction is a safeguard, not a promise that nothing secret remains. File permissions
        are not encryption, so use disk encryption and a locked login: Lore cannot stop another process running as you.
        Lore never deletes your original agent logs or exports you saved elsewhere.
      </Footnote>
      <GroupHeader>Archive</GroupHeader>
      <FormGroup>
        <FormRow
          label="Forget everything"
          description="Removes every session, repository, finding and backup from Lore. SSDs may keep old blocks until they are reused."
        >
          <Button destructive onClick={ask}>
            Forget Everything…
          </Button>
        </FormRow>
      </FormGroup>
      <StatusNote status={status} />
      {alert && <Alert alert={alert} onClose={() => setAlert(null)} />}
    </>
  );
}

/**
 * Settings in System Settings' shape (spec 9.11, 18.12): a 720 x 520 sheet with a pane
 * list on the left and grouped inset forms on the right, and Done at the bottom right
 * on the sheet itself. Arrow keys move between panes, as in a source list.
 */
export default function SettingsDialog({
  initialPane = "general",
  prefs,
  onPreference,
  sort,
  onSort,
  agents,
  onAddRoot,
  onRemoveRoot,
  onForgetEverything,
  onClose,
}: {
  initialPane?: SettingsPane;
  prefs: Preferences;
  onPreference: SetPreference;
  sort: SearchSort;
  onSort: (sort: SearchSort) => void;
  agents: DetectedAgent[];
  onAddRoot: (agent: DetectedAgent) => Promise<string | null>;
  onRemoveRoot: (agent: DetectedAgent, path: string) => Promise<void>;
  onForgetEverything: () => Promise<ForgetReport>;
  onClose: () => void;
}) {
  const [pane, setPane] = useState<SettingsPane>(initialPane);
  const tabs = useRef<(HTMLButtonElement | null)[]>([]);
  const current = PANES.find((p) => p.id === pane) ?? PANES[0];

  function onKeyDown(event: React.KeyboardEvent) {
    const from = PANES.findIndex((p) => p.id === pane);
    const step = event.key === "ArrowDown" ? 1 : event.key === "ArrowUp" ? -1 : 0;
    const to = from + step;
    if (step === 0 || to < 0 || to >= PANES.length) return;
    event.preventDefault();
    setPane(PANES[to].id);
    tabs.current[to]?.focus();
  }

  return (
    <Dialog open onOpenChange={(open) => !open && onClose()}>
      <DialogContent className="grid h-settings-h w-settings grid-cols-settings overflow-hidden" aria-describedby={undefined}>
        <DialogTitle className="sr-only">Settings</DialogTitle>
        <div
          role="tablist"
          aria-label="Settings panes"
          aria-orientation="vertical"
          className="group/panes flex flex-col bg-bg-sidebar px-2.5 pt-3 shadow-hairline-r select-none"
          onKeyDown={onKeyDown}
        >
          {PANES.map((p, index) => {
            const Icon = p.icon;
            const selected = p.id === pane;
            const onSelected = "group-focus-within/panes:text-on-selected";
            return (
              <button
                key={p.id}
                ref={(el) => {
                  tabs.current[index] = el;
                }}
                type="button"
                role="tab"
                id={`settings-tab-${p.id}`}
                aria-selected={selected}
                aria-controls="settings-pane"
                tabIndex={selected ? 0 : -1}
                className={clsx(
                  "flex h-nav-row w-full flex-none items-center gap-1.5 rounded-control px-2 text-body text-label outline-none",
                  selected && ["bg-selected-unfocused group-focus-within/panes:bg-selected", onSelected],
                )}
                onClick={() => setPane(p.id)}
              >
                <Icon className={clsx(ICON.body, "text-accent", selected && onSelected)} aria-hidden="true" />
                {p.label}
              </button>
            );
          })}
        </div>
        <div className="flex min-h-0 flex-col">
          <div
            id="settings-pane"
            role="tabpanel"
            aria-labelledby={`settings-tab-${pane}`}
            className="min-h-0 flex-1 overflow-y-auto px-6 pb-3 pt-5"
          >
            <h2 className="mb-4 text-title3">{current.label}</h2>
            {pane === "general" && <General prefs={prefs} onPreference={onPreference} />}
            {pane === "sources" && <Sources agents={agents} onAddRoot={onAddRoot} onRemoveRoot={onRemoveRoot} />}
            {pane === "search" && <SearchPane sort={sort} onSort={onSort} />}
            {pane === "backups" && <Backups />}
            {pane === "privacy" && <Privacy onForgetEverything={onForgetEverything} />}
          </div>
          <div className="flex flex-none justify-end px-5 pb-4 pt-2">
            <Button variant="default" onClick={onClose}>
              Done
            </Button>
          </div>
        </div>
      </DialogContent>
    </Dialog>
  );
}
