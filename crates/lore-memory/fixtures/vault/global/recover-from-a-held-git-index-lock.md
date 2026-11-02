---
id: 01JC9V6Q4M8K2T7X3N5R0B1H15
type: howto
title: Recover from a held git index lock
scope: global
status: active
sources:
  - codex:rollout-2026-10-21T08-15-00
created: 2026-10-21
---
If `.git/index.lock` exists and no git process is running, the lock is stale.
Delete the lock file and run the command again.
