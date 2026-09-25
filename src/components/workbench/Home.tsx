import { formatRelative, formatTime } from "../../format";
import type { TaskDto } from "../../ipc";
import { AgentChip, StateBadge } from "./badges";
import { groupTasks, type BoardGroup } from "./board";
import { Mark, MergeIcon, WarningIcon } from "./icons";
import NewAgentForm from "./NewAgentForm";
import { baseName, shortcut } from "./state";

/** How many merged tasks the board shows before collapsing the rest into a count. */
const MERGED_SHOWN = 4;

type Props = {
  workspace: string | null;
  /** The tasks in scope (the open repository, or every repository). */
  tasks: TaskDto[] | null;
  onSelect: (id: string) => void;
  onCreated: (task: TaskDto) => void;
  onOpenFolder: () => void;
  onOpenMergeQueue: () => void;
  /** Changes whenever something asks to focus the composer. */
  focusToken: number;
  draft?: string;
};

/**
 * The overview stage, shown when no agent or file is open: launch an agent and
 * see every agent at a glance, grouped by what it needs from you.
 */
export default function Home({
  workspace,
  tasks,
  onSelect,
  onCreated,
  onOpenFolder,
  onOpenMergeQueue,
  focusToken,
  draft,
}: Props) {
  const groups = groupTasks(tasks ?? []);
  const count = (id: BoardGroup["id"]) => groups.find((g) => g.id === id)?.tasks.length ?? 0;
  const summary = [
    count("running") > 0 ? `${count("running")} running` : null,
    count("attention") > 0 ? `${count("attention")} need${count("attention") === 1 ? "s" : ""} you` : null,
    count("ready") > 0 ? `${count("ready")} ready to merge` : null,
  ].filter(Boolean);
  const branch = (tasks ?? []).find((t) => t.repo_path === workspace && t.repo_branch)?.repo_branch;

  return (
    <div className="home" role="region" aria-label="Overview">
      <div className="home__inner">
        {workspace ? (
          <header className="home__header">
            <h2 className="home__title">{baseName(workspace)}</h2>
            <p className="home__summary">
              {branch && <span className="mono">{branch}</span>}
              {summary.length > 0 ? summary.map((part) => <span key={part}>{part}</span>) : <span>No agents running</span>}
            </p>
          </header>
        ) : (
          <header className="home__header home__header--welcome">
            <Mark />
            <h2 className="home__title">Lore</h2>
            <p className="home__lede">
              Run coding agents in parallel, each in its own git worktree and branch. Your checkout is not touched.
            </p>
            <div className="home__actions">
              <button type="button" className="wb-btn wb-btn--primary" onClick={onOpenFolder}>
                Open folder…
              </button>
            </div>
          </header>
        )}

        {workspace && (
          <NewAgentForm
            workspace={workspace}
            onCreated={onCreated}
            onOpenFolder={onOpenFolder}
            focusToken={focusToken}
            draft={draft}
          />
        )}

        {tasks === null ? (
          <p className="wb-note" role="status">
            Loading agents…
          </p>
        ) : groups.length === 0 ? (
          workspace && (
            <p className="home__empty">
              No agents yet. Describe a task above and press <kbd>{shortcut("↵")}</kbd> to launch one.
            </p>
          )
        ) : (
          groups.map((group) => (
            <section key={group.id} className={`board board--${group.id}`} aria-label={group.title}>
              <h3 className="board__title">
                {group.title}
                <span className="board__count">{group.tasks.length}</span>
                {group.id === "ready" && (
                  <button type="button" className="wb-btn wb-btn--small board__action" onClick={onOpenMergeQueue}>
                    <MergeIcon /> Merge queue…
                  </button>
                )}
              </h3>
              {group.id === "merged" ? (
                <ul className="board__merged">
                  {group.tasks.slice(0, MERGED_SHOWN).map((task) => (
                    <li key={task.id}>
                      <button type="button" className="board__merged-row" onClick={() => onSelect(task.id)}>
                        <span className="board__merged-title">{task.title}</span>
                        <AgentChip agent={task.agent} />
                        <span className="board__time">{formatRelative(task.created_at_ms)}</span>
                      </button>
                    </li>
                  ))}
                  {group.tasks.length > MERGED_SHOWN && (
                    <li className="board__more">and {group.tasks.length - MERGED_SHOWN} more</li>
                  )}
                </ul>
              ) : (
                <ul className="board__grid">
                  {group.tasks.map((task) => (
                    <li key={task.id}>
                      <TaskCard task={task} onSelect={onSelect} />
                    </li>
                  ))}
                </ul>
              )}
            </section>
          ))
        )}
      </div>
    </div>
  );
}

function TaskCard({ task, onSelect }: { task: TaskDto; onSelect: (id: string) => void }) {
  const files = task.changed_files_total ?? task.changed_files.length;
  const uncommitted = task.uncommitted_count ?? 0;
  const stats = [
    files > 0 ? `${files} ${files === 1 ? "file" : "files"}` : null,
    task.commits_ahead > 0 ? `${task.commits_ahead} ${task.commits_ahead === 1 ? "commit" : "commits"}` : null,
    uncommitted > 0 ? `${uncommitted} uncommitted` : null,
  ].filter(Boolean);
  const problem =
    task.attention ??
    ((task.claim_conflicts?.length ?? 0) > 0 ? "Changed files another agent owns" : null);
  return (
    <button
      type="button"
      className={`card card--${task.state}${problem ? " card--attention" : ""}`}
      aria-label={task.title}
      onClick={() => onSelect(task.id)}
    >
      <span className="card__top">
        <AgentChip agent={task.agent} />
        <time className="card__time" title={formatTime(task.created_at_ms)}>
          {formatRelative(task.created_at_ms)}
        </time>
      </span>
      <span className="card__title">{task.title}</span>
      {problem ? (
        <span className="card__problem">
          <WarningIcon />
          {problem}
        </span>
      ) : (
        task.last_activity && <span className="card__activity">{task.last_activity}</span>
      )}
      <span className="card__meta">
        <StateBadge task={task} compact />
        {stats.map((s) => (
          <span key={s}>{s}</span>
        ))}
      </span>
    </button>
  );
}
