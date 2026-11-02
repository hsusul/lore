---
id: 01JC9V6Q4M8K2T7X3N5R0B1H06
type: decision
title: lexopt instead of clap
scope: repo:3f9a1c0b
status: active
anchors:
  - path: crates/lorectl/Cargo.toml
sources:
  - codex:rollout-2026-09-30T11-02-44
created: 2026-10-11
---
`lorectl` parses arguments with `lexopt`, a zero dependency parser, so the
egress audit of its dependency graph stays small enough to read.
