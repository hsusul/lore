---
id: 01JC9V6Q4M8K2T7X3N5R0B1H07
type: decision
title: The index lives outside lore.db
scope: repo:3f9a1c0b5e2d7a46
status: active
created: 2026-10-12
---
The memory index is its own SQLite file at `.lore/index.db`. The archive has
one writer lock at a time, and agent traffic must never wait on a scan.

## Timeline
- 2026-10-12 decided in the memory design review
