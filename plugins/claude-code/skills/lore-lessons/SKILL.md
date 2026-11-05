---
name: lore-lessons
description: Turn the corrections a person gave coding agents in past sessions into short lessons filed in the Lore inbox for review. Use when asked to find lessons, learn from past corrections, or mine Lore candidates.
allowed-tools: mcp__plugin_lore_lore__recall_memory mcp__plugin_lore_lore__mine_candidates
---

# Lore lessons

You turn this person's past corrections into short rules they can review. The tools come from Lore's MCP server. Each rule you file lands in Lore's inbox as a proposal. The person accepts or rejects it later.

## 1. Learn what already exists

Call `recall_memory` with `cwd` set to the absolute path of the current directory. Leave out `query`. Read every note it returns. Never propose a rule that an existing note already states. If a rule sharpens an existing note, pass that note's id as `updates`.

## 2. Read every candidate

Call `mine_candidates` with `cwd` set to the absolute path of the current directory. While its `next_cursor` is not null, call it again with the same `cwd` and `cursor` set to that value. Read every page before you decide anything. If there are no candidates, say so and stop.

Each candidate has the person's words, a suggested rule, receipts, and `may_conflict_with`. A receipt names the session, agent, date, and repository, and quotes what the person said. The reply's `cwd_repo` is the repository at the current directory.

## 3. Keep standing rules only

Keep a candidate only when it states a rule a future session should follow. Drop these.

- Questions.
- One-off orders about a specific branch, PR, file, or task.
- Explanations of what happened.
- Venting.

## 4. Group by meaning

Put candidates that mean the same thing in one group, even when the words differ or have typos. One group becomes one rule. The group keeps all its receipts.

## 5. Check later turns

For each group, read its `may_conflict_with` turns. If a later turn overrides the rule, leave the group out. If a later turn only narrows it, keep the rule and say so in the body. A turn about something else is no conflict.

## 6. Write the rule

Write each rule as one short imperative sentence in plain words, such as "Run the tests before you push." Start from the suggested rule, then fix its wording and typos. Leave out branch, PR, and file names unless the rule is about them. Keep it under 120 characters.

## 7. File each rule once

Call `propose_memory` once per rule with these fields.

- `title` is the rule.
- `body` is one sentence of context, then a Receipts list. Each item quotes the person's words and names the agent, session, and date from one receipt.
- `type` is `convention`, or `pitfall` when the rule warns against a mistake.
- `scope` is `repo`, with `cwd` set to the current directory, when `cwd_repo` is not null and every receipt's `repo.scope` equals `cwd_repo.scope`. Otherwise it is `global`.
- `session_id` is the session of the newest receipt.

If `propose_memory` refuses the session because another agent recorded it, use the newest receipt your own agent recorded. If there is none, leave out `session_id`.

A body looks like this.

```
Said in 2 sessions while reviewing pull requests.

Receipts:
- "always run cargo fmt before pushing" (claude-code, session 0193a, 2026-10-12)
- "you forgot fmt again, run it before every push" (codex, session 0194c, 2026-10-20)
```

## 8. Report

End with two short lists.

- Filed: each rule with its proposal id.
- Dropped: each candidate or group you left out, with the reason.

Then tell the person to run `lorectl init --review` to accept or reject the proposals.

## Hard rules

- Never call `propose_memory` for anything a `mine_candidates` receipt does not back.
- Never invent sessions, dates, or quotes. Copy them from the receipts.
- Never accept, edit, or retire notes. Only the person does that.
