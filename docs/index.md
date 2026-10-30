---
title: Overview
description: What agentmeter reads, what it reports and where it stops.
order: 0
---

agentmeter reads the session logs that coding agents already write to your disk and explains
your usage: numbers from your own logs, each with the evidence behind it and a concrete next
step to check. It targets **Claude Code** and **Codex CLI**.

**Status:** early development. Nothing is released yet; readers for Claude Code and Codex work,
parts of this documentation (menu, configuration check, budget) describe planned behaviour.

## What it reports

- A summary of requests, input, cache share, output and compactions over a period.
- Sessions and projects that account for most of the input.
- [Findings](reference/findings/): observations and hints from rules over the same data, such as
  large contexts, cache rebuilds and the size of the first request. Each states how sure it is.
- A data-quality section: observed period, files without usage, unreadable entries, unknown models.

## Privacy

agentmeter only reads usage fields: token counts, timestamps, model, session and project ids and
compaction events. It does not read prompts or file contents, does not touch the network and
sends no telemetry.

## Where it stops

- It cannot show the remaining quota of a subscription, because that is not stored locally. A
  later budget mode can estimate the API-equivalent value of your usage.
- Log formats of both agents are not official. [Log formats](reference/log-formats/) lists what
  is read and where that is an assumption.
- Tokens are not comparable across vendors. Numbers are compared within one agent or in money.
