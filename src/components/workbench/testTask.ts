import type { TaskDto } from "../../ipc";

/** A test task with sensible defaults. */
export function task(overrides: Partial<TaskDto> = {}): TaskDto {
  return {
    id: "t1",
    title: "Fix parser",
    prompt: "fix it",
    agent: "claude_code",
    state: "running",
    repo_path: "/repo",
    worktree_path: "/lore/worktrees/fix-parser",
    branch: "lore/fix-parser",
    base_commit: "abc123",
    created_at_ms: Date.now() - 120_000,
    exit_code: null,
    commits_ahead: 2,
    changed_files: ["src/a.rs", "src/b.rs"],
    last_activity: "Editing src/a.rs",
    permission: "edits",
    runs: 1,
    uncommitted_count: 0,
    overlaps: [],
    ...overrides,
  };
}
