---
id: 01JC9V6Q4M8K2T7X3N5R0B1H09
type: fact
title: FTS5 bm25 scores, lower is better
scope: repo:3f9a1c0b5e2d7a46
status: active
created: 2026-10-14
---
SQLite FTS5 returns `bm25()` as a negative number where the best match is the
most negative, so order by it ascending.
