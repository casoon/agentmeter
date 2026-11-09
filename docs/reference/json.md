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
- `subagents` — only with `--subagents`: direct and delegated usage per session, overlapping runs and shared files.
- `compaction_causes` — only with `--compaction-causes` or `--compaction-settings`: compactions by project, model and session with hints at the lever, and with the second flag today's auto-compact setting.
- `before_after` — only with `--changed-on`: the period before a change date next to the period since it.
- `tool_calls` — only with `--tool-calls`: repeated reads, repeated failures and large outputs, from tool calls and results.
- `instructions` — only with `--instructions <dir>`: the instruction files that apply to that directory for this agent.
- `skills` — only with `--skills`: installed skills called and not observed in the period, with the size of their entries in the session-start skill list.

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

### `sources[].subagents`

Opt-in with `--subagents`. Requests are split into the direct ones of main sessions and those of
delegated agents. An agent belongs to a session only where the log names its parent (Claude Code:
the session folder of the agent's log; Codex: the spawn record, the fork id, or the parent
history copied into the rollout); a project name or closeness in time never makes a delegation,
and an agent without a named parent stays unassigned. Replays of parent messages and duplicate
usage events are dropped as in the summary. With `--tool-calls` the file accesses of the streams
are compared; without them, or where the log has no file targets (Codex), those fields are `null`
and `notes` says so.

- `method` — how the report was built.
- `file_data` — `true` when file targets of tool calls were read and the log records them.
- `counts` — sessions, agents and overlapping pairs.
- `totals` — the requests of the period counted once, split three ways.
- `roots` — the sessions that delegated, with input per direct and delegated use, busiest first.
- `unassigned` — agents without a usable parent chain, with the fields of `roots[].children`.
- `overlaps` — pairs of streams whose runs overlap in time, at most 20, those with the most shared files first.
- `counterexamples` — what looks like the pattern but is not one, with numbers.
- `notes` — limits of the view.

### `sources[].subagents.counts`

- `sessions` — main sessions with requests in the period.
- `sessions_with_agents` — of those (or parents whose own log was not read), sessions with at least one assigned agent.
- `agents` — agent streams with requests in the period, assigned or not.
- `nested_agents` — assigned agents at depth 2 or more.
- `max_depth` — the longest chain of named parents from a session, `0` without agents.
- `unassigned_agents` — agents without a usable parent chain.
- `overlaps_delegation` — overlapping pairs where one is an ancestor of the other by named parents.
- `overlaps_same_task` — overlapping agents of one session that are not ancestor and descendant.
- `overlaps_time_only` — overlapping pairs the logs do not relate in any way.

### `sources[].subagents.totals`

`direct + delegated + unassigned` is `requests`, and `requests` and `input` are the summary's for
the same period. Inclusive sums (`roots[].inclusive_input`) are derived and never added to these.

- `requests` — requests in the period.
- `input` — their input: fresh, cache read and cache write.
- `direct` — usage of main sessions.
- `delegated` — usage of agents whose parent chain leads to a session.
- `unassigned` — usage of agents whose parent the logs do not name.

### `sources[].subagents.roots[]`

- `session` — session id the agents name as their root; a parent whose own log was not read still appears.
- `project` — project name.
- `in_logs` — `true` when the session's own requests were read.
- `direct` — the session's own usage.
- `delegated` — usage of all its agents, nested ones once.
- `inclusive_input` — `direct.input + delegated.input`, derived for reading.
- `agents` — assigned agents.
- `max_depth` — longest chain among them.
- `first` — first request of session or agents.
- `last` — last request of session or agents.
- `span_ms` — last minus first request.
- `active_ms` — the lengths of the runs added up. The logs hold no run time: a run is a stretch of requests less than an hour apart, its length is first to last request and still contains waiting, and a run of one request has no observed length.
- `paused_ms` — gaps of an hour or more between runs; `active_ms + paused_ms = span_ms`.
- `runs` — number of runs.
- `shared_reads` — areas (file and line range) read by two or more streams of the session; `null` when not measurable.
- `shared_reads_other_ranges` — files read by two or more streams but never the same lines (counter-example); `null` when not measurable.
- `shared_writes` — files written by two or more streams; a reason to check, not a conflict; `null` when not measurable.
- `shared_examples` — up to three file paths among them.
- `start_context` — first requests of the agents (`agents`, `context`, `fresh`, `cache_read`, `cache_write`), only with two or more agents and recorded cache writes, otherwise `null`. A measure of what was sent, not a verdict on delegating.
- `children` — the assigned agents, largest input first.

### `sources[].subagents.roots[].direct`

The same fields describe `delegated`, `totals.direct`, `totals.delegated`, `totals.unassigned` and
`children[].usage`.

- `requests` — requests.
- `input` — `fresh + cache_read + cache_write`.
- `fresh` — input neither read from nor written to the cache.
- `cache_read` — input read from the cache.
- `cache_write` — input written to the cache.
- `output` — output tokens.
- `models` — requests per model name, `unknown` for requests without one.

### `sources[].subagents.roots[].children[]`

- `agent` — the agent's id as the log gives it; `null` when the stream has no agent log of its own.
- `parent` — the session or agent the log names as parent; `null` when it names none.
- `parent_basis` — where the log names it: `session_folder` (Claude Code), `spawn_record`, `fork_record` or `copied_history` (Codex; the weakest, taken from parent history copied into the rollout); `null` with the parent.
- `depth` — steps from the root session along named parents, 1 for a direct child; `null` when the chain does not reach a session.
- `project` — project name.
- `file` — the agent's log file, the evidence.
- `first` — its first request.
- `last` — its last request.
- `usage` — its usage, with the fields of `roots[].direct`.

### `sources[].subagents.overlaps[]`

Overlap is between runs of requests and says nothing about conflicting work. `relation` tells a
delegation the logs name from two streams that merely ran at the same time.

- `first` — session or agent id; the log file when the agent has no id.
- `first_kind` — `session` or `agent`.
- `second` — as `first`.
- `second_kind` — `session` or `agent`.
- `relation` — `delegation`, `same_task` or `time_only`.
- `overlap_ms` — time both were in a run; `0` when one run is a single request inside the other.
- `shared_reads` — areas both read while both were in a run; `null` when not measurable.
- `shared_writes` — files both wrote while both were in a run; `null` when not measurable.
- `examples` — up to three of those files.
- `files` — the two log files the pair is based on.

### `sources[].compaction_causes`

The compactions the summary counts, grouped; nothing is recounted from another source. A pattern
is worded as a hint with the lever it points to (`setting_or_window` or `working_style`), never
as the cause. A missing trigger or size stays unknown (always for Codex). Counts and rates use
the main conversation; a compaction event names no model, so each takes the model of the request
just before it.

- `method` — how the groups were built.
- `totals` — all compactions of the period, as a group.
- `projects` — one group per project with compactions, most compactions first; the fields of `totals`.
- `models` — one group per model, the same fields; `unknown` when no request names a model.
- `sessions` — sessions with at least one compaction.
- `observations` — patterns above the thresholds; empty when there is none.
- `related_rules` — `compact-thrash`, `big-context` and `long-session` as the findings report them, not recounted.
- `settings` — only with `--compaction-settings` for `claude`: today's settings files; otherwise `null`.
- `settings_comparison` — only with `--compaction-settings` for `claude`: today's window against the observed sizes; otherwise empty.
- `notes` — limits of the view.

### `sources[].compaction_causes.totals`

- `key` — `all` for the totals, else the project, model or session id.
- `compactions` — compactions in the period.
- `requests` — main-conversation requests of the group in the period.
- `per_1000_requests` — compactions per 1,000 of those requests, or `null` without requests.
- `triggers` — compactions by trigger.
- `tokens_before` — size before compaction, or `null` without compactions.
- `without_tokens_after` — compactions whose size after compaction the log does not state (always, for Codex); never counted as zero.

### `sources[].compaction_causes.totals.triggers`

- `auto` — started by the agent when the window filled up.
- `manual` — started by the user.
- `unknown` — the log does not say (always for Codex).

### `sources[].compaction_causes.totals.tokens_before`

- `min` — smallest size before compaction, as the log states it (Codex: the context of the last request before the event).
- `q1` — first quartile.
- `median` — median.
- `q3` — third quartile.
- `max` — largest size.

### `sources[].compaction_causes.sessions[]`

- `key` — session id.
- `compactions` — compactions of the session.
- `requests` — main-conversation requests of the session in the period.
- `per_1000_requests` — compactions per 1,000 requests, or `null`.
- `triggers` — as in `totals`.
- `tokens_before` — as in `totals`.
- `without_tokens_after` — as in `totals`.
- `project` — project name.
- `span_ms` — first to last request in the period, idle time included.
- `peak_context` — the largest context of a request of the session.

#### `sources[].compaction_causes.observations[]`

Not checked against the output by the generic field test; a test of its own does.

- `kind` — `size_band` (automatic compactions of one model began within a narrow band, as a fixed limit produces), `long_sessions` (half of the compactions or more fell in sessions of more than 24 hours) or `concentrated` (one project holds half of the compactions or more while others exist).
- `scope` — model name, project name or `all`.
- `message` — the observation in words.
- `compactions` — compactions the observation counts.
- `of` — compactions of the scope it is taken from.
- `lever` — `setting_or_window` or `working_style`.
- `certainty` — always `hint`: a pattern in the logs, not a cause.
- `see_rule` — the rule that already reports the same behaviour (`long-session`, `big-context`), else `null`; the observation counts compactions in it and is not a second finding.
- `check` — what to check and where an effect would show.

### `sources[].compaction_causes.related_rules[]`

- `rule` — `compact-thrash`, `big-context` or `long-session`.
- `status` — `observed`, `checked_none` or `not_measurable`, as in `rules`.
- `message` — what the rule measured, or `null`.
- `reason` — why it could not be measured, or `null`.

### `sources[].compaction_causes.settings`

Only the auto-compact keys of the Claude Code settings files, and only with
`--compaction-settings`. The files are read as they are now; the content is never attributed to
compactions of the past. Managed settings, `CLAUDE_CODE_AUTO_COMPACT_WINDOW` and `--autocompact`
are not visible here and take precedence over the files.

- `reference` — the documentation and versions the keys come from: `autoCompactWindow` (100000 to 1000000 tokens, any settings file), `modelSettings.<model>.autoCompactWindow` (a number or `"auto"`, Claude Code 2.1.288 or later; before it `/autocompact` saved the top-level key) and `autoCompactEnabled`.
- `as_of` — says that the files are read as they are now.
- `files` — every file looked at, with its state.
- `windows` — the window settings found.
- `auto_compact_enabled` — `autoCompactEnabled` values found.
- `projects_not_read` — projects whose working directory is unknown or gone, so their project and local settings were not looked at (unknown, not absent).

### `sources[].compaction_causes.settings.files[]`

User settings (`settings.json` of each configuration directory) and, per project, `.claude/settings.json` and `.claude/settings.local.json` in the working directory of its latest session.

- `path` — the file.
- `scope` — `user`, `project` or `local`.
- `project` — project name for `project` and `local`, else `null`.
- `state` — `read`, `missing`, `unreadable` or `invalid_json`.

### `sources[].compaction_causes.settings.windows[]`

- `file` — the file the entry is from.
- `scope` — `user`, `project` or `local`.
- `project` — project name for `project` and `local`, else `null`.
- `model` — the model of a `modelSettings` entry, `null` for every model.
- `tokens` — as written, `null` for `"auto"` (the window tuned for the model, not stated in the logs).
- `note` — set when the number lies outside 100,000 to 1,000,000, which Claude Code limits it to.

### `sources[].compaction_causes.settings.auto_compact_enabled[]`

- `file` — the file the entry is from.
- `scope` — `user`, `project` or `local`.
- `project` — project name for `project` and `local`, else `null`.
- `enabled` — `autoCompactEnabled` as written; `false` turns automatic compaction off.

### `sources[].compaction_causes.settings_comparison[]`

One entry per project and model with compactions. The window that applies today is the first of local, project and user settings; in a file the entry for the model comes before the general one. A hint, not proof.

- `project` — project name.
- `model` — model of the compactions (the request before each).
- `setting` — the window that applies, with the fields of `windows[]`, or `null` when none is set.
- `auto_compact_enabled` — `autoCompactEnabled` that applies, or `null`.
- `automatic_compactions` — automatic compactions of the pair with a known size.
- `median_tokens_before` — their median, or `null`.
- `max_tokens_before` — their largest, or `null`.
- `median_share_of_window_pct` — the median as a percent of the window, or `null`.
- `above_window` — automatic compactions larger than the window.
- `fit` — `within` (median 70 to 105 percent of the window), `below`, `above`, `no_setting`, `auto_value` or `not_measurable`; the limits are uncalibrated.
- `message` — the comparison in words, marked as today's setting.

### `sources[].before_after`

The period from the date the user gave up to now (`later`) against an equally long period ending
at that date (`earlier`), computed by the same function as `compare`. The date is the user's;
nothing is detected. A difference is shown, never attributed to the change.

- `change_point` — the given day, 00:00 UTC.
- `label` — the user's name for the change (`--change-label`), otherwise `null`.
- `period_days` — calendar days each period covers; `0` when the date is not in the past.
- `notice` — in words: the difference is no effect of the change.
- `comparison` — the comparison of the two periods.

### `sources[].before_after.comparison`

Same content as `sources[].compare`, including its objects `earlier`, `later`, `metrics[]` and
`limitations[]`.

- `verdict` — `compared`, `not_reliable` or `no_data`, as in `compare`.
- `method` — how the figures are made, in words.
- `earlier` — the period before the change date.
- `later` — the period from the change date to now.
- `metrics` — the figures, in a fixed order.
- `limitations` — what limits the comparison, among them model and front-end shifts and periods that are too short.

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

### `sources[].skills`

Opt-in. Message content is opened only for the skill name of `Skill` tool calls (arguments are
not kept); the installed skills are read from the headers of their `SKILL.md` files for the name
and the length of the description, never the content. A skill without a call in the period is
`not observed`, not `never used`: a skill started by hand with `/name` is not in the log in a form
this reads. Sizes are bytes of the listing entry (name plus `description` and `when_to_use`, cut
at 1,536 characters); tokens are estimated as bytes ÷ 4, never counted, and the two are not
equated. Codex logs no skill calls: its `status` is `not_measurable` and nothing is scanned.

- `method` — how calls and sizes were read.
- `rules_version` — the skill-loading rules the scan follows and the documentation they come from.
- `status` — `observed` (listed skills without a call), `checked_none` or `not_measurable`.
- `reason` — why it is not measurable; otherwise `null`.
- `message` — what was measured; otherwise `null`.
- `calls` — skill calls in the selected period.
- `installed` — skills the scan found.
- `listed` — of these, skills whose entry is in the list sent at session start.
- `user_only` — skills with `disable-model-invocation: true`: no list entry, never counted as unused.
- `unreadable` — skills whose `SKILL.md` could not be read.
- `used` — skills with at least one call in the period.
- `unused_listed` — listed skills without a call in the period.
- `listed_bytes` — bytes of all listed entries (estimate); `listed_est_tokens` is that as estimated tokens.
- `listed_est_tokens` — `listed_bytes` as estimated tokens.
- `unused_bytes` — bytes of the entries of `unused_listed` (estimate); `unused_est_tokens` is that as estimated tokens.
- `unused_est_tokens` — `unused_bytes` as estimated tokens.
- `largest_unused` — up to five entries of `unused_listed`, largest first, with the fields of `skills[]`.
- `skills` — every scanned skill, most called first.
- `called_not_scanned` — skills called in the period that are not among the scanned ones (built in, synced from claude.ai, nested or added directories, removed since).
- `probe` — with `--probe` and a run in an empty directory: that run's `Skills` figure next to the scan; otherwise `null`.
- `next_step` — what to check, for `observed`; otherwise `null`.
- `notes` — limits of the scan: missing or unreadable directories, name clashes, plugins left out, what is not covered.

### `sources[].skills.skills[]`

- `name` — the name a call uses; plugin skills as `plugin:skill`.
- `source` — `user`, `project` or `plugin`.
- `path` — absolute path of the `SKILL.md`.
- `state` — `listed`, `user_only` or `unreadable`.
- `calls` — calls in the selected period.
- `last_used` — time of the latest call in all logs read (not limited by `--since`), or `null`.
- `list_bytes` — bytes of the listing entry (estimate); 0 unless listed.
- `list_est_tokens` — `list_bytes` as estimated tokens.

### `sources[].skills.called_not_scanned[]`

- `name` — the skill name as called.
- `calls` — calls in the selected period.
- `last_used` — time of the latest call in all logs read, or `null`.

### `sources[].skills.probe`

- `place` — the probe run used (`empty directory`).
- `probe_tokens` — tokens of the category `Skills` in `/context`, as Claude Code estimates them (`< 20` style cells are read as their limit).
- `scanned_est_tokens` — estimated tokens of the listed user and plugin skills the scan found. The two cover different sets (the probe also counts built-in and claude.ai skills) and are shown side by side, not reconciled.

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

## Captures

`--capture <name> --since <period>` writes the figures of that period, up to now, of one source to
a local file; nothing is written unless you ask. The figures are the ones of `--compare`. A capture
holds aggregated numbers and labels (model and front-end names, project filter, your change
label), never conversation, source code, tool calls, session or message ids, or configuration
text. Values from `--probe` are not part of it.

- Location: `$AGENTMETER_DATA_DIR/captures`, else `$XDG_DATA_HOME/agentmeter/captures`, else `~/.local/share/agentmeter/captures`; one file `<name>.json` per capture. A name has letters, digits, `-`, `_` and `.`, up to 64, and does not start with a dot.
- `--capture` does not overwrite; `--delete-capture <name>` removes the file, deleting the file by hand works as well. `--captures` lists them, `--compare-captures <earlier> <later>` compares two.
- A file with another `schema_version` than this version reads is listed as unreadable and is not compared.

#### Capture file

Pretty-printed JSON, also what `--capture --json` prints.

- `schema_version` — layout version of the file, `1`.
- `name` — the name it is stored under.
- `label` — your name for a change (`--change-label`), otherwise `null`.
- `created` — when it was made (UTC).
- `tool` — `claude` or `codex`.
- `project` — the `--project` filter, otherwise `null`.
- `rules` — version of each rule whose figure is kept (`baseline-load`, `cache-rebuild`).
- `data_quality` — `files`, `skipped_lines`, `files_without_usage`, `compressed_files_unreadable` and `analysable` of the source when it was made.
- `facts` — the period as in `sources[].compare.earlier`.
- `figures` — per figure of `sources[].compare.metrics[]`: `name`, `value` (`null` where it could not be computed), `denominator` and `note`.

#### `--compare-captures --json`

An object with `schema_version` and `capture_comparison`:

- `comparable` — `false` when the captures are from different sources, projects or rule versions; no figure is given then.
- `reason` — why not, otherwise `null`.
- `earlier` and `later` — `name`, `label`, `created` of each capture.
- `comparison` — as `sources[].compare` (same rules, thresholds and limitations; a `windows-overlap` limitation is added when the periods overlap), or `null` when not comparable.

#### `--captures --json`

An object with `schema_version`, `directory`, `captures` (`name`, `label`, `created`, `tool`, `project`, `from`, `to`, `requests`) and `unreadable` (`name` and `reason`).
