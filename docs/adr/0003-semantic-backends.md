# ADR-0003: Semantic (L2) backends

Status: accepted (2026-10-14). Evidence: `spikes/{rust-l2,python-l2,ts-l2}/README.md`.

## Decision
| Language | Oracle | Transport | Pin |
|---|---|---|---|
| Rust | rust-analyzer | LSP stdio, UTF-8 | 0.3.3081 (2026-10-12) |
| Python | ty | `ty server`, LSP stdio, UTF-8 | 0.0.87 |
| TypeScript | TypeScript 7 (Go native) | `tsc --lsp --stdio`, UTF-8 | 7.0.2 |

## Evidence (summarised; machines were loaded, so times are ±3x)
| | rust-analyzer (ripgrep) | ty (httpx) | tsc 7 (zod) |
|---|---|---|---|
| Load until refs are correct | 6–15 s | ~1 s | ~0.6 s |
| Warm refs | 4 ms | 2–5 ms | 18–40 ms |
| Memory | ~1 GB | ~85 MB | 330–470 MB |
| Refs through aliases/re-exports | ✓ | ✓ 11/11, 0 extra | ✓ 1048 |
| Rename | ✓ 33 edits across 4 crates, `cargo check` passes | ✗ misses `super().send`; no override expansion | ✓ but misses string keys |

## Rejected alternatives
- **Embedding `ra_ap_*` crates:** same results and memory, plus a 5-minute build, weekly API
  churn and toolchain coupling. Kept as the upgrade path for L3.
- **ty as a library:** works (compiled first try against rev `1ae6552a`), but crates are
  `publish=false` with no semver promises, the first build takes 11 minutes, and its salsa may
  conflict with rust-analyzer's. Revisit for L3.
- **basedpyright:** correct, including `super()` in rename, but uses 4x more memory, needs node, is
  UTF-16 only, and returns partial results while indexing.
- **tsserver via typescript-language-server:** same answers, 3x more memory, UTF-16 only,
  and the syntax server silently answers early with wrong results.
- **tsc 7 `--api` (msgpack):** no rename or call hierarchy, node-index results, UTF-16, and no
  stable wire protocol.
- **Our own resolver on tree-sitter:** about 31% precision on `Client.send` with name matching,
  and an estimated 6–8 of 11 with scope graphs. Kept only as a degraded fallback.
- **oxc_semantic:** 7 MB, under 0.2 s for all of zod, but per-file only with no member refs.
  Possibly useful later as an instant first tier for TS.

## Obligations this creates for semcode
1. Readiness gating per backend (ADR-0001 §2).
2. File watching and change forwarding (ty doesn't watch).
3. Rename hardening: expand renames to overrides via type hierarchy and implementations. Capability
   check on 2026-10-14: `tsc 7.0.2 --lsp` advertises `implementationProvider` and
   `callHierarchyProvider` but **not** `typeHierarchyProvider`, so TS override discovery uses
   `textDocument/implementation` on the base member, with an L1 fallback for `extends` and
   `implements` clauses. Add
   `super()` call sites; list string/doc occurrences as *uncertain*; apply, then diff diagnostics.
4. Run oracles against the user's tree but never let them write there. (rust-analyzer build
   scripts write to `target/`, which is normal for cargo users but still worth documenting.)
5. Locate binaries robustly, and report `semcode doctor` when one is missing or broken. (On this
   machine `~/.cargo/bin/rust-analyzer` is a broken rustup proxy.)
