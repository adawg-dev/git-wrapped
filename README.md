# Git Wrapped

Git Wrapped reads a local repository's HEAD-reachable Git history and writes a shareable SVG report and deterministic JSON data. It needs Rust and Git; no hosting account or repository scripts are required.

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
git-wrapped [PATH] [--output DIR] [--theme dark|light]
git-wrapped report [PATH] [--output DIR] [--theme dark|light]
git-wrapped export --format json [PATH]
```

`PATH` defaults to `.`, and `--output` defaults to `git-wrapped-report` relative to the directory where you invoked the command. `--theme` defaults to `dark`. Export writes JSON to stdout; report commands write files and print a short summary. `--help` lists the current options. A repository with no commits or an invalid configuration exits with an error before creating report files.

## Report files

```text
git-wrapped-report/
├── summary.svg
├── contributors.svg
├── activity.svg
├── activity-heatmap.svg
├── awards/
│   ├── commit-machine.svg
│   ├── code-creator.svg
│   ├── code-destroyer.svg
│   └── night-owl.svg
└── data.json
```

The four award cards are always present; an ineligible award displays “No eligible winner.” JSON contains repository, contributor, commit, activity, heatmap, and eligible award records. The summary and charts use the same analytics as JSON.

## What the numbers mean

- **Commits:** HEAD-reachable commits, including root, empty, and merge commits. The date range uses author dates.
- **Lifetime additions/deletions:** Lines added and removed in Git numstat history. Binary changes count as touched files but add zero lines. Merge commits count as commits but contribute no line or file changes, avoiding duplicate merge diffs. Root commit additions count.
- **Net historical lines:** Lifetime additions minus lifetime deletions. **Churn:** their sum. Neither measures current code ownership or surviving lines.
- **Files touched:** Distinct paths changed by a contributor; a rename counts under its new path. **Tracked files:** Files in the current HEAD tree.
- **Activity:** Author date and hour in each commit's recorded author offset. A contributor's active days and time-of-day awards use those local dates and hours, with no conversion to your machine's timezone.

Git's `.mailmap` is applied before aliases from `.git-wrapped.json`. The config currently accepts only contributor aliases:

```json
{
  "contributors": {
    "Ada": ["ada@example.com", "ada@work.example"]
  }
}
```

Contributor IDs are case-folded email addresses (or normalized names when email is blank). Raw author identities remain in commit data. Git Wrapped uses HEAD-reachable local history, its merge and binary rules, and those identity rules, so totals can differ from GitHub or GitLab displays.

Awards select deterministic winners by metric, breaking ties by contributor ID. Commit Machine, Code Creator, Code Destroyer, Net Positive, Biggest Bang, Biggest Cleanup, and Repo Explorer require a positive value. Night Owl (00:00–04:59), Early Bird (05:00–08:59), and Weekend Warrior (Saturday/Sunday) require at least five commits and a positive qualifying share.

Shallow clones still work, but a warning says historical totals cover **available history** only. Use a full clone for lifetime totals.

## Scope

The current release produces SVG and JSON. Ownership, surviving LOC, PNG, caching, a terminal explorer, external analyzers, and animation are separate future work. See [architecture](docs/architecture.md) for the current data flow.
