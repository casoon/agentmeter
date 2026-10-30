# Changelog

All notable changes to agentmeter are listed here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and the project follows
[Semantic Versioning](https://semver.org/): while the version is 0.x, minor releases may change
behaviour.

## [Unreleased]

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

### Fixed

- **Build:** the declared minimum Rust version is 1.88, the version the code and `ruzstd` need; 1.85 did not build.
- **CLI:** `--since` with a day count too large to compute is refused as a bad call instead of overflowing.
- **CLI:** `--project` that matches no project in any source exits 1 with a message instead of printing an empty summary.
- **CLI:** requests without a model name appear as `(unknown)` in the model table instead of a blank row.
- **Findings:** the `baseline-load` next step names `/context` as a Claude Code command instead of suggesting it for every source.
