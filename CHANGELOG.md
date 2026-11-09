# Changelog

All notable changes to agentmeter are listed here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and the project follows
[Semantic Versioning](https://semver.org/): while the version is 0.x, minor releases may change
behaviour.

## [Unreleased]

### Added

- **CLI:** `--capture <name> --since <period>` saves the figures of `--compare` for one source and period in a local JSON file (numbers only, `schema_version` 1, never overwritten, optional `--change-label`), `--captures` lists them, `--compare-captures <earlier> <later>` compares two with the same rules and `--delete-capture <name>` removes one. Captures from other sources, projects or rule versions are reported as not comparable, with the reason; files live under `$AGENTMETER_DATA_DIR`, `$XDG_DATA_HOME/agentmeter` or `~/.local/share/agentmeter` ([#16](https://github.com/casoon/agentmeter/issues/16)).

- **CLI:** `--changed-on <date>` (with an optional `--change-label`) puts the period before a change date you give next to the equally long period since it, through the same comparison as `--compare`: both values, absolute and relative change and denominators, with model or front-end shifts and too-short periods named as limitations. The date is never guessed, a difference is not presented as an effect of the change, and nothing is stored. JSON key `before_after` ([#16](https://github.com/casoon/agentmeter/issues/16)).

- **CLI:** `--compare <Nd>` puts the last N days next to the N days before them, computed from the logs with the existing analysis: requests per active day, input per request, cache share, compactions per 1,000 requests, median start context (`baseline-load`), sub-agent share and cache rebuilds, each with both values, absolute and relative change and the denominators. Days without logs are counted; a window with too few active days or requests gets no change, only its values, and model or front-end shifts between the windows are named as limitations. It shows changes, not causes. JSON key `compare`; the core function `agentmeter_core::compare::compare` takes any two windows ([#18](https://github.com/casoon/agentmeter/issues/18)).

- **CLI:** `--transitions` prints a timeline per log file of session starts, resumes and forks (parent only where the log names it), incomplete logs, compactions and pauses of one hour or more, with the context before, after (only if logged) and at the next request, time to it and requests to the next compaction; with `--tool-calls` also the areas read again after a compaction and a project hint. A pause or a new chat is a marker, never a verdict; compactions are the events the summary counts. JSON key `transitions` ([#14](https://github.com/casoon/agentmeter/issues/14)).

- **CLI:** `--tool-calls` is an explicit extension that also reads tool calls and results and reports `repeat-read`, `repeat-failure` and `large-output` with period, sample, counter-examples (re-read after an observed edit or compaction, other line ranges), evidence and next step; limits are uncalibrated. Only ids, tool names, file paths, command fingerprints and result sizes are kept, no command or output text; Codex measures `large-output` only. JSON key `tool_calls` ([#13](https://github.com/casoon/agentmeter/issues/13)).

- **CLI:** `--instructions <dir>` lists the instruction files that apply to a directory by each agent's own loading rules (Claude Code: `CLAUDE.md` hierarchy, rules, imports, excludes; Codex: `AGENTS.md` from the `.git` root with `project_doc_max_bytes`) with source, size, estimated tokens and load state (`expected`, `confirmed` via `--probe`, `on_demand`, `not_loaded`, `unreadable`, `missing`); findings `broken-import`, `duplicate-instructions`, `base-size`, `load-limit`. Reads files only; JSON key `instructions` ([#12](https://github.com/casoon/agentmeter/issues/12)).

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
