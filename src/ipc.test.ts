import { beforeEach, describe, expect, it, vi } from "vitest";

const invoke = vi.fn();
const open = vi.fn();
const listen = vi.fn();
vi.mock("@tauri-apps/api/core", () => ({ invoke: (...a: unknown[]) => invoke(...a) }));
vi.mock("@tauri-apps/api/event", () => ({ listen: (...a: unknown[]) => listen(...a) }));
vi.mock("@tauri-apps/plugin-dialog", () => ({ open: (...a: unknown[]) => open(...a) }));

import {
  cancelMergeQueue,
  chooseRepositoryDirectory,
  commitTask,
  continueTask,
  createTask,
  discardTask,
  getMergeQueue,
  getRepoSettings,
  getTask,
  listDecisions,
  listTasks,
  onTasksChanged,
  listWorkspaceDir,
  mergeTask,
  openTaskWorktree,
  openWorkspace,
  readWorkspaceFile,
  setRepoTestCommand,
  startMergeQueue,
  stopTask,
  taskActivity,
  taskDiff,
  taskLoadWarning,
} from "./ipc";

beforeEach(() => {
  invoke.mockReset();
  open.mockReset();
  listen.mockReset();
});

describe("ipc contract", () => {
  it("task commands invoke their Tauri commands with the right arguments", async () => {
    invoke.mockResolvedValue(undefined);
    const request = {
      repo_path: "/repo",
      title: "t",
      prompt: "p",
      agent: "claude_code" as const,
      permission: "auto" as const,
    };
    await listTasks();
    await createTask(request);
    await stopTask("t1");
    await discardTask("t1");
    await openTaskWorktree("t1");
    await taskLoadWarning();
    expect(invoke.mock.calls).toEqual([
      ["list_tasks"],
      ["create_task", { request }],
      ["stop_task", { id: "t1" }],
      ["discard_task", { id: "t1" }],
      ["open_task_worktree", { id: "t1" }],
      ["task_load_warning"],
    ]);
  });

  it("continue, commit, and merge pass their arguments in the shapes the backend expects", async () => {
    invoke.mockResolvedValue(undefined);
    await continueTask({ id: "t1", prompt: "more" });
    await continueTask({ id: "t1", prompt: "take over", agent: "codex" });
    await commitTask("t1", "");
    await mergeTask("t1");
    expect(invoke.mock.calls).toEqual([
      ["continue_task", { request: { id: "t1", prompt: "more" } }],
      ["continue_task", { request: { id: "t1", prompt: "take over", agent: "codex" } }],
      ["commit_task", { id: "t1", message: "" }],
      ["merge_task", { id: "t1" }],
    ]);
  });

  it("workspace and task-detail commands pass camelCase args Tauri maps to snake_case", async () => {
    invoke.mockResolvedValue(undefined);
    await openWorkspace("/repo/src");
    await listWorkspaceDir("/repo", "");
    await readWorkspaceFile("/repo", "src/main.ts");
    await taskDiff("t1");
    await taskActivity("t1");
    expect(invoke.mock.calls).toEqual([
      ["open_workspace", { path: "/repo/src" }],
      ["list_workspace_dir", { root: "/repo", relPath: "" }],
      ["read_workspace_file", { root: "/repo", relPath: "src/main.ts" }],
      ["task_diff", { id: "t1" }],
      ["task_activity", { id: "t1" }],
    ]);
  });

  it("sends force on a commit only when it was asked for, and scopes the decision log", async () => {
    invoke.mockResolvedValue(undefined);
    await getTask("t1");
    await commitTask("t1", "m", true);
    await commitTask("t1", "m", false);
    await listDecisions("/repo", 50);
    await listDecisions();
    expect(invoke.mock.calls).toEqual([
      ["get_task", { id: "t1" }],
      ["commit_task", { id: "t1", message: "m", force: true }],
      ["commit_task", { id: "t1", message: "m" }],
      ["list_decisions", { repoPath: "/repo", limit: 50 }],
      ["list_decisions", { repoPath: null, limit: null }],
    ]);
  });

  it("merge-queue and repo-settings commands pass camelCase args Tauri maps to snake_case", async () => {
    invoke.mockResolvedValue(undefined);
    await getRepoSettings("/repo");
    await setRepoTestCommand("/repo", "npm test");
    await setRepoTestCommand("/repo", null);
    await startMergeQueue("/repo", ["t2", "t1"]);
    await getMergeQueue("/repo");
    await cancelMergeQueue("/repo");
    expect(invoke.mock.calls).toEqual([
      ["get_repo_settings", { repoPath: "/repo" }],
      ["set_repo_test_command", { repoPath: "/repo", command: "npm test" }],
      ["set_repo_test_command", { repoPath: "/repo", command: null }],
      ["start_merge_queue", { repoPath: "/repo", taskIds: ["t2", "t1"] }],
      ["get_merge_queue", { repoPath: "/repo" }],
      ["cancel_merge_queue", { repoPath: "/repo" }],
    ]);
  });

  it("unwraps the tasks_changed payload into the changed ids", async () => {
    const unlisten = vi.fn();
    listen.mockResolvedValue(unlisten);
    const seen: string[][] = [];
    await expect(onTasksChanged((ids) => seen.push(ids))).resolves.toBe(unlisten);
    expect(listen.mock.calls[0][0]).toBe("tasks_changed");
    (listen.mock.calls[0][1] as (e: { payload: { ids: string[] } }) => void)({ payload: { ids: ["a", ""] } });
    expect(seen).toEqual([["a", ""]]);
  });

  it("repository selection uses a single-directory native dialog", async () => {
    open.mockResolvedValue("/repo");
    await expect(chooseRepositoryDirectory()).resolves.toBe("/repo");
    expect(open).toHaveBeenCalledWith({
      directory: true,
      multiple: false,
      title: "Choose repository",
    });
  });
});
