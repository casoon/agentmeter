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
- `transitions` — only with `--transitions`: starts, resumes and forks, compactions and pauses with the context around them.
- `compare` — only with `--compare`: two periods side by side with change and denominators.
- `tool_calls` — only with `--tool-calls`: repeated reads, repeated failures and large outputs, from tool calls and results.
- `instructions` — only with `--instructions <dir>`: the instruction files that apply to that directory for this agent.

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

### `sources[].transitions`

One timeline per log file, from metadata only. A start, a resume or fork, a pause or the end of
a log is a marker, never proof of a crash, an abort or lost context, and sessions are linked
only by a parent the log names, never by closeness in time. Consumption after a transition is
measured, not proven avoidable. The compactions are the events the summary counts.

- `method` — how the timeline was built.
- `tool_data` — `true` when `--tool-calls` was given too, so compactions carry `repeats`.
- `counts` — the transitions by kind.
- `projects` — one entry per project.
- `transitions` — every transition in the period, each with its evidence.
- `notes` — limits of the view.

### `sources[].transitions.counts`

- `starts` — sessions whose log starts at a conversation root, at the first request of the file.
- `resumed_or_forked` — files that begin with history copied from another session.
- `incomplete_log` — files whose first message has a parent that is not in the file.
- `compactions` — compactions in the period.
- `pauses` — gaps of one hour or more between two requests of one log file.

### `sources[].transitions.projects[]`

- `project` — project name.
- `counts` — the same fields as `transitions.counts`, for this project.
- `median_context_before_compaction` — as the log states it, or `null`.
- `windows_checked` — compactions whose following calls could be checked (needs `--tool-calls`).
- `compactions_with_rereads` — of those, compactions followed by at least one re-read area.
- `rereads_after_compaction` — areas read again after those compactions, summed.
- `hint` — worded from the pattern when at least two compactions were followed by at least five re-read areas; otherwise `null`.

### `sources[].transitions.transitions[]`

- `kind` — `start`, `resumed_or_forked`, `incomplete_log`, `compaction` or `pause`.
- `project` — project name.
- `session` — session id.
- `time` — the first request (starts), the request before (pause) or the compaction.
- `file` — the log file; with `event` the evidence.
- `event` — the compaction's event id, otherwise `null`.
- `context_before` — compaction: tokens before, as the log states it; pause: context of the request before; otherwise `null`.
- `context_after` — compaction: tokens after as the log states it, `null` when it does not (always for Codex); never zero as a guess.
- `next_request_context` — context of the first request after the event (starts: the first request).
- `ms_to_next_request` — milliseconds to the next request, or `null`.
- `requests_to_next_compaction` — compaction only: requests until the next compaction or the end of the log.
- `then_log_ends` — compaction only: no later compaction in this file, so the count runs to the end of the log.
- `parent` — resume or fork only: `id` of the session the log names and `in_logs` (that session's log was read); otherwise `null`.
- `repeats` — compaction only, with `--tool-calls`: `window` (in words), `calls_checked`, `reads`, `reread_areas` (reads of a file and line range read before the compaction and not edited since), `failures_repeated` (identical failed calls repeated) and `examples` (up to three files); otherwise `null`. A count of what happened, not proof the re-read was avoidable.

### `sources[].compare`

The last N days (`later`) against the N days before them (`earlier`), from the same logs and the
same rules. A change is shown, never explained: it is no verdict on the quality of the work and no
proof of a cause.

- `verdict` — `compared`, `not_reliable` (a window has too few active days or requests: values, no change) or `no_data` (a window holds no request).
- `method` — how the figures are made, in words.
- `earlier` — the window before.
- `later` — the window up to now.
- `metrics` — the figures, in a fixed order.
- `limitations` — what limits the comparison.

### `sources[].compare.earlier`

- `from` — start of the window, included.
- `to` — end of the window, excluded.
- `days` — UTC calendar days the window touches; the first and last may be partial.
- `active_days` — days with at least one request.
- `days_without_logs` — `days` minus `active_days`.
- `requests` — requests in the window.
- `input` — input tokens in the window.
- `compactions` — compactions in the window.
- `reliable` — at least 3 active days and 100 requests (uncalibrated).
- `models` — requests per model, most first: `label`, `count`, `share_pct`.
- `entrypoints` — main-conversation sessions per front end, most first: `label`, `count`, `share_pct`; `(not recorded)` where the log has none.

### `sources[].compare.later`

- `from` — start of the window, included.
- `to` — end of the window, excluded.
- `days` — UTC calendar days the window touches; the first and last may be partial.
- `active_days` — days with at least one request.
- `days_without_logs` — `days` minus `active_days`.
- `requests` — requests in the window.
- `input` — input tokens in the window.
- `compactions` — compactions in the window.
- `reliable` — at least 3 active days and 100 requests (uncalibrated).
- `models` — as in `earlier`.
- `entrypoints` — as in `earlier`.

### `sources[].compare.metrics[]`

- `name` — `active_days`, `requests_per_active_day`, `input_per_request`, `cache_read_pct`, `compactions_per_1000_requests`, `median_start_context` (`baseline-load` of the window), `sub_agent_pct` or `cache_rebuilds` (`cache-rebuild` of the window).
- `unit` — unit of the values; `%` for shares.
- `earlier` — value in the earlier window, or `null` when it cannot be computed (zero denominator, rule not measurable); never zero as a guess.
- `later` — value in the later window, or `null`.
- `change` — `later` minus `earlier`, in percentage points for `%`; `null` unless both values exist and both windows are reliable.
- `change_pct` — relative change in percent; `null` as `change`, and when `earlier` is zero.
- `denominator_earlier` — what the earlier value was divided by or sampled from.
- `denominator_later` — the same for the later value.
- `denominator_unit` — what the denominators count.
- `note` — why a value is missing where a rule could not measure it, otherwise `null`.

### `sources[].compare.limitations[]`

- `kind` — `empty-window`, `window-not-reliable`, `model-mix` or `entrypoint-mix` (a model's or front end's share moved by 10 percentage points or more).
- `text` — the limitation in words.

### `sources[].tool_calls`

Opt-in: message content is opened for call ids, tool names, file paths of file tools, a hash of
what makes other calls identical (a command is never kept) and the byte size of results. No
command text and no output text is stored or printed. Limits are uncalibrated.

- `method` — how calls and results were read.
- `calls` — calls in the selected period.
- `stats` — what reading saw: `calls`, `calls_without_result`, `results_without_call`, `duplicates_merged` (streaming copies and sub-agent replays of one call) and `tools` (calls per tool name).
- `rules` — one entry per rule, once each.

### `sources[].tool_calls.stats`

- `calls` — calls in the period.
- `calls_without_result` — calls whose result is not in the log; status and size stay unknown, never success or zero.
- `results_without_call` — results whose call is not in the logs.
- `duplicates_merged` — copies of one call id that were merged.
- `tools` — calls per tool name.

### `sources[].tool_calls.rules[]`

- `id` — `repeat-read`, `repeat-failure` or `large-output`.
- `version` — version of the rule.
- `status` — `observed`, `checked_none` or `not_measurable`.
- `reason` — why it is not measurable; otherwise `null`.
- `message` — what was measured; otherwise `null`.
- `count` — calls that match; `of` is the number of calls the rule looked at.
- `of` — the denominator of `count`.
- `from` — time of the first call in the period; `to` the last.
- `to` — time of the last call in the period.
- `threshold` — the limit, in words; `calibrated` is `false` for every rule.
- `calibrated` — whether the limit is calibrated; currently always `false`.
- `examples` — up to five entries `label` (a file path, or a tool name with a parameter fingerprint such as `Bash #a1b2c3d4`), `count` and `bytes` (output bytes of those calls, or `null`).
- `counterexamples` — what looks like the pattern but is not counted, with numbers (re-read after an observed edit or a compaction, other line ranges).
- `uncertainty` — what the rule cannot know, such as edits made by commands.
- `evidence` — up to five entries `file` (the log file) and `event` (the call id).
- `next_step` — one concrete step to check; otherwise `null`.

Rules: `repeat-read` counts a read of the same file and line range as an earlier one in the same
log file with no observed edit of that file and no compaction between; `repeat-failure` counts an
identical failed call repeated with no observed edit in the session between; `large-output`
counts single results of 100,000 bytes or more. Codex logs its tool arguments as scripts and
no success or failure, so there only `large-output` is measurable. Bytes are not tokens; the
tokens of a request are not attributed to one tool.

### `sources[].instructions`

Today's files, not the history. Sizes are bytes; tokens are estimated as bytes ÷ 4, never counted.

- `tool` — `claude` or `codex`; each agent follows its own loading rules.
- `rules_version` — the loading rules and the documentation they come from.
- `method` — how sizes were measured.
- `dir` — the directory that was scanned.
- `files` — every instruction file found, including those that do not load.
- `loaded_est_tokens` — estimated tokens of the files expected (or confirmed) at session start. Files that are on demand, not loaded, unreadable or missing are not in it.
- `findings` — rule findings; empty when there are none.
- `notes` — limits of the scan.

### `sources[].instructions.files[]`

- `path` — absolute path.
- `scope` — `managed`, `user`, `ancestor` (a directory above), `project` (the directory itself), `rules`, `nested` (below the directory) or `import`.
- `state` — `confirmed` (`--probe` listed it), `expected` (the loading rules say it loads at start; not observed), `on_demand`, `not_loaded` (exists, but excluded, shadowed or beyond a limit), `unreadable` or `missing` (an import that is not there).
- `bytes` — size on disk; 0 when unreadable or missing.
- `est_tokens` — `bytes` as estimated tokens.
- `loaded_bytes` — bytes expected in the context at start; 0 unless `confirmed` or `expected`, less than `bytes` when a limit cuts the file.
- `via` — the file that imports this one, or `null`.
- `note` — why the state, or `null`.

### `sources[].instructions.findings[]`

- `rule` — `broken-import`, `duplicate-instructions`, `base-size` or `load-limit` (Codex).
- `file` — the file (or directory) it is about.
- `measure` — what was measured.
- `certainty` — `high`, `medium` or `low`.
- `check` — one concrete step to check it.

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
