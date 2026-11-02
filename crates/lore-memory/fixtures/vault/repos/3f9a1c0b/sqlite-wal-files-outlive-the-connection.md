---
id: 01JC9V6Q4M8K2T7X3N5R0B1H19
type: pitfall
title: SQLite WAL files outlive the connection
scope: repo:3f9a1c0b
status: active
anchors:
  - path: crates/lore-core/src/storage.rs
    lines: 7
created: 2026-10-25
---
A `-wal` and `-shm` file can stay beside a SQLite database after the writer
closes. Never copy the database without them, and never hold a lock on them.
