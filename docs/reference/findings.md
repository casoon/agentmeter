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

More rules are planned; see the project plan before relying on a rule that is not listed here.

## How the numbers are derived

- **Context of a request** is the sum of fresh input, cache reads and cache writes.
- **Sub-agent requests** are counted separately from the main session.
- **Today's configuration** does not explain past usage. The configuration check and historic
  numbers are reported separately.
