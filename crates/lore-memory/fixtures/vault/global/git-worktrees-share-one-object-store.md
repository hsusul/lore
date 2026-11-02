---
id: 01JC9V6Q4M8K2T7X3N5R0B1H11
type: fact
title: Git worktrees share one object store
scope: global
status: active
created: 2026-10-16
---
Every worktree of a repository reads and writes the same objects and refs, so
a fetch in one worktree is visible in all of them.
