# ADR-0004: Own structural search & replace engine

Status: accepted (2026-10-14). Evidence: `spikes/ssr/README.md` (prototype `src/engine.rs`).

## Decision
Build our own SSR engine on tree-sitter. We don't depend on `ast-grep-core`, but we borrow its
proven ideas: contextual patterns for fragments, and strictness levels.

## Why
In the spike's tests (ast-grep-core 0.50.0, our prototype of about 690 LOC):

| Test | ast-grep-core | our prototype |
|---|---|---|
| `f($$$A, $B)` on `f(1,"x",2)` | 0 matches (no `$$$` backtracking) | 1 |
| Predicate inside matching (e.g. `$Y==2` choosing the split) | not possible, only after the match | ✓ |
| Nested rewrites in one pass | 37 leftovers on the corpus | ✓ |
| Empty `$$$ARGS` + appended kwarg | `get(, timeout=10)` | `get(timeout=10)` |
| Inline comments and multi-line layout | lost/garbled | preserved (dropped ones are reported) |
| Throughput (parse+match) | ~200–260 files/s | ~170–260 files/s |

Semantic predicates (types, resolved symbols) are what make this *IntelliJ* SSR rather than
syntactic grep. They have to run inside backtracking, and that is the deciding factor.

## Design notes for the real engine
- A pattern is parsed by the target grammar, with metavariables replaced by placeholder
  identifiers. It is compiled **per grammar** (TS and TSX patterns differ), and the language is
  chosen by file extension.
- `$X` matches one node, `$$$X` matches a (possibly empty) sequence, `$_` is an anonymous wildcard.
  A repeated metavariable must match structurally equal subtrees.
- Predicates are a trait (`fn check(&self, binding, ctx) -> Tri`), with sync L1 predicates and
  batched async L2 predicates. L2 predicates run after a syntactic prefilter to avoid one oracle
  round-trip per candidate.
- Backtracking has a **step budget** with memoisation on `(pattern node, tree node, seq index)`.
  The spike measured 354–729 ms worst case on a 400-argument call with three `$$$`.
- Replacements splice tokens and keep original trivia wherever neighbouring tokens survive. List
  separators are handled by the grammar's list context, re-indented for Python, and the result is
  re-parsed. A rewrite that introduces a parse error is rejected.
- Nested matches are rewritten innermost-first in one pass. Rewriting twice must be a no-op
  (tested).
- Escape hatch: raw tree-sitter queries with our custom predicates.

## What would change this
If ast-grep upstream adds backtracking `$$$` and in-match predicate hooks, we reconsider using it
for the matcher and keep our rewrite/trivia layer.
