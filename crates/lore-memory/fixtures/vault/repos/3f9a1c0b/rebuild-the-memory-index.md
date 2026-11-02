---
id: 01JC9V6Q4M8K2T7X3N5R0B1H14
type: howto
title: Rebuild the memory index
scope: repo:3f9a1c0b
status: active
anchors:
  - path: crates/lore-memory/src/index.rs
    commit: 067ef8d4c1a2b3e4f5a6b7c8d9e0f1a2b3c4d5e6
created: 2026-10-19
---
Run `lorectl mem reindex`. It reads every note in the vault and replaces the
SQLite index in one rename.
