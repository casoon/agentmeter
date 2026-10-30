---
title: JSON output
description: The layout of `--json`, field by field, with the schema version.
order: 3
---

`agentmeter --json` prints one JSON object. Its layout is versioned: `schema_version` is `1` and
changes whenever a field is renamed, removed or changes meaning. New fields may be added without a
new version, so readers should ignore fields they do not know. Every change to the layout is an
entry in the changelog.

Times are ISO 8601 in UTC. Token counts are integers. Percentages are numbers from 0 to 100.
A field that does not apply is `null`, never left out. Only the sections selected by flags
(`--summary`, `--sessions`, `--projects`, `--findings`) are present; `data_quality` always is.

Each heading below is the path of an object in the output (`[]` is one element of an array); the
list under it names every field of that object. The tests compare these lists with the real output.

### `root`

- `schema_version` — layout version of this document.
- `sources` — one entry per source that has logs (`--tool` limits them); each is reported on its own.

### `sources[]`

- `tool` — `claude` or `codex`.
- `period` — the selected period in words, such as `last 7 days` or `all logs`.
- `project` — the `--project` filter, or `null`.
- `data_quality` — what was read and what could not be used.
- `summary` — totals; only with `--summary`.
- `sessions` — sessions by input; only with `--sessions`.
- `projects` — projects by input; only with `--projects`.
- `verdict` — only with `--findings`: `no_data` (nothing to analyse), `none` (every rule that could be measured found nothing) or `findings`.
- `findings` — only with `--findings`: the diagnoses of the rules with status `observed`.
- `rules` — only with `--findings`: the status of every rule, once each.
- `probe` — only with `--probe` and only for `claude`: what Claude Code's `/context` reported.

### `sources[].data_quality`

- `observed_from` — time of the first request in the logs, or `null`.
- `observed_to` — time of the last request in the logs, or `null`.
- `requests` — requests after merging duplicates, over the whole logs (not limited by `--since`).
- `analysable` — `false` when there is no usage at all; then "no findings" would be misleading.
- `files` — log files read.
- `lines` — non-empty lines read.
- `skipped_lines` — lines that were not valid JSON or lacked a field a usage line needs.
- `files_without_usage` — files that held no usage.
- `unknown_types` — entry types the reader does not know, with line counts.
- `unknown_usage_fields` — usage fields the reader does not know, with line counts.
- `duplicates_merged` — repeated lines merged into one request.
- `replays_dropped` — sub-agent lines that replay a parent and were dropped.
- `requests_with_ttl_split` — usage lines with cache writes split by lifetime (Claude Code).
- `requests_flat_cache` — usage lines with only the flat cache-write total (Claude Code).
- `requests_without_model` — requests without a model name.
- `compactions_without_post_tokens` — compactions without the size after compaction.
- `compressed_files_read` — compressed log files (`.zst`) that were read.
- `compressed_files_unreadable` — compressed files that could not be decoded.
- `compressed_files_shadowed` — compressed files skipped because a plain copy exists.
- `cache_write_measured` — whether the source records cache writes; if not, rules that need them are not measurable.
- `usage_delta_mismatches` — Codex: events whose change of the running total differs from the event's own value.
- `placeholder_version_entries` — entries with the placeholder version `0.0.0`.
- `first_version` — lowest agent version seen, or `null`.
- `last_version` — highest agent version seen, or `null`.

### `sources[].summary`

- `requests` — requests in the period.
- `sub_agent_requests` — of these, written by sub-agents.
- `sessions` — sessions with requests in the period.
- `projects` — projects with requests in the period.
- `input` — `fresh + cache_read + cache_write_5m + cache_write_1h`.
- `fresh` — input that was neither read from nor written to the cache.
- `cache_read` — input read from the cache.
- `cache_write_5m` — cache writes with the 5-minute lifetime (Codex: all writes, its lifetime is not logged).
- `cache_write_1h` — cache writes with the 1-hour lifetime.
- `output` — output tokens (Codex: including reasoning).
- `cache_read_pct` — `cache_read` as a share of `input`.
- `compactions` — compactions in the period.
- `days` — one entry per UTC day.
- `models` — requests per model.

### `sources[].summary.compactions`

- `total` — all compactions.
- `auto` — started by the agent.
- `manual` — started by the user.
- `trigger_unknown` — the log does not say (Codex).
- `avg_tokens_before` — average context before a compaction.
- `min_tokens_before` — smallest context before a compaction.
- `max_tokens_before` — largest context before a compaction.

### `sources[].summary.days[]`

- `date` — UTC day, `YYYY-MM-DD`.
- `requests` — requests that day.
- `input` — input that day.
- `cache_read_pct` — cache read share that day.
- `compactions` — compactions that day.

### `sources[].summary.models[]`

- `model` — model name as logged.
- `requests` — requests with that model.

### `sources[].sessions[]`

- `session` — session id.
- `project` — project name.
- `requests` — requests, sub-agents included.
- `sub_agent_requests` — of these, written by sub-agents.
- `first` — time of the first request.
- `last` — time of the last request.
- `hours` — hours between first and last request.
- `input` — input of the session.
- `peak_context` — largest context of a main-conversation request.
- `cache_read_pct` — cache read share.
- `compactions` — compactions in the session.

### `sources[].projects[]`

- `project` — project name.
- `sessions` — sessions.
- `requests` — requests.
- `input` — input.
- `input_share_pct` — share of the input of all projects in the selection.
- `cache_read_pct` — cache read share.

### `sources[].findings[]`

- `rule` — rule id, such as `big-context`.
- `rule_version` — version of the rule's definition.
- `level` — `notice` or `info`; `finding` is reserved for a verifiable malfunction and no rule emits it yet.
- `certainty` — `observed`, `likely` or `suspected`.
- `message` — the observation in one line.
- `measure` — the number behind it.
- `evidence` — where to look.
- `counterexamples` — known cases in which the observation is harmless.
- `next_step` — what to check next.
- `conclusion` — what the numbers mean.
- `parts` — the measure split into its parts; may be empty.
- `values` — more numbers, with keys specific to the rule.

### `sources[].findings[].conclusion`

- `kind` — `lever` (a setting or habit you control moves the number; weigh it), `look` (the log cannot decide; the evidence needs a look) or `fine` (nothing to do).
- `text` — the conclusion in one or two sentences.

### `sources[].findings[].parts[]`

- `label` — what the part is.
- `value` — its size.
- `unit` — unit of `value`: `tokens` or `hours`.
- `count` — how many events the part stands for, or `null`.
- `count_unit` — what `count` counts (`rebuilds`, `requests`, `starts`), or `null`.

### `sources[].findings[].measure`

- `name` — what was measured.
- `value` — the measured value.
- `unit` — unit of `value`.
- `denominator` — what it was measured against.
- `denominator_unit` — unit of `denominator`.
- `sample` — number of independent units behind it.
- `sample_unit` — unit of `sample`.

### `sources[].findings[].evidence[]`

- `file` — log file, relative to the projects directory (Claude Code) or the Codex home.
- `event` — event id in that file (message id, compaction id or `ordinal:<line number>`); never message text.

### `sources[].rules[]`

- `rule` — rule id.
- `rule_version` — version of the rule's definition.
- `status` — `observed` (a diagnosis is in `findings`), `checked_none` (measured, below the threshold) or `not_measurable`.
- `reason` — why it is not measurable; otherwise `null`.
- `message` — what was measured, for `observed` and `checked_none`; otherwise `null`.
- `measure` — the measure behind `message`, same fields as `findings[].measure`; otherwise `null`.

### `sources[].probe`

- `command` — the command that was run in each place.
- `runs` — one entry per place that answered.
- `failures` — one entry per place that did not, each with `place` and `reason` (`claude` not found, no answer within 90 s, an error answer, or a format the reader does not know).

### `sources[].probe.runs[]`

- `place` — `empty directory` (the configuration alone) or a project name.
- `dir` — the directory it ran in.
- `model` — the model `/context` names, or `null`.
- `total` — tokens sent at session start: the sum of `sent`. An estimate by Claude Code, not a count from the log.
- `sent` — categories sent with every request (system prompt, tools, memory files, skills, ...).
- `deferred` — categories listed by name only and loaded on demand; not in `total`.
- `memory_files` — memory files with their paths.
- `mcp_servers` — MCP tool tokens summed per server.

### `sources[].probe.runs[].sent[]`

The entries of `deferred`, `memory_files` and `mcp_servers` have the same fields.

- `label` — category, file path or server name.
- `tokens` — tokens as `/context` estimates them.

## `--check data-quality`

`agentmeter --check data-quality --json` prints a different, smaller object, also with
`schema_version`:

- `schema_version` — layout version.
- `check` — `data-quality`.
- `passed` — `true` if every source passed.
- `sources` — per source: `tool`, `passed` and `reasons` (empty when it passed).

The exit status is `0` if all sources passed, `1` if one failed (unreadable lines or compressed
files over the limits, or no usage in the selected period) and `2` for a bad call. Notices and info
from the rules never fail the check.
