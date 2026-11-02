---
id: 01JC9V6Q4M8K2T7X3N5R0B1H03
type: convention
title: Library code never panics on bad input
scope: repo:3f9a1c0b5e2d7a46
status: active
anchors:
  - path: Cargo.toml
    lines: 24-29
    hash: b3:5be0c1d2
    commit: 067ef8d
created: 2026-10-06
---
The workspace warns on `unwrap`, `expect`, and `panic` in library code. Return
an error that names the bad field instead of a panic.

## Timeline
- 2026-10-06 written by hand
