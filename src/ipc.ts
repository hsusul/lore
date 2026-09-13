// Typed IPC surface over the Tauri commands. The payload types are the
// generated contract from `crates/lore-ipc/bindings` (never hand-edited); this
// module only names the commands and wires argument/return types to them.

import { invoke } from "@tauri-apps/api/core";
import { open } from "@tauri-apps/plugin-dialog";

import type { ActivityDto } from "../crates/lore-ipc/bindings/ActivityDto";
import type { ActivityKind } from "../crates/lore-ipc/bindings/ActivityKind";
import type { ContinueTaskRequest } from "../crates/lore-ipc/bindings/ContinueTaskRequest";
import type { CreateTaskRequest } from "../crates/lore-ipc/bindings/CreateTaskRequest";
import type { DirEntryDto } from "../crates/lore-ipc/bindings/DirEntryDto";
import type { FileContentDto } from "../crates/lore-ipc/bindings/FileContentDto";
import type { MergeResultDto } from "../crates/lore-ipc/bindings/MergeResultDto";
import type { TaskAgent } from "../crates/lore-ipc/bindings/TaskAgent";
import type { TaskDiffDto } from "../crates/lore-ipc/bindings/TaskDiffDto";
import type { TaskDto } from "../crates/lore-ipc/bindings/TaskDto";
import type { TaskOverlapDto } from "../crates/lore-ipc/bindings/TaskOverlapDto";
import type { TaskPermission } from "../crates/lore-ipc/bindings/TaskPermission";
import type { TaskState } from "../crates/lore-ipc/bindings/TaskState";

export type {
  ActivityDto,
  ActivityKind,
  ContinueTaskRequest,
  CreateTaskRequest,
  DirEntryDto,
  FileContentDto,
  MergeResultDto,
  TaskAgent,
  TaskDiffDto,
  TaskDto,
  TaskOverlapDto,
  TaskPermission,
  TaskState,
};

// Dev-only browser preview: outside Tauri (`npm run dev` in a plain browser)
// every command is served by an in-memory mock. `import.meta.env.DEV` is
// statically false in production builds, so the mock module is dropped there;
// tests (MODE "test") never use it unless they import it directly.
type IpcMock = typeof import("./mock/ipc-mock");
const mock: Promise<IpcMock> | null =
  import.meta.env.DEV &&
  import.meta.env.MODE !== "test" &&
  typeof window !== "undefined" &&
  !("__TAURI_INTERNALS__" in window)
    ? import("./mock/ipc-mock")
    : null;

/** Open the OS folder picker to choose a git repository for a task. */
export function chooseRepositoryDirectory(): Promise<string | null> {
  if (mock) return mock.then((m) => m.chooseRepositoryDirectory());
  return open({ directory: true, multiple: false, title: "Choose repository" });
}

/** Orchestrated tasks, newest first. State is refreshed on every call. */
export function listTasks(): Promise<TaskDto[]> {
  if (mock) return mock.then((m) => m.listTasks());
  return invoke<TaskDto[]>("list_tasks");
}

/** Create a Lore-owned worktree for the request and launch its agent. */
export function createTask(request: CreateTaskRequest): Promise<TaskDto> {
  if (mock) return mock.then((m) => m.createTask(request));
  return invoke<TaskDto>("create_task", { request });
}

/** Stop a task's agent and everything it started. */
export function stopTask(id: string): Promise<TaskDto> {
  if (mock) return mock.then((m) => m.stopTask(id));
  return invoke<TaskDto>("stop_task", { id });
}

/**
 * Send more work to a task that is not running and not merged. The same agent
 * continues its session; a different agent is a handoff with a context brief.
 */
export function continueTask(request: ContinueTaskRequest): Promise<TaskDto> {
  if (mock) return mock.then((m) => m.continueTask(request));
  return invoke<TaskDto>("continue_task", { request });
}

/** Commit every uncommitted change in the task worktree (empty message = task title). */
export function commitTask(id: string, message: string): Promise<TaskDto> {
  if (mock) return mock.then((m) => m.commitTask(id, message));
  return invoke<TaskDto>("commit_task", { id, message });
}

/**
 * Merge the task branch into the branch checked out in the user's repository.
 * A conflicting merge is aborted and reported with `merged: false`.
 */
export function mergeTask(id: string): Promise<MergeResultDto> {
  if (mock) return mock.then((m) => m.mergeTask(id));
  return invoke<MergeResultDto>("merge_task", { id });
}

/** Stop the agent and delete the task's Lore-owned worktree and branch. */
export function discardTask(id: string): Promise<void> {
  if (mock) return mock.then((m) => m.discardTask(id));
  return invoke<void>("discard_task", { id });
}

/** Why saved tasks could not be loaded at startup, or null. */
export function taskLoadWarning(): Promise<string | null> {
  if (mock) return mock.then((m) => m.taskLoadWarning());
  return invoke<string | null>("task_load_warning");
}

/** Reveal a task's worktree in the OS file manager. */
export function openTaskWorktree(id: string): Promise<void> {
  if (mock) return mock.then((m) => m.openTaskWorktree(id));
  return invoke<void>("open_task_worktree", { id });
}

/** Resolve any folder inside a git repository to the repository top-level. */
export function openWorkspace(path: string): Promise<string> {
  if (mock) return mock.then((m) => m.openWorkspace(path));
  return invoke<string>("open_workspace", { path });
}

/**
 * List one directory of a workspace (a repository top-level or a task
 * worktree). `relPath` "" is the root. Folders come first; `.git` is hidden.
 */
export function listWorkspaceDir(root: string, relPath: string): Promise<DirEntryDto[]> {
  if (mock) return mock.then((m) => m.listWorkspaceDir(root, relPath));
  return invoke<DirEntryDto[]>("list_workspace_dir", { root, relPath });
}

/** Read a workspace file for display (text is null for binary files). */
export function readWorkspaceFile(root: string, relPath: string): Promise<FileContentDto> {
  if (mock) return mock.then((m) => m.readWorkspaceFile(root, relPath));
  return invoke<FileContentDto>("read_workspace_file", { root, relPath });
}

/** Unified diff of a task's worktree against its base commit, untracked files included. */
export function taskDiff(id: string): Promise<TaskDiffDto> {
  if (mock) return mock.then((m) => m.taskDiff(id));
  return invoke<TaskDiffDto>("task_diff", { id });
}

/** A task's parsed agent activity, oldest first (at most 500 entries). */
export function taskActivity(id: string): Promise<ActivityDto[]> {
  if (mock) return mock.then((m) => m.taskActivity(id));
  return invoke<ActivityDto[]>("task_activity", { id });
}
