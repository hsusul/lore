---
id: 01JC9V6Q4M8K2T7X3N5R0B1H17
type: pitfall
title: Reference::id panics on symbolic refs
scope: repo:3f9a1c0b
status: active
anchors:
  - path: crates/lore-core/src/git.rs
    lines: 210-224
    hash: b3:9c1e04aa
    commit: 067ef8d
relations:
  - updates: 01JC9V6Q4M8K2T7X3N5R0B1H03
sources:
  - claude-code:7d2f9a1e
created: 2026-11-01
forget_after: 2027-05-01
---
Peel a symbolic ref before you call `id()` on it.

## Timeline
- 2026-11-01 accepted from claude-code:7d2f9a1e
