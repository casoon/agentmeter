---
title: Findings
description: The rules agentmeter evaluates, how sure each one is, and what it deliberately does not conclude.
order: 1
---

agentmeter reports **observations with evidence**. High usage, large instruction files, long
sessions and expensive models do not prove waste, and usage logs do not show the quality of the
result. Every diagnosis therefore separates what was observed from what is only suspected.

## Levels

| Level | Meaning |
| --- | --- |
| Finding | A verifiable malfunction, for example instructions that are demonstrably cut off at a known limit. |
| Hint | A measured value worth a look. It does not imply a fault and never changes an exit status. |
| Info | Context that helps you read the other numbers. |
| Unknown | The data does not allow a statement. "No findings" and "not enough data" are reported separately. |

Every rule appears once per source with one status: **observed** (with level and measure),
**checked, nothing found** (with the measure and sample, for example "0 of N requests above the
limit") or **not measurable** (with the reason, such as missing cache-write data in Codex logs).

## Conclusions

Every diagnosis opens with one conclusion, so the numbers end in a statement:

| Conclusion | Meaning |
| --- | --- |
| Lever | A setting or habit you control moves the number. Weigh it: the rule says what it would gain and what it costs, never a saving guarantee. |
| Look at it | The logs cannot decide; the cause is not visible. The evidence and the breakdown say where to look. |
| Fine | Measured and nothing to do, for example large contexts that were almost entirely cache reads. |

Below the conclusion, bars split the measure into its parts (causes of cache rebuilds, sessions
holding the large contexts, the parts of a session start). The text output starts with a block
that lists what was read, data that limits a measurement, how many rules could be measured and,
with `--probe`, where Claude Code was asked.

## What every diagnosis carries

Rule id and version, measured value and its denominator, sample size, affected events, a
reference to the local file and event (never prompt text), how sure the rule is, known
counter-examples and one concrete next step to check. JSON output has the same fields.

## Rules

| Id | Level | Measures |
| --- | --- | --- |
| `baseline-load` | Hint | Size of the first observed request of a session: smallest, quartiles, median, largest, and the median per front end the log names (`claude-desktop`, `cli`, ...). Includes everything the agent sends up front. Resumed or forked sessions and logs without a start are marked. With `--probe` the median is split into today's configuration as Claude Code's `/context` reports it in an empty directory, the rest of the smallest start, and what varies per session; it is a lever when the configuration is at least half of the median. |
| `big-context` | Hint | Share of tokens from requests above 500k (Claude Code) or above half of the logged context window (Codex), shown with the cache share. Large contexts are cheap while the cache is warm. |
| `compact-thrash` | Hint | Automatic compactions that follow each other with little new context in between. Needs the size after compaction; not measurable for Codex. |
| `cache-rebuild` | Observation | Large cache writes after a pause. The lifetime of the new cache does not prove the lifetime of the old one, so the cause stays unknown without earlier cache events. Expected rebuilds (model switch, compaction) are listed apart, and sub-agent files apart from the main conversation, since they write with lifetimes of their own. Where the log splits writes by lifetime, the main conversation's "5 min likely expired" rebuilds are replayed with a 1 h lifetime at list-price ratios (read 0.1×, write 1.25× / 2×): a lever only if the saving exceeds the extra cost of longer-lived writes. |
| `long-session` | Info | Sessions that span more than 24 hours. Wall-clock time says little about cost; a long pause costs nothing. |
| `config-size` | Hint | Size and count of instruction files, skills, hooks and MCP servers, only when you ask for the configuration check. |

## Transitions

`--transitions` is a timeline, not a rule: it states what the logs show around starts, resumes
and forks, compactions and pauses (one hour or more), per log file. It adds to `compact-thrash`
without recounting: its compactions are the events the summary counts, and `compact-thrash`
keeps judging how close together they are. Missing sizes after a compaction and unknown triggers
stay unknown (always for Codex). A new chat, a pause or a log that ends carries no verdict; a
parent is named only where the log names it. With `--tool-calls`, the first 30 calls after a
compaction are compared with what the same log did before: re-read areas (file and line range,
not edited since) and repeated identical failed calls are counted, with the compaction as
evidence. That count is not an avoidable amount.

## Comparison

`--compare <Nd>` is a view, not a rule. It runs the existing analysis over two windows (the last
N days and the N days before) and shows both values, the change and the denominators, including
`baseline-load` and `cache-rebuild` as each window measures them; where a rule cannot measure a
window the figure is unknown, not zero. A window below 3 active days or 100 requests (uncalibrated)
gets no change. A shift of 10 percentage points or more in the share of a model or front end
(`entrypoint`) between the windows is named as a limitation. A change is a correlation with
whatever else changed in that time; it says nothing about cause or about the quality of the work.

## Subagents

`--subagents` is a report, not a rule. It splits the requests of the period three ways that add
up to the summary's: direct requests of sessions, requests of agents whose named parents lead to
a session (nested agents count once, under their root), and requests of agents without a named
parent. Parents come from the logs only; project and time never link. Overlap is computed from
runs of requests less than an hour apart and says nothing about run time (the logs hold none) or
about conflicting work. A shared read or write (with `--tool-calls`, not for Codex) is a reason to
check. The report states evidence (log file per agent) and counter-examples (agents that read
other lines of a file, time-only overlaps, expected overlap of a session and its own agent); it
makes no claim that delegation is inefficient or that parallel work saves time.

## Compaction causes

`--compaction-causes` is a grouping, not a rule: it counts the compactions the summary counts by
project, model and session and words patterns as hints with a lever, never as the cause. It
adds to `compact-thrash`, `long-session` and `big-context` without recounting: those rules are
shown with their status, and the hints only count compactions in the same places (`see_rule`).
Three patterns: automatic compactions of one model that began in a narrow band of sizes (lever:
the model's window or a setting), half of the compactions or more in sessions of more than 24
hours, and half or more in one project when others exist (lever: the way of working). Thresholds
are uncalibrated. Unknown triggers and sizes stay unknown (always for Codex). With
`--compaction-settings` today's `autoCompactWindow` (per model under `modelSettings`, Claude Code
2.1.288 or later; `autoCompactEnabled`) is compared with the median size before automatic
compactions; the setting is today's content of the files and is never claimed for the past.

## Before and after a change

`--changed-on <date>` is the same view as `--compare` with the windows set by the user: the period
from the given day to now, and an equally long period before that day. The day is never guessed
and the report does not know what changed (`--change-label` only echoes the user's name). All
figures, the reliability threshold and the model and front-end limitations are those of
`--compare`. A difference between the periods is not attributed to the change: other tasks,
models or front ends, and short periods, move the numbers too, and less or more input is not
better or worse work. Today's `--probe` values are not part of it and are never read as an earlier
configuration; nothing is stored, so no earlier configuration can be recovered from this view.

## Captures

`--capture <name> --since <period>` stores the `--compare` figures of one source and period in a
local file, so a later period can be compared without the old logs; `--compare-captures` runs the
same comparison on two files and adds a limitation when their periods overlap. Captures made with
other rule versions, for another source or for another project are not compared; the reason is
stated and no figure is. A missing baseline is an error. `--probe` values are never kept or
assigned to an earlier capture. Format and location: `reference/json.md`.

## Tool-call rules

Only with `--tool-calls`. Each observation carries the period, the sample, counter-examples that
look like the pattern but are not counted, evidence (log file and call id, never text), what the
rule cannot know and one step to check. Limits are uncalibrated. Output is bytes; request tokens
are not attributed to a tool.

| Id | Measures |
| --- | --- |
| `repeat-read` | Reads of the same file and line range as an earlier read in the same log file, from 5 on, with no observed edit of that file and no compaction between. An edit made by a command is not observed, so a re-read can still have been justified. |
| `repeat-failure` | A failed call repeated unchanged (same tool and parameters, commands compared as hashes), from 3 on, with no observed edit anywhere in the session between the attempts. |
| `large-output` | Single tool results of 100,000 bytes or more, by tool, with their share of all returned bytes. |

Claude Code logs the file, the command and whether a call failed; Codex logs scripts, so there only
`large-output` is measured and the other two are "not measurable".

## Instruction-file rules

Only with `--instructions <dir>`, from today's files rather than the logs. Each finding names
the file, the measure, its certainty and one step to check.

| Id | Certainty | Measures |
| --- | --- | --- |
| `broken-import` | medium | An `@path` import in a loaded Claude file whose target does not exist. A token that looks like a path but is no import is a false positive. |
| `duplicate-instructions` | high | Files with byte-identical content that both load at start. |
| `base-size` | low | Estimated tokens (bytes ÷ 4) of the files expected at start, from 10,000. Large is not wrong; the threshold is uncalibrated. |
| `load-limit` | high | Codex only: project files exceed `project_doc_max_bytes`, so the end is not loaded. |

A file's state is `expected` by the loading rules, `confirmed` when `--probe` lists it. Rules
followed: Claude Code memory documentation and Codex `AGENTS.md` discovery (rust-v0.165), both as
of 2026-11; the two agents share no heuristic. Not evaluated: Codex project trust, contradictions
between files.

## Skill rule

Only with `--skills`, Claude Code only. `unused-skills` compares the `Skill` tool calls of the
period with the skills installed on disk (user `~/.claude/skills`, plugin and project
`.claude/skills` of the sessions' working directories) and totals the session-start list entries
of the listed skills that were not called. Such a skill is "not observed in the period", never
"never used": the log shows no skill the user starts by hand with `/name`, and a rarely used
skill can still matter. The result lists skills installed and called, the largest unused entries
and the next step to check (disable a plugin, shorten a description, keep a skill in the project
that needs it); nothing is changed. Codex logs no skill calls, so there it is "not measurable".

The size of an entry is bytes of the name plus `description` and `when_to_use`, cut at the
documented 1,536 characters; tokens are bytes ÷ 4 and an estimate. Claude Code does not document
the exact format of the list, so neither number is a count. `--probe` adds Claude Code's own
`Skills` figure next to the scan; the two cover different sets and are not reconciled. Rules
followed: Claude Code skills documentation as of 2026-11, checked with Claude Code 2.1.316:
personal, project and plugin skills (`plugin:name`), the frontmatter `name` (the directory name
also calls the skill), the description in the list at session start, and
`disable-model-invocation: true` (no entry, so not counted as unused). Not covered: enterprise,
nested and added-directory skills, skills synced from claude.ai, built-in skills,
`skillOverrides` and plugin enablement in project settings; a skill called but not found is
listed apart.

More rules are planned; see the project plan before relying on a rule that is not listed here.

## How the numbers are derived

- **Context of a request** is the sum of fresh input, cache reads and cache writes.
- **Sub-agent requests** are counted separately from the main session.
- **Today's configuration** does not explain past usage. The configuration check and historic
  numbers are reported separately.
