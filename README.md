# Git Wrapped

Git Wrapped reads a local repository's HEAD-reachable Git history and writes a shareable SVG report, a PNG poster, and deterministic JSON data. It needs Rust and Git; no hosting account or repository scripts are required.

## Build and run

From the Git Wrapped project checkout:

```sh
cargo build --release
cargo run -- /path/to/repository
cargo run -- export --format json /path/to/repository > data.json
cargo install --path crates/git-wrapped-cli
```

`cargo run` works from the project checkout. After `cargo install`, run the binary from any repository to use that repository as the default path:

```sh
cd /path/to/repository
git-wrapped
```

The installed binary also accepts an explicit path:

```sh
git-wrapped [PATH] [--output DIR] [--theme dark|light] [--no-png] [--no-cache] [--verbose]
git-wrapped report [PATH] [--output DIR] [--theme dark|light] [--no-png] [--no-cache] [--verbose]
git-wrapped export --format json [PATH]
git-wrapped contributors [--by commits|additions|deletions|churn|net|files|active-days] [PATH]
git-wrapped contributor <ID-OR-NAME> [PATH]
git-wrapped activity [--bucket day|week|month|quarter|year] [PATH]
git-wrapped archaeology [PATH]
git-wrapped awards [PATH]
git-wrapped top commits|additions|deletions|churn|files [PATH]
```

`PATH` defaults to `.`, and `--output` defaults to `git-wrapped-report` relative to the directory where you invoked the command. `--theme` defaults to `dark`. Reports write `summary.png` at 2400×3200 by default; `--no-png` skips PNG generation (an earlier PNG in the same output directory is left alone). Export writes JSON to stdout; report commands write files and print a short summary. Focused commands print text, with up to 20 rows per list. `contributors` defaults to commits; `top` is a shortcut for its five named rankings, including `top files` for contributors by distinct files touched. `contributor` accepts an exact ID or a unique case-insensitive display name. `activity` defaults to month and shows the latest 20 periods. `archaeology` ranks changed paths by historical churn and marks paths absent from HEAD as historical. Global `--exclude PATTERN` may repeat before or after a subcommand. `--help` lists the current options. A repository with no commits or an invalid configuration exits with an error before creating report files.

## Report files

```text
git-wrapped-report/
├── summary.svg
├── summary.png
├── commits-over-time.svg
├── additions-deletions.svg
├── rhythm.svg
├── highlights.svg
├── contributor-mix.svg
├── file-churn.svg
├── directories.svg
├── contributors.svg
├── activity.svg
├── activity-heatmap.svg
├── ownership.svg                 # with --deep
├── ownership-over-time.svg       # with --deep
├── ship-of-theseus.svg           # with --deep
├── contributors/<stable-id>.svg
├── awards/
│   ├── commit-machine.svg
│   ├── code-creator.svg
│   ├── code-destroyer.svg
│   ├── night-owl.svg
│   └── <eligible-award>.svg
├── card-manifest.json
├── .git-wrapped-cache.json
└── data.json
```

The four original award cards are always present; an ineligible award displays “No eligible winner.” JSON contains repository, contributor, commit, activity, heatmap, and eligible award records. The summary and charts use the same analytics as JSON. The PNG is rasterized from `summary.svg` with bundled Lato Regular under the [SIL Open Font License](crates/git-wrapped-cli/assets/OFL.txt); other glyphs may use a fallback in SVG viewers, while unsupported glyphs may be absent from the PNG. PNG generation uses a maximum scale of 4 and a 64 million pixel limit.

Reports reuse `.git-wrapped-cache.json` when the canonical repository path, HEAD commit, shallow history boundary, mailmap inputs, tag refs, config, and analysis selection are identical. A repeat run says `Using cached analysis` on stderr. A stale, corrupt, or oversized cache is recomputed. `--no-cache` forces a fresh analysis; `export` bypasses the cache and creates no report directory. Cache write failures warn on stderr while the report still succeeds.

Long scans show coarse progress on stderr when stderr is a terminal; `--verbose` enables it when output is redirected. Export keeps stdout as JSON. Press Ctrl+C to cancel a scan (exit code 130); cancellation before rendering leaves no report files from that run.

## What the numbers mean

- **Commits:** HEAD-reachable commits, including root, empty, and merge commits. The date range uses author dates.
- **Lifetime additions/deletions:** Lines added and removed in Git numstat history. Binary changes count as touched files but add zero lines. Merge commits count as commits but contribute no line or file changes, avoiding duplicate merge diffs. Root commit additions count.
- **Net historical lines:** Lifetime additions minus lifetime deletions. **Historical churn:** their sum across selected changes. File churn is attributed to the changed path, including the new path of a rename; directory churn is attributed to that path's immediate parent. These totals do not measure current code size, surviving lines, ownership, or code quality.
- **Files touched:** Distinct paths changed by a contributor; a rename counts under its new path. **Tracked files:** Files in the current HEAD tree.
- **Activity:** Author date and hour in each commit's recorded author offset. Active days count distinct author dates. Day, week, and month views include zero periods between first and last activity; quarter and year group months. The selected bucket is capped at 20,000 periods, with a prompt to choose a coarser period if exceeded. Streaks end at the latest selected activity date, not today.
- **Contributor tenure and overlap:** Tenure spans first to last selected author date; a return means a selected commit after at least 90 days without one. Overlap counts calendar weeks when both contributors committed, among the top 20 by commits. It does not establish direct collaboration, review, employment, or team membership.
- **Commit concentration proxy:** JSON's `bus_factor_proxy` is the smallest number of normalized contributors whose commits cover at least half the selected commits. It describes commit concentration, not the actual bus factor or project resilience.
- **Growth and release timing:** Monthly net growth is additions minus deletions, not current lines of code. Churn is additions plus deletions. Release intervals use reachable tags on distinct target commits and are descriptive; tags do not add commits.
- **Deep ownership (`--deep`):** `ownership.svg` counts surviving regular text lines in HEAD by normalized contributor, immediate directory, and extension. Date and author selectors do not restrict this current snapshot; path exclusions do. The chart shows analyzed/eligible file counts, unknown lines, skipped binary files and submodules, and partial coverage when the shared deep budget ends.
- **Sampled ownership history (`--deep`):** `ownership-over-time.svg` shows up to 12 selected commits evenly spaced by commit index and placed at their actual selected-timezone calendar dates. Every stacked column is a sampled snapshot; dates between columns are unmeasured. Each snapshot's JSON coverage records eligible/analyzed files, skipped files, unknown lines, and truncation. Charts show the top eight normalized contributor IDs and group the rest as Other; unknown lines remain separate. The shared file, line, and time budgets can truncate this history. Git blame origin attribution is an estimate of surviving line ownership, not exact authorship across rewrites or renames.

Git's `.mailmap` is applied before aliases from `.git-wrapped.json`. The config can also set a timezone and opt-in path exclusions:

```json
{
  "contributors": {
    "Ada": ["ada@example.com", "ada@work.example"]
  },
  "timezone": "utc",
  "exclude": ["**/*.lock", "vendor/**"]
}
```

No paths are excluded by default. Config patterns run first, then any `--exclude` patterns; `--timezone` overrides the config timezone. Exclusions match the changed path (the new path for a rename) after commit selection. A matched change contributes no file or line metrics, but its commit still counts. HEAD tracked-file counts and current file/extension/directory counts also omit matching paths. JSON records the exact `excluded_patterns` and the number of selected historical changes omitted as `excluded_changes`. Patterns match the lossy UTF-8 display form of a path; raw byte paths still have distinct IDs. Invalid patterns report their source and pattern.

Contributor IDs are case-folded email addresses (or normalized names when email is blank). Raw author identities remain in commit data. Git Wrapped uses HEAD-reachable local history, its merge and binary rules, and those identity rules, so totals can differ from GitHub or GitLab displays.

Awards select deterministic winners by metric, breaking ties by contributor ID. Commit Machine, Code Creator, Code Destroyer, Net Positive, Biggest Bang, Biggest Cleanup, and Repo Explorer require a positive value. Night Owl (00:00–04:59), Early Bird (05:00–08:59), and Weekend Warrior (Saturday/Sunday) require at least five commits and a positive qualifying share.

Shallow clones still work, but a warning says historical totals cover **available history** only. Use a full clone for lifetime totals.

## Scope

The current release produces SVG, PNG, JSON, and focused terminal views. Ownership, surviving LOC, a terminal explorer, external analyzers, and animation are separate future work. See [architecture](docs/architecture.md) for the current data flow.
