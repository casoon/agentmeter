---
title: Log formats
description: Which log fields agentmeter reads for Claude Code and Codex CLI, and what is assumed.
order: 2
---

Both formats are not documented officially and change between versions. Parsers skip unknown
fields and lines and count what could not be read.

## Claude Code

Location: `~/.claude/projects/<project>/<session>.jsonl`, sub-agents in `subagents/`.

- Assistant entries carry `usage` with fresh input, output, cache reads and cache writes.
- The same response can appear more than once, so entries are deduplicated by message id and
  request id.
- Compactions are `system` entries with `compactMetadata` (trigger and tokens before).
- Entries marked as sidechain belong to a sub-agent.

## Codex CLI

Location: `~/.codex/sessions/` and `~/.codex/archived_sessions/`.

- `token_count` events carry cumulative totals and the last call, plus the model's context
  window.
- Input includes cached input; output includes reasoning. Checked on one author's rollouts
  (3,453 events, versions 0.130 to 0.165): `total_tokens` is always input plus output, and cached
  input is never larger than input.
- Repeated events with unchanged totals occur, so deltas of the cumulative total are used.
  Summing the last call instead would count them twice.
- Replayed parent history in sub-agent rollouts is not counted twice.
- Cache writes are read only where the field exists (newer versions) and carries a value above 0;
  otherwise they are "not measured", never zero.
- Compactions appear as `compacted` entries with a response id. The trigger and the size after
  compaction are not logged, so rules that need them report "not measurable".
- Older rollouts have no `token_count` events and are reported as files without usage.
- Compressed rollouts (`.jsonl.zst`) are read as a stream; damaged files are counted and reported.
- The version in `session_meta` is shown but not used to pick a format, because a resumed session
  keeps the original value.

## Alternative source

Both agents can export OpenTelemetry data, which is the only documented and stable interface.
Reading it is a possible later addition.
