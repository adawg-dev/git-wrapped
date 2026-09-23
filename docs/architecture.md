# Architecture

The workspace has one Rust package, `git-wrapped`. Its binary in `main.rs` parses CLI arguments and coordinates the library:

```text
Git repository
  → git.rs (discover repository; stream HEAD-reachable commits and numstat)
  → config.rs (load aliases after Git .mailmap identities)
  → analysis.rs (aggregate canonical model.rs data)
  → awards.rs (deterministic award selection)
  → render.rs (SVG report and data.json) / JSON stdout export
```

`git.rs` invokes Git with explicit arguments and reads a null-delimited stream. It does not execute repository code. `analysis.rs` uses one scan to compute repository, contributor, commit, and activity records. `render.rs` consumes that dataset, escapes dynamic SVG text, and writes fixed artifact names; the CLI sanitizes terminal text. Reports are deterministic for the same history and alias configuration.

The current model covers historical activity and line changes. Ownership, code survival, caching, optional external analyzers, PNG, a terminal explorer, and animation are later independent phases; they have no placeholder crates or commands here. The [handoff spec](../git-wrapped-handoff.md) describes that broader direction.
