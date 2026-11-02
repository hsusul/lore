---
id: 01JC9V6Q4M8K2T7X3N5R0B1H02
type: convention
title: Name every package in CI
scope: repo:3f9a1c0b5e2d7a46
status: active
anchors:
  - path: .github/workflows/ci.yml
    lines: 30-33
sources:
  - claude-code:7d2f9a1e
created: 2026-10-05
---
CI runs clippy and the tests with an explicit `-p` per package. A new crate is
not checked until its name is added to both lists.

## Timeline
- 2026-10-05 accepted from claude-code:7d2f9a1e
