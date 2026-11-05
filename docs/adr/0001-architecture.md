# ADR-0001: Overall architecture

Status: accepted (2026-10-14), based on the spikes in `spikes/` and the decisions in ADRs 0002–0005.

## Decision

semcode **owns syntax, patterns, symbol identity, edit transactions and flow analysis**. For
each language, it uses that language's best compiler-grade analyzer, run as a managed LSP
subprocess, as the **oracle** for name resolution, types, references, renames and call hierarchy.

```
            agents ──MCP──┐        humans ──CLI──┐
                          ▼                      ▼
                 ┌──────────────── semcoded (per-workspace daemon) ────────────────┐
                 │  query planner · budgeted renderer · plan/apply journal (0002)   │
                 ├──────────────────────────────────────────────────────────────────┤
  L1 syntax      │  tree-sitter trees (incremental)  ·  own SSR engine (0004)       │
  L2 symbols     │  symbol IDs · oracle broker → rust-analyzer | ty | tsc 7 (0003)  │
  L3 flow        │  own per-function IR on L1 + call edges from L2 (later)          │
  L4 intent      │  EmbeddingGemma 2 via ort, background indexing (0005, last)      │
                 ├──────────────────────────────────────────────────────────────────┤
                 │  VFS: file watcher · content hashes · UTF-8 line index           │
                 └──────────────────────────────────────────────────────────────────┘
```

## Principles
1. **The oracle is the source of truth, and semcode checks it.** Every spike found an oracle
   rename bug (ty skips `super().send`; none of the three renames overrides; TS misses
   string keys in `vi.spyOn(x, "name")`). semcode adds what the oracles miss (override expansion,
   string/doc candidates as *uncertain* edits) and **re-checks after apply** (diagnostic delta).
2. **Readiness before answers.** tsserver and basedpyright silently return partial results while
   loading (1 of 1048 refs; 7 of 11 refs). The broker tracks each backend's readiness (progress
   tokens, plus a canary query) and won't serve L2 answers early. Instead it either waits within
   the budget or answers at L1 with an explicit `degraded: syntax-only` flag.
3. **UTF-8 everywhere.** All three chosen oracles negotiate `positionEncoding: utf-8`. A conversion
   layer still exists for any oracle that doesn't.
4. **semcode watches files itself.** `ty server` doesn't watch files. semcode owns the VFS,
   forwards `didChange`/`didChangeWatchedFiles`, and hashes content for plan staleness checks.
5. **Library embedding is an upgrade path, not a starting point.** The LSP and embedded-library
   modes produced identical results (Rust, Python). We switch an oracle to a library (`ra_ap_*`,
   ty crates) only when we need something LSP doesn't expose, which most likely means body-level
   inference for L3.

## Crate layout (initial)
```
crates/
  semcode-core     # spans, UTF-8 line index, VFS, content hashes, symbol IDs
  semcode-lang     # tree-sitter grammars, per-language node kinds, outline/symbol extraction
  semcode-ssr      # pattern compiler, backtracking matcher, predicates, rewrite + trivia
  semcode-edit     # plans, staleness checks, apply, journal/undo, diff rendering
  semcode-oracle   # LSP client, per-language backend configs, readiness, broker
  semcode-render   # budgeted text/JSON rendering, cursors
  semcode-daemon   # semcoded: workspace state, request handling
  semcode-cli      # `semcode` binary: CLI + `semcode mcp`
```

## Build order
1. **M1, L1 for all three languages:** `find`, `rewrite`, `outline`, `read`, `edit`, plans and apply,
   budgeted output, the CLI, **and a thin in-process `semcode mcp`** (stdio, no daemon yet). This
   is useful on day one with no oracle required. Shipping MCP in M1 lets us use semcode from agents
   (including the one building it) while M2 is in progress, so the agent-facing design gets tested
   early. Gate: eval section S1–S12 plus X1, X2, X5.
2. **M2, L2 via oracles:** `refs`, `rename`, `callers`, `info`, typed SSR predicates, starting
   with Rust and then Python and TS (all three share one LSP client).
3. **M3, agent surface:** the daemon (keeping oracles warm across MCP sessions), `fixes`/`fix`
   (code actions plus preview), and diagnostic deltas.
4. **M4, L3:** intra-procedural def-use and data flow, plus `flow --to/--from`.
5. **M5, L4:** `ask` with EmbeddingGemma 2, re-ranked by structure.

## What would change this
- If an oracle's LSP surface blocks a core verb, embed that oracle as a library.
- If juggling three oracle processes proves too heavy for agent laptops (about 1.5 GB combined),
  start them lazily per language and stop them when idle.
