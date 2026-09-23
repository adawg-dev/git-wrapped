# Architecture direction

The intended flow is: local Git repository → Rust history analysis → canonical analytics data → CLI and renderers → JSON and visual artifacts. The CLI is the only workspace member so far. Add separate crates when implemented responsibilities need them; keep the canonical data independent of optional external tools.

The [handoff spec](../git-wrapped-handoff.md) defines the product scope and development phases. This document will be refined during implementation planning.
