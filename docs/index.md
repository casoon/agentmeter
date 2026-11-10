---
title: Overview
description: What agentmeter reads, what it reports and where it stops.
order: 0
---

agentmeter reads the session logs that coding agents already write to your disk and explains
your usage: numbers from your own logs, each with the evidence behind it and a concrete next
step to check. It targets **Claude Code** and **Codex CLI**.

**Status:** early development (0.x), released on crates.io and as signed binaries on the GitHub
releases. Output, JSON shape and rule definitions may change between minor versions; the
changelog lists every change.

## What it reports

- A summary of requests, input, cache share, output and compactions over a period.
- Sessions and projects that account for most of the input.
- [Findings](reference/findings/): observations and hints from rules over the same data, such as
  large contexts, cache rebuilds and the size of the first request. Each states how sure it is.
- A data-quality section: observed period, days without requests inside it, files without usage,
  unreadable entries, unknown models.

## Analyses you ask for

Each of these is a flag of its own; without it nothing of it is read or computed.

- `--compare <Nd>`, `--changed-on <date>` and captures (`--capture`, `--compare-captures`): two
  periods side by side with both values, the change and the denominators. Changes, not causes.
- `--transitions`: a timeline of session starts, resumes, compactions and pauses.
- `--subagents`: usage of a session's own requests next to that of the agents it delegated to.
- `--compaction-causes` and `--compaction-settings`: compactions by project, model and session,
  next to today's compaction settings.
- `--retention`: the age of the oldest request next to Claude Code's log retention.
- `--tool-calls`: repeated reads of the same file area, identical failed calls, very large outputs.
- `--instructions <dir>`: the instruction files that apply to a directory, with size and load state.
- `--skills`: installed Claude Code skills that were called, and those that were not observed.
- `--ai-report`: the findings condensed to one line of JSON for an AI to evaluate.

The fields of each are in the [JSON reference](reference/json/).

## Privacy

By default agentmeter extracts only usage fields: token counts, timestamps, model, session and
project ids and compaction events. `--tool-calls` and `--skills` also open message content, and
keep only call ids, tool and skill names, file paths, a fingerprint of commands and the size of
results. `--instructions`, `--skills`, `--compaction-settings` and `--retention` read
configuration files for sizes and settings. No prompt, command, output or file text is kept or
printed. It does not touch the network and sends no telemetry; only `--probe` starts another
program, Claude Code itself.

## Where it stops

- It cannot show the remaining quota of a subscription, because that is not stored locally. A
  later budget mode can estimate the API-equivalent value of your usage.
- Log formats of both agents are not official. [Log formats](reference/log-formats/) lists what
  is read and where that is an assumption.
- Tokens are not comparable across vendors. Numbers are compared within one agent or in money.
