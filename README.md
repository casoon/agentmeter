# agentmeter

Explains coding-agent usage. agentmeter reads the session logs that **Claude Code** already
writes to your disk, and the ones **Codex CLI** writes, and reports observations from your own
usage, each with the evidence behind it, how sure it is and one concrete thing to check next.

> **Status:** early development (0.x). Output, JSON shape and rule definitions may change
> between minor versions; the changelog lists every change.

<p align="center">
  <img src=".github/readme/demo.svg" alt="Terminal recording of agentmeter --tool claude --findings --probe: a status block of what was read, then a cache-rebuild finding marked as a lever with its breakdown bars and the 1 h estimate, a baseline-load finding that splits the median session start into configuration and the rest, and the rules that found nothing. The logs are small synthetic sample data, not real usage." width="900">
</p>

<p align="center"><sub>Recorded output of <code>agentmeter --findings --probe</code> on synthetic sample logs from <code>examples/demo-logs</code>.</sub></p>

## Install

```sh
cargo install agentmeter
```

Needs Rust 1.88 or newer. Prebuilt binaries for macOS (Apple silicon, Intel) and Linux (x86_64,
arm64, static) are on the [releases page](https://github.com/casoon/agentmeter/releases).

Official binaries come only from the GitHub releases of `casoon/agentmeter`, built there by the
release workflow, never on a developer machine. To verify a download by hand, check the hash and
the signed build provenance:

```sh
shasum -a 256 -c agentmeter-<version>-<target>.tar.gz.sha256
gh attestation verify agentmeter-<version>-<target>.tar.gz --repo casoon/agentmeter
```

The binaries are built with `cargo auditable`, so `cargo audit bin agentmeter` can check a
downloaded one for known vulnerabilities.

Run `agentmeter` in a terminal: it opens a menu with the summary,
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

Each finding opens with a conclusion: **lever** (a setting or habit you control moves the
number; weigh it), **look at it** (the logs cannot decide; the evidence needs a look) or **fine**.
Bars show what the number is made of, and a block at the top shows what was read, how well, and
which rules could be measured.

High usage does not prove waste, and usage logs do not show the quality of a result. Every rule
therefore states what it measured, its denominator and sample size, known counter-examples and
what it does *not* conclude. "No findings" and "not enough data" are reported separately, and the
report lists data gaps: observed period, files without usage, unreadable entries, unknown models
and entry types.

```
READ
  ✓ logs      1,299 files, 855,776 lines from ~/.claude/projects
  ✓ probe     /context in 4 of 4 places; answered locally, no model request

FINDINGS   ? 3 to look at   ✓ 1 fine   ✓ 1 nothing found

? cache-rebuild                                      look at it · notice · observed · v1
  ⇒ 202 rebuilds (60.1 M) are in sub-agent files, which wrote 100 % of their caches with
    5 min while the main conversation wrote 100 % with 1 h; …

    pause over 1 h                 █████████████████·······  42.4 M  147 rebuilds
    5 min expired, main session    ························       0  0 rebuilds
    5 min expired, sub-agents      ████████████████████████  60.1 M  202 rebuilds

? baseline-load                                      look at it · notice · observed · v1
  ⇒ today's configuration is 34k of the median start 79k (43 %); 34k more is in every
    start (front end, project instructions, a short first prompt) and 11k varies per
    session; 146 of 146 starts came from claude-desktop, …

    median start, split            ██████████████▓▓▓▓▓▓▓▓▓▓▓▓▓▓░░░░     79k
```

(Shortened; figures from one author's logs. The full output is `agentmeter --findings --probe`.)

## Usage

```sh
agentmeter                     # on a terminal: menu; otherwise the summary of every source with logs
agentmeter --summary           # totals, days and compactions
agentmeter --sessions          # sessions by input
agentmeter --projects          # projects by input
agentmeter --findings          # rules with conclusion, measure, evidence and next step
agentmeter --probe             # also ask Claude Code what a session start holds (see below)
agentmeter --compare 7d         # the last 7 days next to the 7 days before (see below)
agentmeter --changed-on 2026-10-01  # the period before a change date next to the one since (see below)
agentmeter --transitions        # starts, resumes, compactions, pauses as a timeline (see below)
agentmeter --tool-calls        # also read tool calls: repeated reads/failures, large outputs (see below)
agentmeter --instructions .  # the instruction files that apply to a directory (see below)
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

## Asking Claude Code: `--probe`

The logs show how large the first request of a session is, not what it is made of. `--probe` asks
Claude Code itself: it runs `claude -p /context --no-session-persistence --output-format json` in
an empty directory and in the three busiest projects of the period, and splits the median session
start into today's configuration (system prompt, tools, skills, memory files, MCP servers), the
rest of the smallest start and what varies per session. Tools loaded on demand are listed apart,
because they are not sent at the start.

`/context` is answered locally: no model request, no cost, no session file. Starting `claude`
does load the configuration of each directory, including its MCP servers and hooks, which is why
the probe only runs when asked for (`--probe`, or *Context probe* in the menu). It measures
today's configuration, not the one the period ran with, and it measures the command-line
`claude`: the report names the front end that wrote the sessions, since a desktop app or IDE adds
instructions and tools of its own.

## Transitions: `--transitions`

A timeline of session starts, resumes and forks, compactions and pauses of an hour or more, one
per log file, from metadata only. Each compaction shows the context before, after (only where the
log records it; never a guess) and at the next request, the time until it and the requests until
the next compaction, with file and event id as evidence. A resume or fork names its parent only
where the log does; sessions are never linked by time, and a pause or a new chat is a marker, not
a loss of context. With `--tool-calls` each compaction also counts the areas read again in the
next 30 calls that were read before it and not edited since, and a project hint appears when this
repeats; consumption after a transition is measured, not proven avoidable.

## Comparing periods: `--compare <Nd>`

Puts the last N days next to the N days before them, from the same logs and with the same rules
as the other views. Each figure shows both values, the absolute and relative change and what it
was divided by: requests per active day, input per request, cache share, compactions per 1,000
requests, median start context with its sample, sub-agent share and cache rebuilds. Days without
logs are counted and figures are per active day or per request, not per calendar day. A window
with fewer than 3 active days or 100 requests is shown without a change, and a shift in the models
or front ends between the windows is listed as a limitation. A figure the logs cannot give stays
unknown. A change is shown, not explained, and less or more input is not better or worse work.
`--project` and `--tool` apply; `--since` does not combine with it.

## Before and after a change: `--changed-on <date>`

Shows the figures of `--compare` for the period before a date you give and an equally long period
from that date to now. Name the change with `--change-label "<text>"` if you like; Agent Meter does
not know what changed on that day and never guesses it. A difference between the periods is not an
effect of the change: other tasks, a different model or front end and short periods change the
numbers too, and these are listed as limitations. Less or more input is not better or worse work.
Nothing is stored. `--project` and `--tool` apply; it does not combine with `--since`.

## Tool calls: `--tool-calls`

The default mode reads usage metadata only. `--tool-calls` is an explicit extension: it also opens
the message content for tool calls and their results and reports three rules, each with period,
sample, counter-examples, evidence and a next step:

- `repeat-read`: the same file and line range read again with no observed edit of that file and no
  compaction in between. Other line ranges are separate areas.
- `repeat-failure`: an identical failed call repeated with no observed edit in the session
  between the attempts (a retest after a fix does not count).
- `large-output`: single results of 100,000 bytes or more.

What is kept: call ids, tool names, file paths of file tools, a hash of what makes other calls
identical (a command is shown as `Bash #a1b2c3d4`, never as text) and the byte size of results.
No command text and no output text is stored or printed. Edits made by commands are not observed,
so the rules name that uncertainty; limits are uncalibrated; bytes are not tokens, and the tokens
of a request are not attributed to one tool. Calls without a result stay unknown, not successful.
Codex logs its tool arguments as scripts, so only `large-output` is measurable there; the other
rules say "not measurable". Details: [Findings](docs/reference/findings.md).

## Instruction files: `--instructions <dir>`

Shows which instruction files apply to one directory, where each comes from, how large it is and
whether the agent's loading rules put it into the context at session start. The files are only
read, no agent is started. Each agent follows its own rules and the output names the version of
the documentation they come from:

- **Claude Code:** `CLAUDE.md`, `.claude/CLAUDE.md` and `CLAUDE.local.md` in the directory and
  every directory above it, `~/.claude/CLAUDE.md`, rules in `.claude/rules/` (with `paths:`
  frontmatter: on demand), `@path` imports up to four hops, `claudeMdExcludes`, `AGENTS.md` only
  when no `CLAUDE.md` exists. Files in subdirectories load on demand.
- **Codex CLI:** `AGENTS.override.md` or `AGENTS.md` in the Codex home, then from the `.git`
  root down to the directory one file per directory, cut at `project_doc_max_bytes` (default
  32 KiB).

Every file has a state: `expected` (the rules say it loads), `confirmed` (with `--probe`, Claude
Code lists it), `on demand`, `not loaded` (excluded, shadowed, beyond a limit), `unreadable` or
`missing`. Only expected and confirmed files count towards the size at start; tokens are
estimated as bytes ÷ 4. Findings (`broken-import`, `duplicate-instructions`, `base-size`,
`load-limit`) carry file, measure, certainty and a step to check; a large file alone is none.
This is today's configuration, separate from the log history. Worktrees are separate paths:
scan the worktree to see its files. Rules in [Findings](docs/reference/findings.md).

## Privacy

Read-only, no network, no telemetry; `--tool-calls` opens message content for call ids, paths, command fingerprints and result sizes (see above); `--probe` is the one exception that starts another program
(`claude`, see above). The log files contain prompt and tool text, so they are
read in full, but agentmeter extracts only usage metadata (token counts, timestamps, model,
session and project ids, compaction events). Prompt and tool text is not analysed, stored or
printed; evidence points to a file and an event id, never to text. With `--probe` the report also
shows what `/context` lists: category sizes, paths of memory files and names of MCP servers.

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
