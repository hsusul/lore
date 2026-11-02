---
id: 01JC9V6Q4M8K2T7X3N5R0B1H05
type: decision
title: The vault is the source of truth
scope: repo:3f9a1c0b5e2d7a46
status: active
relations:
  - derives: 01JC9V6Q4M8K2T7X3N5R0B1H07
created: 2026-10-10
---
Notes are markdown files in a git vault. The SQLite index is a cache that
`mem reindex` rebuilds from the vault alone, so deleting it loses nothing.

## Timeline
- 2026-10-10 decided in the memory design review
