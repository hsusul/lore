import { beforeEach, describe, expect, it, vi } from "vitest";

const invoke = vi.fn();
const open = vi.fn();
vi.mock("@tauri-apps/api/core", () => ({ invoke: (...a: unknown[]) => invoke(...a) }));
vi.mock("@tauri-apps/plugin-dialog", () => ({ open: (...a: unknown[]) => open(...a) }));

import {
  chooseRepositoryDirectory,
  commitTask,
  continueTask,
  createTask,
  discardTask,
  listTasks,
  listWorkspaceDir,
  mergeTask,
  openTaskWorktree,
  openWorkspace,
  readWorkspaceFile,
  stopTask,
  taskActivity,
  taskDiff,
  taskLoadWarning,
} from "./ipc";

beforeEach(() => {
  invoke.mockReset();
  open.mockReset();
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
