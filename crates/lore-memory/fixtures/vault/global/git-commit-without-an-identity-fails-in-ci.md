---
id: 01JC9V6Q4M8K2T7X3N5R0B1H20
type: pitfall
title: Git commit without an identity fails in CI
scope: global
status: active
created: 2026-10-26
---
A fresh runner has no `user.email`, so `git commit` refuses to run. Pass an
identity with `-c` only when none is configured.
