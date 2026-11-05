# semcode evaluation corpus

These are tasks an agent actually runs into, chosen so that `grep`/`rg` alone gives wrong
or incomplete answers. Every candidate approach (backend, matcher, index) is scored
against this list. Ground truth is checked by hand once and then frozen in
`docs/eval/truth/<id>.json` (a list of `file:line:col` spans, or an expected diff).

Corpus (pinned, cloned into `corpus/`, which is gitignored):

| repo | commit | lang |
|---|---|---|
| BurntSushi/ripgrep | `3fce3b5` | Rust (cargo workspace, 10 crates) |
| encode/httpx | `b5addb6` | Python |
| colinhacks/zod | `680d19f` | TypeScript (monorepo; v3 and v4 share names) |

Layers: **L1** syntax · **L2** symbols/types · **L3** call graph / data flow · **L4** intent/embeddings.

## Rust: ripgrep

| id | agent task | why grep fails | layer |
|---|---|---|---|
| R1 | Find all call sites of `grep_searcher::Searcher::search_path` | three different `search_path` fns exist (`core/search.rs:342` method, `core/search.rs:380` free fn, `searcher/mod.rs:643`) | L2 |
| R2 | Rename trait method `Matcher::find_at` → `find_at_offset` across the trait, all impls (pcre2, regex, testutil, test utils) and all 30 call sites | name-based replace also hits unrelated methods; call sites are untyped text | L2 |
| R3 | Incoming call hierarchy of `core/search.rs::search_path` (free fn), depth 3 | needs name resolution and a transitive call graph | L3 |
| R4 | List `.unwrap()` calls on `Result` (not `Option`) outside `#[cfg(test)]` / `tests/` | needs the receiver type and test-context filtering | L1+L2 |
| R5 | Codemod: `$X.unwrap()` on `Result` in `crates/core/**` → `$X?`, only inside fns that return `Result` | type of `$X` plus the enclosing fn's return type | L1+L2 |
| R6 | "Where is the decision to search a file as binary made?" | natural language, no keyword anchor | L4 |

## Python: httpx

| id | agent task | why grep fails | layer |
|---|---|---|---|
| P1 | Find all callers of `httpx.Client.send` (not `AsyncClient.send`, not generator `flow.send(...)` in `_auth.py`/`_client.py:949`) | `.send(` matches generator protocol and the async variant | L2 |
| P2 | Rename `BaseTransport.handle_request` → `handle` across the hierarchy (`wsgi.py`, `default.py`, `mock.py`, and callers) without touching `handle_async_request` | override relations + attribute calls | L2 |
| P3 | Find callers of `Client.send` that also pass `follow_redirects=` | structural kwarg predicate + resolution | L1+L2 |
| P4 | Where does the `follow_redirects` argument of `Client.request` flow? (to which reads/decisions) | intra-/inter-procedural data flow | L3 |
| P5 | Codemod: two-line `if $V is None:` / `    $V = $D` → `$V = $D if $V is None else $V`, only where `$V` is a local or parameter (not `self.x`) | multi-line, indentation-sensitive match and replacement + scope check | L1+L2 |
| P6 | "Where are proxy settings read from the environment?" | natural language | L4 |

## TypeScript: zod

| id | agent task | why grep fails | layer |
|---|---|---|---|
| T1 | Find all usages of v4 `ZodType.safeParse` (exclude v3 `ZodType.safeParse` and test files) | v3 and v4 define identically named APIs | L2 |
| T2 | Rename an exported v4 helper and fix every import and re-export chain (`export * from`, `export { x as y }`) | re-export chains, aliases | L2 |
| T3 | Find `$A as any` casts whose operand type is not already `any`/`unknown` | type of operand | L1+L2 |
| T4 | Incoming callers of v4 core `parse` internals, depth 2 | call graph through method dispatch | L3 |
| T5 | Codemod: `import { $$$N } from "zod/v3"` → `"zod/v4"`, only in files where every imported name exists in v4 | sequence metavariable + export lookup | L1+L2 |
| T6 | "How are error messages localized?" | natural language | L4 |

## L1 structural search & replace (M1 gate)

Taken from the SSR spike probes (`spikes/ssr/`), these are cases where grep fails and so does
ast-grep-core 0.50. Expected outputs are frozen as integration tests in `crates/semcode-ssr/tests/`.

| id | pattern → template | input | expected |
|---|---|---|---|
| S1 | `f($$$A, $B)` | `f(1, "x", 2)` | 1 match; `$A`=`1, "x"`, `$B`=`2` (backtracking) |
| S2 | `[$$$X, $Y, $$$Z]` where `$Y` == `2` | `[1, 2, 3]` | 1 match; predicate chooses the split |
| S3 | `f($$$A, $B, $B)` | `f(1, 2, 2)` / `f(1, 2, 3)` | 1 match / 0 (repeated metavariable is structural equality) |
| S4 | `requests.get($$$A)` → `requests.get($$$A, timeout=10)` | `requests.get()` | `requests.get(timeout=10)` (no stray `, `) |
| S5 | same as S4 | `requests.get(u, headers=h,)` | single comma, trailing comma kept |
| S6 | same as S4 | multi-line call with an inline `# keep me` comment | layout and comment kept |
| S7 | `$X.unwrap()` → `$X.expect("todo")` | `a.unwrap().b.unwrap()` | both rewritten in one pass |
| S8 | Python `if $V is None:\n    $V = $D` → `$V = $D if $V is None else $V` | nested inside a method body | correct indentation; re-parses cleanly |
| S9 | `import { $$$N } from '$M'` | `import { a, b } from "x"` (double quotes) | matches (quote style is not structural) |
| S10 | any rewrite above, applied twice | — | second run yields 0 edits (idempotence) |
| S11 | `f($$$A, $$$B, $$$C)` on a call with 400 arguments | — | finishes in < 50 ms (step budget / memoisation) |
| S12 | TS pattern on a `.tsx` file | — | compiled per grammar; matches |

## Agent-ergonomics checks (all languages)

| id | check |
|---|---|
| X1 | The answer to any task above fits in 4k tokens by default, with pagination and a stable continuation token |
| X2 | Rename/codemod returns a preview with per-file content hashes; applying after a concurrent edit to any touched file is **refused** |
| X3 | Results carry stable symbol IDs that can be passed back (`semcode refs <symbol-id>`) |
| X4 | Warm query latency p50 < 50 ms (L1/L2), < 300 ms (L3/L4) on these repos |
| X5 | Applying a codemod twice is a no-op (idempotence) |
| X6 | After any rename, the project still type-checks (`cargo check` / `ty`/`pyright` / `tsc`) |
