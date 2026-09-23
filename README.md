# Git Wrapped

Git Wrapped is a local Git analytics tool that turns a repository's history into stats, charts, awards, and shareable reports.

Think contributor leaderboards, code churn, activity heatmaps, ownership, repo archaeology, and eventually animated history, all generated from the Git repository itself.

No hosting integration is required. If it is a Git repo, Git Wrapped should be able to analyze it.

## What it does

Git Wrapped looks through a repository's history and answers questions like:

- Who has the most commits?
- Who has added the most code?
- Who has deleted the most code?
- Who has the highest total churn?
- Who has touched the most files?
- Who commits the latest at night?
- Who works on weekends the most?
- What parts of the repo change the most?
- How has activity changed over time?
- Which contributor currently owns the most surviving code?
- How much old code is still around?

It also generates a few less serious awards.

Examples:

- Commit Machine
- Code Creator
- Code Destroyer
- Night Owl
- Early Bird
- Weekend Warrior
- Refactor Goblin
- Repo Explorer
- Biggest Bang
- Biggest Cleanup

Awards are based on actual repository metrics and are deterministic.

## Example

Run Git Wrapped from inside a repository:

```bash
git-wrapped
```

Or point it at another repo:

```bash
git-wrapped /path/to/repository
```

Example terminal output:

```text
Git Wrapped

Repository
────────────────────────────────────────────
7,482 commits
23 contributors
4y 7mo old
1,842,093 lines added
1,203,552 lines deleted
638,541 net lines

Awards
────────────────────────────────────────────
🏆 Commit Machine       Adam       2,182 commits
🔥 Code Creator         Thomas     412,482 lines added
💀 Code Destroyer       CJ         381,291 lines deleted
🌙 Night Owl            Adam       31.2% after midnight
🗺 Repo Explorer         CJ         843 unique files

Rendering report...
✓ summary.svg
✓ contributors.svg
✓ activity.svg
✓ activity-heatmap.svg
✓ awards/
✓ data.json

Report written to:
./git-wrapped-report/
```

## Output

A typical report looks something like this:

```text
git-wrapped-report/
├── summary.svg
├── summary.png
├── contributors.svg
├── commits-over-time.svg
├── additions-deletions.svg
├── activity-heatmap.svg
├── file-churn.svg
├── ownership.svg
├── code-survival.svg
├── contributor-interactions.svg
├── awards/
│   ├── commit-machine.svg
│   ├── code-creator.svg
│   ├── code-destroyer.svg
│   ├── night-owl.svg
│   └── weekend-warrior.svg
└── data.json
```

SVG is the primary output format for reports and charts. PNG export is useful for sharing, and JSON export makes it easy to inspect or reuse the underlying analytics.

## Why

Git contains a ridiculous amount of history that usually gets reduced to a commit list and a blame view.

Git Wrapped is an excuse to dig into the rest of it.

The goal is to make repository history fun to explore while still keeping the underlying metrics useful and reasonably rigorous.

## Features

### Repository stats

Git Wrapped can calculate:

- total commits
- total contributors
- first and latest commit
- repository age
- total lines added
- total lines deleted
- net historical change
- total churn
- tracked files
- activity over time

### Contributor stats

Per contributor:

- commits
- percentage of commits
- lines added
- lines deleted
- net lines
- churn
- files touched
- directories touched
- active days
- first contribution
- latest contribution
- average commit size
- median commit size
- largest commit
- largest deletion
- commits by hour
- commits by weekday
- commits by month

### File and directory stats

Git Wrapped can also look at:

- high-churn files
- frequently modified directories
- files with many contributors
- long-lived files
- frequently rewritten files
- co-changing files
- file and directory activity over time

### Contributor identity normalization

Git history gets messy when one person has committed under several names or email addresses.

Git Wrapped supports:

- `.mailmap`
- multiple email addresses
- GitHub noreply addresses
- custom aliases
- case normalization

The goal is to avoid treating one person as three separate contributors.

## Metric definitions

Some Git statistics sound similar but mean very different things.

Git Wrapped keeps them separate.

### Lines added

The total number of lines introduced by a contributor across the repository's history.

### Lines deleted

The total number of lines removed by a contributor.

### Net lines

```text
lines added - lines deleted
```

This is a historical delta, not the amount of code that contributor currently owns.

### Churn

```text
lines added + lines deleted
```

This is useful for measuring how much code a contributor has changed overall.

### Surviving LOC

The amount of code currently present in the repository that is attributed to a contributor.

This requires blame or ownership analysis and is different from lifetime additions.

For example:

```text
Alice

Commits:                 1,403
Lines ever added:      320,491
Lines ever deleted:    281,091
Net historical change: +39,400
Current surviving LOC:  18,382
```

All of those numbers can be correct at the same time.

## CLI

Planned command structure:

```bash
# Analyze the current repository
git-wrapped

# Analyze another repository
git-wrapped /repo/foo

# Generate the full report
git-wrapped report

# Contributor leaderboard
git-wrapped contributors

# Single contributor
git-wrapped contributor "Adam Xu"

# Activity charts
git-wrapped activity

# Ownership analysis
git-wrapped ownership

# File archaeology
git-wrapped archaeology

# Awards
git-wrapped awards

# Machine-readable export
git-wrapped export --format json

# Interactive terminal explorer
git-wrapped explore

# Animated repository history
git-wrapped animate
```

Convenience commands are also planned:

```bash
git-wrapped top commits
git-wrapped top additions
git-wrapped top deletions
git-wrapped top churn
git-wrapped top files
```

## Configuration

Git Wrapped can read configuration from:

```text
.git-wrapped.json
```

Example:

```json
{
  "exclude": [
    "package-lock.json",
    "pnpm-lock.yaml",
    "yarn.lock",
    "dist/**",
    "vendor/**"
  ],
  "contributors": {
    "Adam Xu": [
      "adam@example.com",
      "adam@company.com"
    ]
  },
  "timezone": "local",
  "theme": "dark"
}
```

This can be used to:

- exclude generated or vendored files
- merge contributor identities
- control timezone handling
- choose a report theme

## Installation

Git Wrapped is written in Rust.

Once published as a crate:

```bash
cargo install git-wrapped
```

For local development:

```bash
git clone <repository-url>
cd git-wrapped
cargo build
```

Run it with:

```bash
cargo run -- /path/to/repository
```

For an optimized build:

```bash
cargo build --release
```

The binary will be available at:

```text
target/release/git-wrapped
```

## Development

The project is organized as a Cargo workspace.

```text
git-wrapped/
├── crates/
│   ├── git-wrapped-cli/
│   ├── git-wrapped-core/
│   ├── git-wrapped-analysis/
│   ├── git-wrapped-render/
│   ├── git-wrapped-awards/
│   ├── git-wrapped-cache/
│   ├── git-wrapped-tui/
│   └── git-wrapped-external/
├── fixtures/
├── tests/
└── examples/
```

The general data flow is:

```text
Git repository
      ↓
Rust analytics engine
      ↓
canonical dataset
      ↓
CLI / TUI / renderer
      ↓
SVG / PNG / JSON / MP4
```

Core analytics should not depend on GitHub, GitLab, or any other hosting provider.

## External tools

Git Wrapped is intended to implement its important metrics natively, but some external tools can provide useful optional functionality.

Possible integrations include:

- Gource
- git-fame
- git-of-theseus
- Hercules
- git-quick-stats
- ffmpeg

These are optional.

The main application should continue to work if none of them are installed.

## Large repositories

Git Wrapped is intended to support repositories with long histories and large commit graphs.

The implementation should avoid:

- repeated full-history scans
- unnecessary blob reads
- re-running blame across the entire repository
- loading huge Git outputs into one string
- O(commits × files) algorithms where possible

Caching and incremental analysis are planned so repeated runs do not have to start from zero.

## Shallow clones

Git Wrapped detects shallow repositories.

If history is incomplete, historical totals are reported as covering only the available history.

For accurate lifetime statistics, use a full clone.

## Safety

Repositories are treated as untrusted input.

Git Wrapped does not run repository code, package scripts, build systems, hooks, or arbitrary binaries from the repository.

It only inspects Git data and repository files required for analysis.

## Roadmap

### Phase 1

- repository discovery
- Git history ingestion
- contributor normalization
- core contributor metrics
- deterministic JSON export

### Phase 2

- summary report
- contributor charts
- activity charts
- activity heatmap
- award cards
- SVG and PNG output

### Phase 3

- interactive TUI
- contributor profiles
- repo exploration

### Phase 4

- blame-based ownership
- surviving LOC
- directory ownership

### Phase 5

- code survival
- code age
- file coupling
- historical ownership
- contributor interaction matrix
- Ship of Theseus visualization

### Phase 6

- Gource integration
- ffmpeg rendering
- animated repository history

## Current status

Git Wrapped is under active development.

The first target is simple:

```bash
git-wrapped /path/to/repo
```

should produce a useful terminal summary, a clean `summary.svg`, contributor charts, activity charts, awards, and a complete `data.json`.

Everything else builds on top of that.

## License

License information will be added before the first public release.
