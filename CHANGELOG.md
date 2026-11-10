# Changelog

All notable changes to agentmeter are listed here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and the project follows
[Semantic Versioning](https://semver.org/): while the version is 0.x, minor releases may change
behaviour.

## [Unreleased]

## [0.2.0] - 2026-11-10

### Added

- **CLI:** `--instructions <dir>` lists the instruction files that apply to a directory by each agent's own loading rules, with source, size, estimated tokens and load state, and reports `broken-import`, `duplicate-instructions`, `base-size` and `load-limit`. JSON key `instructions` ([#12](https://github.com/casoon/agentmeter/issues/12)).
- **CLI:** `--tool-calls` also reads tool calls and results and reports `repeat-read`, `repeat-failure` and `large-output` with sample, counter-examples and next step. Only ids, tool names, file paths, command fingerprints and result sizes are kept; Codex measures `large-output` only. JSON key `tool_calls` ([#13](https://github.com/casoon/agentmeter/issues/13)).
- **CLI:** `--transitions` prints a timeline per log file of session starts, resumes and forks, compactions and pauses with the context around each; with `--tool-calls` also the areas read again after a compaction. A pause or a new chat is a marker, never a verdict. JSON key `transitions` ([#14](https://github.com/casoon/agentmeter/issues/14)).
- **CLI:** `--subagents` splits each session's usage into its own requests and those of the agents it delegated to, assigning an agent only where the log names its parent, and lists overlapping runs. JSON key `subagents` ([#15](https://github.com/casoon/agentmeter/issues/15)).
- **CLI:** `--changed-on <date>` (optional `--change-label`) puts the period before a change date you give next to the equally long period since it. The date is never guessed and a difference is not presented as an effect of the change. JSON key `before_after` ([#16](https://github.com/casoon/agentmeter/issues/16)).
- **CLI:** `--capture <name> --since <period>` saves the comparison figures of one source and period in a local JSON file (numbers only); `--captures` lists them, `--compare-captures <earlier> <later>` compares two and `--delete-capture <name>` removes one ([#16](https://github.com/casoon/agentmeter/issues/16)).
- **CLI:** `--ai-report` prints a compact diagnosis report as one line of JSON for an AI to evaluate: scope and limitations first, then the first findings (`--ai-report-max <n>`), each separating measurement, uncertainty and guess. No prompt, command, output or configuration text is taken over. JSON key `ai_report` ([#17](https://github.com/casoon/agentmeter/issues/17)).
- **CLI:** `--compare <Nd>` puts the last N days next to the N days before them, with both values, absolute and relative change and the denominators; too-short windows and model shifts are named as limitations. It shows changes, not causes. JSON key `compare` ([#18](https://github.com/casoon/agentmeter/issues/18)).
- **CLI:** `--skills` shows which installed Claude Code skills were called in the period and which were not observed, with the estimated size of each entry in the session-start skill list. No skill content or arguments are kept; Codex is "not measurable". JSON key `skills` ([#19](https://github.com/casoon/agentmeter/issues/19)).
- **CLI:** `--compaction-causes` groups the compactions by project, model and session and words patterns as hints, never as the cause; `--compaction-settings` also reads today's `autoCompactWindow` and `autoCompactEnabled` and compares them with the observed sizes. JSON key `compaction_causes` ([#20](https://github.com/casoon/agentmeter/issues/20)).
- **CLI:** the data quality names days without requests inside a source's period, as neither pause nor lost logs, and `--retention` reads `cleanupPeriodDays` and says when the oldest request is close to it. JSON fields `data_quality.gaps`, `compare.*.gaps` and `retention` ([#21](https://github.com/casoon/agentmeter/issues/21)).

### Changed

- **Pages:** the project site uses the current shared theme (0.12.6).

## [0.1.1] - 2026-10-31

### Fixed

- **CLI:** text output stays within the terminal width, at most 88 columns: evidence lines and long paths break after `/` and before ` · ` with an indent, and the sessions and projects tables shrink their bars instead of spilling over ([#2](https://github.com/casoon/agentmeter/issues/2)).

## [0.1.0] - 2026-10-30

### Added

- **CLI:** `agentmeter` reads Claude Code session logs (`~/.claude/projects`, `CLAUDE_CONFIG_DIR`) and prints a summary with days and compactions; `--sessions`, `--projects`, `--findings`, `--since <Nd|date>`, `--project <name>` and `--json` select and filter. Read-only, no network.
- **Adapter:** merges streaming duplicates and drops sub-agent replays, reads cache writes by lifetime (5 min / 1 h) from the nested `cache_creation` object, and counts unreadable lines, unknown entry types and unknown usage fields instead of failing.
- **Findings:** `baseline-load`, `cache-rebuild`, `compact-thrash`, `big-context` and `long-session` report an observation with rule version, measure, denominator, sample, evidence (file and event id), certainty, counterexamples and a next step. None is an error; thresholds are uncalibrated.
- **Data quality:** the report states the observed period, files without usage, unreadable entries and rules that could not be measured, and separates "no analysis possible" from "no findings".
- **Codex adapter:** reads Codex CLI rollouts (`$CODEX_HOME` or `~/.codex`) from the running `token_count` totals, ignoring repeated events and replayed parent history. Compressed `.jsonl.zst` files are streamed within limits; damaged ones are counted, never fatal. Rules that need cache writes or compaction sizes report "not measurable" instead of guessing.
- **CLI:** `--tool claude|codex|all` selects the source; by default every source that has logs is reported on its own, with its own data quality. `--json` now returns a `sources` array.
- **Findings:** every rule appears once per source as observed, checked with nothing to report (with its measure and sample) or not measurable (with the reason), in text and JSON. `big-context` uses 50 % of the logged context window where a window is recorded (Codex) and 500k otherwise.
- **CLI:** `agentmeter` without arguments on a terminal opens a menu (summary, sessions, projects, one row per rule with its status, data quality per source) with the source and the period selectable; in a pipe or with any flag it prints the text as before.
- **CLI:** `--check data-quality` exits 1 when a source has unreadable lines or compressed files over a limit or no usage in the period, 0 otherwise and 2 for a bad call. Notices and info never fail it.
- **JSON:** the output carries `schema_version` (1) and is documented field by field in [JSON output](https://casoon.github.io/agentmeter/docs/reference/json/).
- **CLI:** `--probe` asks Claude Code via `/context` (local, no model request) what a session start holds and splits the median start into configuration and the rest; also in the menu. Details: [Asking Claude Code](https://github.com/casoon/agentmeter#asking-claude-code---probe).
- **Findings:** every diagnosis opens with a conclusion (lever, look at it, fine) and splits its measure into parts; `conclusion` and `parts` are in the JSON. Details: [Conclusions](https://casoon.github.io/agentmeter/docs/reference/findings/#conclusions).
- **Findings:** `cache-rebuild` lists sub-agent files apart from the main conversation and, where writes are split by lifetime, compares a 1 h cache lifetime with 5 min at list-price ratios; `baseline-load` names the front end that wrote the session starts (`entrypoint`, Codex `originator`).
- **CLI:** the text output starts with a block of what was read and how well, and shows bars for days, models, sessions, projects and every breakdown.

- **Release:** binaries for macOS and Linux on the GitHub release with `.sha256`, `SHA256SUMS` and a signed build provenance attestation; the crates are on crates.io (`cargo install agentmeter`). Details: [Install](https://github.com/casoon/agentmeter#install).

### Fixed

- **Build:** the declared minimum Rust version is 1.88, the version the code and `ruzstd` need; 1.85 did not build.
- **CLI:** `--since` with a day count too large to compute is refused as a bad call instead of overflowing.
- **CLI:** `--project` that matches no project in any source exits 1 with a message instead of printing an empty summary.
- **CLI:** requests without a model name appear as `(unknown)` in the model table instead of a blank row.
- **Findings:** the `baseline-load` next step names `/context` as a Claude Code command instead of suggesting it for every source.
