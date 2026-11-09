---
name: lore-lessons
description: Turn the corrections a person gave coding agents in past sessions into short lessons filed in the Lore inbox for review. Use when asked to find lessons, learn from past corrections, or mine Lore candidates.
allowed-tools: mcp__plugin_lore_lore__recall_memory mcp__plugin_lore_lore__mine_candidates
---

# Lore lessons

You turn this person's past corrections into short rules they can review. The tools come from Lore's MCP server. Each rule you file lands in Lore's inbox as a proposal. The person accepts or rejects it later.

## 1. Learn what already exists

Call `recall_memory` with `cwd` set to the absolute path of the current directory. Leave out `query`. Read every note it returns. Never propose a new rule that an existing note already states. If a rule repeats or sharpens an existing note, file it with that note's id as `updates`, so the note gains the receipts.

## 2. Read every candidate

Call `mine_candidates` with `cwd` set to the absolute path of the current directory. While its `next_cursor` is not null, call it again with the same `cwd` and `cursor` set to that value. Read every page before you decide anything. If there are no candidates, say so and stop.

Each candidate has the person's words, a suggested rule, receipts, and `may_conflict_with`. A receipt names the session, agent, date, and repository. It quotes the person's `correction`, what the agent had just done as `before`, and the agent's reply as `after`. The reply's `cwd_repo` is the repository at the current directory.

Lore lists only candidates with a receipt no earlier run has read. Each receipt says whether it is `new`. Only `new` receipts need a rule. Older receipts are context: use them to group and to understand, but do not file a rule again for what they already said. If a new receipt repeats or sharpens a note you read in step 1, file it with that note's id as `updates`. Do not drop it as already covered: the person repeating a rule is a receipt for it.

Receipts with `signal: "reply"` are ordinary replies the rules did not flag, and most of them correct nothing. Apply the same steps to them, and drop each one that does not correct the agent or state a standing rule.

## 3. Understand each candidate

A correction alone is often unclear. "no, cause you never get to access it" means nothing without the turn it answers. For every receipt, read `before`, then `correction`, then `after`, and work out two things.

- What the agent did that the person did not want.
- What the person wanted instead.

If `before` and `after` do not make both clear, drop the candidate. Do not guess.

## 4. Keep standing rules only

Keep a candidate only when it states a rule a future session should follow. One receipt is enough.

Feedback on the style, format, length, or wording of something the agent wrote, such as a doc, a message, a resume, or a debate block, is a standing rule when it would apply to the next thing of that kind. Write the rule for that kind of output.

Drop these.

- Questions.
- One-off orders about a specific branch, PR, file, or task.
- Explanations of what happened.
- Venting.

## 5. Group by meaning

Put candidates that mean the same thing in one group, even when the words differ or have typos. One group becomes one rule. The group keeps all its receipts.

## 6. Check later turns

For each group, read its `may_conflict_with` turns. If a later turn overrides the rule, leave the group out. If a later turn only narrows it, keep the rule and say so in the body. A turn about something else is no conflict.

## 7. Write the rule

Write each rule as one short imperative sentence in plain words, such as "Run the tests before you push." It must make sense to an agent that never saw the conversation.

- Name the thing the rule is about. Write "Keep docs out of git", not "Never track them".
- Never start with it, this, that, they, cause, because, so, yes, or no. Lore refuses those titles.
- Build the rule from what you worked out in step 3, not from the person's words alone. Fix wording and typos.
- Leave out branch, PR, and file names unless the rule is about them. Keep it under 120 characters.

## 8. File each rule once

Call `propose_memory` once per rule with these fields.

- `title` is the rule.
- `body` is one sentence on what the agent did and what the person wanted instead, then a Receipts list with every new receipt in the group. Each item quotes the person's words and names the agent, session, and date from one receipt.
- `type` is `convention`, or `pitfall` when the rule warns against a mistake.
- `scope` is `repo` when every receipt's `repo.scope` is the same and not null, even when it is not `cwd_repo.scope`. Set `cwd` to the current directory when `cwd_repo.scope` matches, else to the `repo_path` of a receipt whose `repo_path` is not null. Otherwise `scope` is `global`. If `propose_memory` refuses that `cwd`, try the other receipts' `repo_path`s. If it refuses them all, or none is set, drop the rule and say why. Never file it as `global` instead.
- `session_id` is the session of the newest receipt that your own agent recorded (Claude Code receipts when you are Claude Code, Codex receipts when you are Codex). Leave it out when no receipt in the group is your agent's: Lore refuses another agent's session.
- `enforce` only when the rule is mechanical and breaking it shows in one tool call, such as AI attribution in a commit message or a planning doc being written. Give one narrow `pattern` for the exact violation, with `tool` set to `bash`, `write`, or `path`, and `unless` for a known safe form. Leave it out for any rule that needs judgment, because a hook will deny every call the pattern matches.

If `propose_memory` refuses the session because another agent recorded it, use the newest receipt your own agent recorded. If there is none, leave out `session_id`.

If `propose_memory` answers `already_filed`, a note or a proposal waiting for review already says the rule. Skip it and count it as already filed in your report.

A body looks like this.

```
The agent pushed without formatting, and the person wanted `cargo fmt` run first.

Receipts:
- "always run cargo fmt before pushing" (claude-code, session 0193a, 2026-10-12)
- "you forgot fmt again, run it before every push" (codex, session 0194c, 2026-10-20)
```

## 9. Report

End with two short lists.

- Filed: each rule with its proposal id.
- Dropped: each candidate or group you left out, with the reason.

Then tell the person to run `lorectl init --review` to accept or reject the proposals.

## Hard rules

- Never call `propose_memory` for anything a `mine_candidates` receipt does not back.
- Never invent sessions, dates, or quotes. Copy them from the receipts.
- Never accept, edit, or retire notes. Only the person does that.
