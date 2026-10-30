# agentmeter

Explains coding-agent usage. agentmeter reads the session logs that **Claude Code** already
writes to your disk, and the ones **Codex CLI** writes, and reports observations from your own
usage, each with the evidence behind it, how sure it is and one concrete thing to check next.

> **Status:** early development, no versioned release yet. Output, JSON shape and rule
> definitions may change.

<p align="center">
  <img src=".github/readme/demo.svg" alt="Terminal recording of agentmeter --tool claude --findings: one cache-rebuild finding with its measure, evidence, counter-arguments and next step, then the rules that found nothing, and the data quality. The logs are small synthetic sample data, not real usage." width="900">
</p>

<p align="center"><sub>Recorded output of <code>agentmeter --findings</code> on synthetic sample logs from <code>tests/fixtures</code>.</sub></p>

## Install

```sh
cargo install --git https://github.com/casoon/agentmeter agentmeter
```

Needs Rust 1.88 or newer. Run `agentmeter` in a terminal: it opens a menu with the summary,
sessions, projects, one row per rule and the data quality per source. `S` switches the source,
`P` the period, `/` searches.

```
agentmeter  Claude Code · Codex CLI · last 7 days

OVERVIEW
› Summary         claude 30,955 · codex 2,959 requests
  Sessions        claude 136 · codex 46
FINDINGS
  baseline-load   claude observed · codex observed
  cache-rebuild   claude observed · codex n/a
  compact-thrash  claude none · codex n/a
  …
```

(Shortened, figures from one author's logs. `none`: checked, nothing to report. `n/a`: the source cannot support the rule.)

## What it does

Existing tools add up tokens and cost. agentmeter looks at how usage came about:

- how large the first request of a session is, and how that changes over time
- how much of your input comes from very large contexts, next to the cache share
- large cache writes after pauses, with the cause marked *unknown* when the logs do not show it
- sessions that span many hours or days
- which sessions and projects account for the input

High usage does not prove waste, and usage logs do not show the quality of a result. Every rule
therefore states what it measured, its denominator and sample size, known counter-examples and
what it does *not* conclude. "No findings" and "not enough data" are reported separately, and the
report lists data gaps: observed period, files without usage, unreadable entries, unknown models
and entry types.

```
!  baseline-load   first request of a session: median 79k tokens
   system prompt, tools, instructions and memory; not broken down
!  big-context     27 % of tokens from requests above 500k, cache share 98 %
   cheap while cached; shown so you can judge the trade-off
?  cache-rebuild   343 of 1,243 pauses over 5 min followed by a write above 100k
```

(Shortened; figures from one author's logs. The full output is `agentmeter --findings`.)

## Usage

```sh
agentmeter                     # on a terminal: menu; otherwise the summary of every source with logs
agentmeter --summary           # totals, days and compactions
agentmeter --sessions          # sessions by input
agentmeter --projects          # projects by input
agentmeter --findings          # rules with measure, evidence and next step
agentmeter --tool codex        # claude, codex or all (default)
agentmeter --since 7d          # 7d, 30d or a date such as 2026-10-01
agentmeter --project <name>    # one project
agentmeter --json              # the same data as JSON, one entry per source (docs/reference/json.md)
agentmeter --check data-quality # exit 1 if a source has unreadable data or none to analyse
```

It reads `~/.claude/projects` (or `$CLAUDE_CONFIG_DIR`) and `~/.codex/sessions` plus
`archived_sessions` (or `$CODEX_HOME`), including compressed rollouts. Rules that a source cannot
support, for example cache rebuilds without cache-write data, are reported as "not measurable". Colour is used only on a terminal and
not when `NO_COLOR` is set.

## Privacy

Read-only, no network, no telemetry. The log files contain prompt and tool text, so they are
read in full, but agentmeter extracts only usage metadata (token counts, timestamps, model,
session and project ids, compaction events). Prompt and tool text is not analysed, stored or
printed; evidence points to a file and an event id, never to text.

## Limits

- Subscription quotas are not stored locally, so remaining quota cannot be shown.
- The log formats are not official and change between versions; unknown fields and entries are
  counted and reported, not guessed.
- Thresholds are uncalibrated. Hints are never errors.
- Tokens are not comparable across vendors.

## Documentation

The project page is built from [`site/`](site/) and [`docs/`](docs/) and published at
<https://casoon.github.io/agentmeter/>.

## Development

```sh
cargo test --workspace
```

Fixtures are synthetic, one per log-format epoch. A test against your own logs runs only with
`AGENTMETER_REAL_LOGS=1`.

## License

MIT — see [LICENSE](LICENSE).
